# Spec: observatory-panel

Status: Locked (P1)

## Problem
拦截四层（proxy/hooks/blocks/接线）与编排平面（runs/tasks/messages/dispatches）已有完整后端与数据，但 GUI 只有 tab badge + 输入框上方配置条；无观测会话 block 流、无编排状态视图、无 GUI 内消息发送闭环。

## Solution
右侧面板「观测台」（Observatory）：
1. 拦截状态头：当前 mode（复用 InterceptSessionsModel）、block 总数、刷新按钮
2. Sessions tab：harness session 列表（按时间倒序）→ 点击选中 → block 时间线（type/seq/大小/内容预览）
3. Orchestration tab：runs（倒序）→ 展开 tasks（状态着色）；消息 composer（to/subject/body → send-message 子进程）
4. 交互闭环：select/refresh/send/set-draft/switch-tab 全部经 ObservatoryModel；send 后自动刷新可见状态变化

## Out of Scope
- 交互式 CC tab 的拦截（另行工作项）
- block 内容编辑/导出、流式 live 推送（refresh 轮询足够）
- dispatch 明细编辑、gate 操作 UI
- wasm

## User Stories
- US1: 用户打开观测台，看到最近 harness session 与其 block 数
- US2: 用户点 session，看到 block 时间线与内容预览
- US3: 用户查看 runs/tasks 状态
- US4: 用户在 composer 填 to/subject/body 发送，刷新后 messages 变化可见

## Implementation Decisions
见 docs/adr/observatory-panel.md（ADR-1..5）。模块 `app/src/ai/observatory/`（model.rs + view.rs）；挂载 util.rs/action.rs/view.rs/header_toolbar_item.rs。

## Testing Decisions
- model 单测：temp sqlite 下 refresh 快照正确性、select/send 状态机（子进程 mock 不可行→send 测 busy 置位与 draft 清理，真实子进程走 e2e）
- e2e：`agent run --harness claude` 落库后面板数据源查询非空（DB 断言替代视觉）；send-message 经子进程落 messages 表

## Acceptance
- A1 面板可从工具栏按钮打开/关闭（ADR-1）
- A2 sessions/blocks 数据来自 harness_blocks.db 快照（ADR-2）
- A3 orchestration 数据来自 orchestration store（ADR-2/4）
- A4 send 闭环：composer → 子进程 → DB message → 刷新可见
- A5 全部状态变更经 model（无视图本地业务状态）
- A6 cargo test 相关套件全绿 + workspace check

## Defer
- block 详情大视图/搜索过滤
- dispatch/gate 操作
- live 推送（block 增量事件订阅）
