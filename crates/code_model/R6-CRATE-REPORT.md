# R6 交付报告 — code + code_review 域 crate 化下沉

HEAD 基线 `03669ee0`（feat/gui-cockpit-nav），未切分支，未执行任何 cargo 命令。
模式对齐 R4（cd31e479 ssh_manager_ui/sftp_manager_ui）：纯文件同刀切 git mv +
宿主路径兼容 shim，语义零漂移。命名对齐 notebooks_model 先例（纯模型层 →
`<domain>_model`）。

## 一、下沉内容（12 个文件，3316 行源码 git mv 保留历史，全部 100% rename）

### crates/code_model/（新 crate，code 域纯模型层）

| 文件 | 来源 | 说明 |
|---|---|---|
| src/buffer_location.rs | app/src/code/buffer_location.rs | RemotePath/BufferLocation/SyncClock（warp_core HostId + warp_util StandardizedPath）；内联单测随迁 |
| src/global_buffer_model.rs | app/src/code/global_buffer_model.rs | GlobalBufferModel 1541 行 buffer 生命周期中枢（FileModel/SaveBuffer 双路径，local_fs/local_tty feature 门控随迁） |
| src/file_tree/snapshot.rs | app/src/code/file_tree/snapshot.rs | FileEntry/FileTreeSnapshot sum_tree 快照模型；`#[path]` 接线（snapshot/iterator.rs、snapshot_tests.rs）随迁有效 |
| src/file_tree/snapshot/iterator.rs | 同名迁移 | 快照迭代器（pub(super) 均为 subtree 内部消费，随迁后语义不变） |
| src/file_tree/snapshot_tests.rs | 同名迁移 | 437 行快照单测（use super::*，随 snapshot.rs #[path] 接线） |
| src/icon.rs | app/src/code/icon.rs | icon_from_file_path 纯函数（扩展名→warpui 图标） |
| src/active_file.rs | app/src/code/active_file.rs | ActiveFileModel/ActiveFileEvent |
| src/opened_files.rs | app/src/code/opened_files.rs | OpenedFilesModel/OpenedFilesInRepo |
| src/editor/line.rs | app/src/code/editor/line.rs | EditorLineLocation（code_review::comments 等跨域消费，经 shim 路径不变） |
| src/editor/line_iterator.rs | app/src/code/editor/line_iterator.rs | LineIterator 纯迭代器 |
| src/editor/scroll.rs | app/src/code/editor/scroll.rs | ScrollWheelBehavior/ScrollPosition/ScrollTrigger |
| src/lib.rs（新写） | app/src/code/mod.rs 根级胶水 verbatim 迁入 | DiffResult、ImmediateSaveError、SaveStatus、SaveOutcome、ShowCommentEditorProvider、NoopCommentEditorProvider、EditorTabBarDropTargetData。**唯二内容改动**：NoopCommentEditorProvider 私有→pub、EditorTabBarDropTargetData.index 私有字段→pub（均为 app code/view.rs 跨 crate 构造必要，对齐 R4 pub(crate)→pub 先例） |

依赖：anyhow/bimap(=0.6.2 同 app 声明)/cfg-if/futures-util/instant/log/
pathfinder_geometry/serde/string-offset/sum_tree/thiserror/warp_core/
warp_editor/warp_util/warpui（均 workspace）+ warp_files（optional，随 local_fs）。

features：`local_fs = ["dep:warp_files"]`、`local_tty = []` — global_buffer_model
与 snapshot.rs 内有**真实代码门控**（非仅 dead_code allow），必须由 app 转发。

### crates/code_review_model/（新 crate，code_review 域纯模型层）

| 文件 | 来源 | 说明 |
|---|---|---|
| src/hidden_lines.rs | app/src/code_review/hidden_lines.rs | calculate_hidden_lines（diff 上下文折叠隐藏行区间纯计算，RangeSet 求补；内联单测随迁） |

依赖：ai / rangemap / warp_editor（均 workspace）。零 feature。

## 二、宿主留守层改写（4 文件，旧路径全量保活）

- `app/src/code/mod.rs`：重写为留守层 — 域 init()、view/wasm/local_code_editor/
  diff_viewer/inline_diff/footer/editor_management 声明保留；buffer_location/
  global_buffer_model/icon(私有)/active_file/opened_files 五模块 shim +
  根级 7 类型 re-export。`crate::code::*` 全部既有路径不变（pane_group、
  remote_server、workspace、search、terminal、ai 等 31 个域外消费文件零改动）。
- `app/src/code/editor/mod.rs`：line(pub)/line_iterator(私有,原样)/scroll(pub)
  三 shim；视图主体声明与其余 re-export 原样。
- `app/src/code/file_tree/mod.rs`：snapshot shim；view 声明与 pub use view::* 原样。
- `app/src/code_review/mod.rs`：单处替换 — `mod hidden_lines;` → inline shim。

## 三、留守边界（物理依赖锚点清单，裁决对齐 R2-a/R2-b/R4）

code 域留守：
- `editor/{model,view,element,diff,comments,comment_editor,embedded_comment,
  nav_bar,find,goto_line}.rs`：`crate::t!` i18n、`crate::appearance`
  （diff.rs add_color 的 Appearance 参数）、`crate::util::link_detection`
  （model.rs）、`crate::code_review` 反向锚（comments.rs 的
  EditorReviewComment 被 code_review::comments 消费 → 双侧留守，互锚）；
