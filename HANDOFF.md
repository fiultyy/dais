# Session Handoff — Zap Fork dev/localize
# Date: 2026-08-13 03:30 UTC
# Branch: dev/localize @ e778aa21

## 身份
- Fork: `fiultyy/zap` (origin) ← `zerx-lab/zap` (upstream)

## 当前状态

### Git
| 项 | 值 |
|---|---|
| 分支 | `dev/localize` |
| HEAD | `e778aa21` feat(orchestration): P3 shell event bridge |
| origin 领先 | 6 个提交（未 push） |
| 工作区 | 干净（HANDOFF.md 未跟踪） |

### 未推送提交（6 个）
```
e778aa21 feat(orchestration): P3 shell event bridge — OSC 133 → DcsHookEvent
5dbc36c1 feat(orchestration): P2b PTY bridge — channel sender + PtyExecutor impl
e2bbaac4 feat(orchestration): P2 router — background message routing loop
df0c3679 feat(orchestration): P1 wiring — store connection + CLI registration
abc46c9a fix(orchestration): fix critical bugs in state machine, DAG, and gates
4b3b7996 feat(oss): enable agent/project/tab features for OSS channel
```

### 测试
- ai crate: 63 passed, 0 failed
- `cargo check -p warp --features orchestration`: 通过
- clippy: 零新警告

## 接线完成状态

### ✅ P1: Foundation（df0c3679）
Feature gate + OnceLock 连接单例 + CLI 6 子命令 + dispatch handler
**关键坑**: `::ai::` 前缀（app 有自己的 `mod ai`）

### ✅ P2a: Message Router（e2bbaac4）
std::thread daemon，500ms poll → drain_inbox → route_message

### ✅ P2b: PTY Bridge（5dbc36c1）
OrchestrationPtySender: PtyExecutor impl via SyncSender channel
Consumer（GPUI model drain）deferred to 终端注册机制

### ✅ P3: Shell Event Bridge（e778aa21）
ShellEventBridge: AnsiHandlerEvent → DcsHookEvent 映射
SessionDispatchMap: SessionId → dispatch_id 注册

## 已知限制 / 待完成
1. **PTY bridge consumer**: HandleRegistry + GPUI drain model 未接线
   （需要终端注册机制：编排 plane 如何知道哪些终端可用）
2. **Exit code propagation**: UserCommandFinished 不携带 exit_code（block-list
   层有，但未传到 ModelEvent）。当前假设 exit_code=0
3. **ShellEventBridge wiring**: 未订阅 ModelEventDispatcher（需要 per-pane
   或全局 singleton + active_session_id 追踪）
4. **PTY bridge bypass permissions**: 编排写入应绕过 WriteToPtyPermission
   （安全边界在 run 级别，非命令级别）

## 关键文件索引
```
# P1-P3 新增（app crate）
app/src/ai/orchestration/
├── mod.rs                   OrchestrationPtySender (PtyExecutor impl)
└── shell_event_bridge.rs    ShellEventBridge + SessionDispatchMap

# P1 新增
app/src/ai/agent_sdk/orchestration.rs    CLI dispatch handler
crates/ai/src/agent/orchestration/connection.rs   OnceLock 连接单例
crates/warp_cli/src/orchestration.rs     CLI 命令定义

# P2 新增
crates/ai/src/agent/orchestration/router.rs   MessageRouter

# 核心模块（未修改）
crates/ai/src/agent/orchestration/
├── mod.rs, types.rs, db.rs, store.rs, worker.rs
├── reconciliation.rs, messaging.rs, groups.rs
├── executor.rs (PtyExecutor + WorkerStatusDetector traits)
└── output.rs
```

## 构建命令
```bash
export PATH="$HOME/.local/bin:$PATH"
cargo check -p warp --features orchestration --lib
cargo test -p ai --features orchestration --lib orchestration
./script/bundle -c oss
```

---

# P0 布局硬伤修复实施小节（2026-08-15）

> 分支 `fiultyy/gui-p0-impl` @ `a97414e0`。需求来源: 规划 worktree
> `gui-layout-dsh-comparison/docs/plans/gui-layout-dsh-comparison.md` §4 P0。
> 本任务只做 P0（布局），不碰 P1/P2。

## 改动

### P0-1 观测台滚动容器 + 列表/详情分区
- `app/src/ai/observatory/view.rs`: Sessions tab 与 Orchestration tab 重构为
  「固定头（header + tab bar + 搜索框）+ 滚动列表区 + 固定详情区」。
  列表区为唯一滚动口（`ClippedScrollable::vertical` 包裹）；
  详情卡（block/raw/task/消息详情 + composer）移出滚动区，固定面板底部，
  选中项不随列表滚动丢失。
- 五个列表（sessions / blocks / raw / messages / archives）接
  `UniformList` 虚拟化，等高行 30px（`row.rs` 的 `LIST_ROW_HEIGHT`），
  行内文本单行 ellipsis 截断。
