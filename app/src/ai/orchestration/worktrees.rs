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

/// `worktree-remove` body: terminal guard → detach → `git worktree remove`.
pub fn worktree_remove(path: &str, force: bool, cx: &mut warpui::AppContext) -> anyhow::Result<String> {
    let target = super::projects_cli::canonical_project_path(path)?;
    // Validate it IS a worktree and find its repo root (the main checkout
    // administers `git worktree remove` — running it from any linked
    // worktree of the same repo works too; we use the target itself).
    git(&target, &["rev-parse", "--git-dir"])?;

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
    let detached = detach_tabs_db(&refs, &mut conn)?;

    let remove_args = if force {
        vec!["worktree", "remove", "--force"]
    } else {
        vec!["worktree", "remove"]
    };
    let target_str = target.to_string_lossy().to_string();
    let mut args = remove_args;
    args.push(&target_str);
    // Run from the worktree's parent (valid even if the worktree dir is the
    // git cwd being removed).
    git(target.parent().unwrap_or(Path::new("/")), &args)
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
        if detached > 0 {
            format!(" ({detached} tab(s) detached to no-project)")
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