- `view.rs`/`wasm.rs`：`crate::pane_group`（R3 主线互斥区）/terminal/workspace；
- `local_code_editor*.rs`：持有 `crate::terminal::TerminalView`，随 terminal 域
  crate 化再迁；
- `diff_viewer.rs`/`inline_diff.rs`：`crate::editor::InteractionState`、
  `crate::ai::blocklist`；
- `file_tree/view.rs` 全家、`footer.rs`、`editor_management.rs`：dispatch 宿主
  Action / `crate::settings` / `crate::ui_components`。

code_review 域留守（本次仅 hidden_lines 物理可移，其余全锚）：
- `diff_state.rs`（3017 行）：`crate::features::FeatureFlag` /
  `crate::terminal::local_shell::LocalShellState` / `crate::throttle` /
  `crate::util::git` / `super::telemetry_event`；
- `comments/` 全家（~1300 行）：`crate::ai::agent::{CurrentHead, DiffBase}`、
  `crate::code::editor::EditorReviewComment`（留守文件）、
  `super::diff_state::{DiffMode, DiffLineType, DiffStateModel}` 三重锚 —
  任一锚不解除则整子树不可移（互锚结构，待 ai 域/diff_state 拆层专项）；
- `diff_size_limits.rs`/`file_invalidation_queue.rs`：引用留守 diff_state 类型；
- `telemetry_event.rs`：`crate::server::telemetry` / `crate::view_components::find`
  / `super::diff_state::DiffMode`；
- 视图全家（code_review_view 7514、comment_list_view、comment_rendering、
  git_dialog/*、diff_menu、diff_selector、find_model、context、
  git_status_update、scroll_preservation、editor_state）：`crate::t!`、
  `crate::code::local_code_editor::LocalCodeEditorView`（留守）、
  `crate::appearance`、`crate::ai` 等宿主依赖。

## 四、自检 grep（三契约验收，全绿）

```
$ grep -rnE 'ObjectStoreModel|dais::|(^|[^a-z_:])app::|crate::' \
    crates/code_model/src crates/code_review_model/src --include='*.rs'  # 剥离注释后
→ 零匹配（新 crate 零 app 代码级引用、零 ObjectStoreModel）

$ grep -rnE '(^|[^a-zA-Z_])t!\(|(^|[^a-zA-Z_])fl!\(' 同上
→ 零匹配（无 i18n 宏，无需 ftl 资产副本）

$ git diff --cached -M --summary | grep rename
→ 12 文件全部 100% rename，0 insertions/deletions（迁移文件字节级不变）
```

附加门（cargo 禁用下的替代验证）：
- `rustfmt --check` 逐文件解析门：8 个手写/改写文件全部 PARSE-OK，
  且 rustfmt 模块树解析无一条 `failed to resolve mod`（所有声明模块文件齐备，
  修复过程中抓到并已修复一处 active_file/opened_files 声明与 shim 重复）；
- 再导出符号可见性审计：lib.rs 再导出的 8 个符号在源文件均为 pub；
- movers 全文 pub(crate)/pub(super) 扫描：仅 snapshot subtree 内部 2 处
  pub(super)（随迁后仍为 subtree 内部消费，语义不变）；
- 域外消费点全集（31 文件）经 shim 路径保活，零改动。

## 五、待主会话收敛（依赖注册 + 编译门，对齐 R4→03669ee0 先例）

1. 根 Cargo.toml `[workspace.dependencies]` 追加（members 已是 `crates/*` glob）：
   ```toml
   code_model = { path = "crates/code_model" }
   code_review_model = { path = "crates/code_review_model" }
   ```
2. app/Cargo.toml `[dependencies]` 追加：
   ```toml
   code_model.workspace = true
   code_review_model.workspace = true
   ```
3. **app/Cargo.toml `[features]`（必须，否则语义漂移）**：
   - `local_fs` 列表追加 `"code_model/local_fs"`
   - `local_tty` 列表追加 `"code_model/local_tty"`
   （app 默认 feature 含 local_fs/local_tty；global_buffer_model/snapshot 内
   有真实 `#[cfg(feature = ...)]` 代码门控，不转发则分支体在 crate 侧恒假。）
4. Cargo.lock 由后续任意 cargo 命令自动再生（本 worker 按约束未执行）。
5. 之后跑编译门 + 定向测试（snapshot_tests 437 行、buffer_location、
   hidden_lines 内联单测已随迁，走新 crate cfg(test)）。

## 六、改动文件清单（我方产物）

- 新建：crates/code_model/{Cargo.toml, src/lib.rs, src/editor/mod.rs,
  src/file_tree/mod.rs}、crates/code_review_model/{Cargo.toml, src/lib.rs}
- git mv（R，staged）：code 域 11 文件、code_review 域 1 文件（见上表）
- 修改：app/src/code/mod.rs（留守层+shim）、app/src/code/editor/mod.rs、
  app/src/code/file_tree/mod.rs、app/src/code_review/mod.rs（单 hunk）
- 未触碰：app/Cargo.toml、根 Cargo.toml、其他任何域；工作区并行改动
  （crates/warp_completer 3 文件 + .completer-deadlock-sweep-report.md）属
  R5/产品票，未动。
