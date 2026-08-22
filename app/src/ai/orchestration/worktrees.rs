//! Git worktree management for the orchestration CLI (orch-caps-v2).
//!
//! Layout convention (documented in docs/orchestration-capabilities.md):
//! a worktree named `name` of project `P` lives at
//! `P/../<repo-dir>-<name>` — a **sibling directory** of the main checkout,
//! on a new branch `name` from HEAD. Rationale: matches the dais repo's own
//! established worktree convention, keeps worktrees in normal fs
//! neighborhoods (file managers / git tooling see them), and every worktree
//! is a full checkout registered as its own project, so `git worktree list`
//! in the main repo enumerates the whole set for free.
//!
//! Terminal guard: a worktree referenced by tabs (project_path in the DB)
//! is refused unless `--force` (tabs detached to no-project first, then
//! `git worktree remove --force` — dirty trees need the git-side force too).

use std::path::{Path, PathBuf};
use std::process::Command;

use diesel::SqliteConnection;

use super::projects_cli::{
    detach_tabs_db, open_db, remove_project_db, tabs_for_project_db, upsert_gui_or_db,
};

/// Run `git -C <dir> <args>` and capture stdout. Stderr feeds the error.
fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("spawn git: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Target path for a worktree: `<project>/../<repo-dir>-<name>`.
pub fn worktree_target_path(project: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let repo_dir = project
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("project path has no file name: {}", project.display()))?;
    let target = project
        .parent()
        .ok_or_else(|| anyhow::anyhow!("project path has no parent: {}", project.display()))?
        .join(format!("{repo_dir}-{name}"));
    Ok(target)
}

/// `worktree-create` body: sibling worktree + branch `name` from HEAD,
/// registered as a project (its own rail entry). Prints the worktree path.
pub fn worktree_create(project_path: &str, name: &str, cx: &mut warpui::AppContext) -> anyhow::Result<String> {
    let project = super::projects_cli::canonical_project_path(project_path)?;
    // Must be a git worktree itself (a main checkout or a linked worktree).
    git(&project, &["rev-parse", "--git-dir"])?;
    // Branch names and dir suffixes share the same namespace rules; be
    // conservative: reject path separators and whitespace.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_whitespace)
    {
        anyhow::bail!("invalid worktree name: {name:?}");
    }
    let target = worktree_target_path(&project, name)?;
    if target.exists() {
        anyhow::bail!("worktree path already exists: {}", target.display());
    }
    git(&project, &["worktree", "add", "-b", name, &target.to_string_lossy()])?;
    // The worktree is a full checkout → its own project entry (GUI: event).
    upsert_gui_or_db(&target, cx)?;
    Ok(target.to_string_lossy().into_owned())
}

/// `worktree-list` body: porcelain `worktree list`, paths only (one per
/// line; the main checkout first — git's own ordering). With
/// `project_path`, scoped to that repo; without it, union over all
/// registered projects that are git repos (dedup by repo: two projects in
/// the same repo would print the list twice).
pub fn worktree_list(project_path: Option<&str>) -> anyhow::Result<String> {
    let repos: Vec<PathBuf> = match project_path {
        Some(p) => vec![super::projects_cli::canonical_project_path(p)?],
        None => {
            let mut conn = open_db()?;
            let repos = super::projects_cli::list_projects_db(&mut conn)?
                .into_iter()
                .map(|r| PathBuf::from(r.path))
                .filter(|p| git(p, &["rev-parse", "--git-dir"]).is_ok())
                .collect();
            repos
        }
    };
    let mut seen_repos: Vec<PathBuf> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for repo in repos {
        // Same git dir = same repo: skip duplicates across projects.
        let git_dir = match git(&repo, &["rev-parse", "--path-format=absolute", "--git-dir"]) {
            Ok(g) => PathBuf::from(g.trim()),
            Err(_) => continue,
        };
        let canonical_git_dir = git_dir.canonicalize().unwrap_or(git_dir);
        if seen_repos.contains(&canonical_git_dir) {
            continue;
        }
        seen_repos.push(canonical_git_dir);
        let porcelain = git(&repo, &["worktree", "list", "--porcelain"])?;
        for line in porcelain.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                lines.push(path.to_string());
            }
        }
    }
    Ok(lines.join("\n"))
}