- `wrap_virtual_list` helper 统一虚拟化包裹；build 闭包按 'static 捕获
  （theme 克隆 + FamilyId/f32 字体参数 Copy + 行数据克隆）。

### P0-2 行密度与状态点体系
- 新增 `app/src/ai/observatory/row.rs`:
  - `status_dot(status, theme)`: task.status / gate.status / worker
    dispatch state 字符串枚举 → (Icon, 语义色) 映射表，颜色走 WarpTheme
    语义 accessor（ansi_fg_green/red/yellow/magenta/blue）。
  - `list_row(...)`: 等高行构建 helper（状态点 + 主文本 + 右对齐辅助列
    + 辅助文本）。
- view.rs: task 行改 Icon 状态点（原 6px 色块 → 12px 图标）；
  dispatch 行 / 已决 gate 复用同一映射；旧 `task_status_color` 硬编码
  RGB 删除。
- 布局常量集中模块头 const 块，新增
  `OBSERVATORY_PANEL_{MIN_WIDTH,MAX_WIDTH_RATIO,DEFAULT_WIDTH}`。

### P0-3 面板 Resizable + 持久化
- `app/src/workspace/view.rs:18202` 挂载处包 `Resizable`
  （DragBarSide::Left，clamp 320 / 0.6×window）。
- 持久化全链路: `ModalType::ObservatoryWidth` + `ModalSizes.observatory_width`
  （`app/src/terminal/resizable_data.rs`）→ `WindowSnapshot.observatory_width`
  （`app/src/app_state.rs`）→ `windows.observatory_width` 列
  （`crates/persistence` model/schema + migration
  `2026-08-15-000000_add_observatory_width`）→ sqlite 读写
  （`app/src/persistence/sqlite.rs`）→ workspace 快照读写。
- `observatory_resizable_state()`: 视图初始化时从 ResizableData 单例
  取持久化句柄，恢复窗口宽度。

## 验证
- `cargo check -p warp --features orchestration` ✅
- `cargo clippy -p warp --features orchestration`: 观测台/持久化相关
  零新警告（残留 2 处 warning 均为预存在，git stash 对照确认）。
- 单测: `cargo test -p warp --features orchestration observatory` 11/11 ✅
  - 状态点映射表: 已知枚举值有确定 Icon + 可见色；语义分组（完成绿/
    失败红/等待黄）正确; 未知回落 Circle。
  - **虚拟化自证** `test_uniform_list_virtualizes_500_items`: 500 行
    UniformList 在测试窗口构建场景后只构建 50 行（可见行数），非全量
    500 —— 满足规划验收标准「500 blocks 场景帧构建元素数 ≈ 可见行数」。
  - sqlite round-trip 7/7 ✅（observatory_width 列读写）。

## 与规划的偏差
1. **滚动口实现**: 规划提 `Scrollable 包 UniformList`；实际采用
   `ClippedScrollable::vertical` 包含多个 UniformList 的 Flex 列。
   原因: Sessions tab 单滚动口需容纳多段列表（sessions+blocks+raw），
   `Scrollable` 要求 child 实现 `ScrollableElement`（Flex 列不满足），
   `ClippedScrollable` 是仓库内任意元素树的标准滚动口方案
   （enum_creation_dialog.rs / workflow_view.rs 同款）。UniformList
   自身仍处理滚轮命中其区域的滚动。
2. **详情区位置**: 规划提「列表底部 dock 或独立检查器列」；采用底部
   固定 dock（独立检查器列留给 P2-1 主从重构，规划亦如此定位）。
3. **`OBSERVATORY_PANEL_MAX_WIDTH=480` 移除**: 该常量语义被 P0-3
   Resizable clamp 取代；render() 内保留 1600px 兜底 max_width 仅为
   防面板槽位水平无限约束 assert。
4. **行高**: 规划 28-32px 取值区间，落定 30px（`LIST_ROW_HEIGHT`）。
5. **run/task 列表未虚拟化**: Orchestration tab 的 runs+tasks 分组
   嵌套结构（run 头 + 缩进 task 子行）与 UniformList 等高扁平模型
   不匹配，且数据量级为 50 run × 200 task 上限，保持原渲染在滚动
   容器内（滚动口已解决撑爆面板问题）。扁平化两级树属 P2-1 主从
   重构范畴。

## 遗留
- 拖拽手柄命中区 5px 偏窄（`resizable.rs:29` TODO，规划 P0-3 风险项
  已点名）: 属框架层改动，影响所有 Resizable 使用方，未在本任务顺手
  改（避免 P0 扩面）。建议独立小 PR。
- archives 列表行高与 LIST_ROW_HEIGHT 不一致（SMALL_FONT 文本行），
  视觉密度略低于其他列表; 归档行结构化（DisclosureRow 折叠头）在
  P2-2 规划内。
- `cargo fmt` 本仓存在大量历史未格式化文件，本 commit 只格式化了
  触碰的文件。
