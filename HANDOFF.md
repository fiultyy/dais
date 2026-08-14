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