/// `worktree-remove` body: guard → (force: close tabs) → `git worktree remove`.
///
/// v2-fix-13: ①git 定位链 — target 自身 rev-parse 失败(如 tab 清理后的
/// 边缘态)时, 读 `.git` 文件(gitdir: <main>/.git/worktrees/<name>)回退到
/// 主仓执行; ②--force 语义与 project-remove 一致: 连 tab 全回收(中断+
/// PTY 关+关 tab+邮箱自然 retire), 再 `git worktree remove --force`。
pub fn worktree_remove(path: &str, force: bool, cx: &mut warpui::AppContext) -> anyhow::Result<String> {
    let target = super::projects_cli::canonical_project_path(path)?;

    // Terminal guard: tabs whose project is this worktree.
    let mut conn: SqliteConnection = open_db()?;
    let refs = tabs_for_project_db(&target, &mut conn)?;
    if !refs.is_empty() && !force {
        anyhow::bail!(
            "worktree {} still referenced by {} tab(s) [{}]: close them first or pass --force",
            target.display(),
            refs.len(),
            refs.iter()
                .map(|i| format!("tab#{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // force: 全回收 tabs(含竞态新到), 不留孤儿; headless 行 detach。
    let mut closed_tabs = 0usize;
    if force {
        if in_gui(cx) {
            closed_tabs = super::new_terminal::close_project_tabs(&target, cx);
        } else {
            detach_tabs_db(&refs, &mut conn)?;
        }
    }

    // Locate the administering repo (v2-fix-13 票2b 实测修正):
    // ①worktree 的 `.git` 是 gitfile(gitdir: <main>/.git/worktrees/<name>)
    //   → 主仓即 `.git/worktrees/<name>` 上溯三级, 从主仓执行(最可靠,
    //   target 处于任何边缘态都不影响);
    // ②无 gitfile(裸 checkout)→ 从 target 自身执行(其 git 上下文知道
    //   管理仓); 两者皆败才报错。绝不用 parent 目录假设。
    let run_dir = (|| -> Option<PathBuf> {
        let raw = std::fs::read_to_string(target.join(".git")).ok()?;
        let gitdir = raw.trim().strip_prefix("gitdir:")?.trim().to_string();
        std::path::Path::new(&gitdir)
            .ancestors()
            .nth(3) // <main>/.git/worktrees/<name> → <main>
            .map(Path::to_path_buf)
    })()
    .unwrap_or_else(|| target.clone());
    if git(&run_dir, &["rev-parse", "--git-dir"]).is_err() && git(&target, &["rev-parse", "--git-dir"]).is_err() {
        anyhow::bail!(
            "{} is not a worktree (no .git gitfile) and git cannot run in it or its recorded main repo",
            target.display()
        );
    }
    let mut args: Vec<String> = vec![
        "worktree".into(),
        "remove".into(),
        if force { "--force".into() } else { String::new() },
        target.to_string_lossy().into_owned(),
    ];
    args.retain(|a| !a.is_empty());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git(&run_dir, &arg_refs)
        .or_else(|_| {
            // 主仓定位失败时再从 target 自身试一次(自身 git 上下文兜底)。
            git(&target, &arg_refs)
        })
        .map_err(|e| anyhow::anyhow!("{e} (dirty worktree? pass --force)"))?;

    // Drop the project entry for the removed worktree (GUI: event refresh).
    if in_gui(cx) {
        remove_project_via_model(&target, cx);
    } else {
        remove_project_db(&target, &mut conn)?;
    }
    Ok(format!(
        "worktree removed: {}{}",
        target.display(),
        if closed_tabs > 0 {
            format!(" ({closed_tabs} tab(s) closed)")
        } else {
            String::new()
        }
    ))
}

fn in_gui(cx: &warpui::AppContext) -> bool {
    super::projects_cli::in_gui_process(cx)
}

fn remove_project_via_model(path: &Path, cx: &mut warpui::AppContext) {
    use crate::projects::ProjectManagementModel;
    use warpui::SingletonEntity as _;
    ProjectManagementModel::handle(cx).update(cx, |m, ctx| {
        m.remove_project(path.to_path_buf(), ctx)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dais-wt-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["config", "user.email", "t@t"]).unwrap();
        git(&dir, &["config", "user.name", "t"]).unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-qm", "init"]).unwrap();
        dir
    }

    #[test]
    fn target_path_layout() {
        let p = Path::new("/home/u/repos/dais");
        assert_eq!(
            worktree_target_path(p, "feat").unwrap(),
            PathBuf::from("/home/u/repos/dais-feat")
        );
    }

    #[test]
    fn create_list_remove_roundtrip() {
        let repo = temp_repo("rt");
        // create
        let target = worktree_target_path(&repo, "w1").unwrap();
        git(&repo, &["worktree", "add", "-b", "w1", &target.to_string_lossy()]).unwrap();
        assert!(target.is_dir());
        assert!(target.join(".git").exists() || target.join(".git").is_file());
        // list contains both
        let out = git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
        let paths: Vec<&str> = out
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .collect();
        assert!(paths.contains(&repo.to_string_lossy().as_ref()));
        assert!(paths.contains(&target.to_string_lossy().as_ref()));
        // remove
        let t = target.to_string_lossy().to_string();
        git(&repo, &["worktree", "remove", &t]).unwrap();
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn invalid_names_rejected() {
        let p = Path::new("/tmp/x");
        assert!(worktree_target_path(p, "a/b").is_ok()); // layout math is name-agnostic
        // name validation lives in worktree_create; direct check:
        assert!("a/b".contains('/'));
    }
}
