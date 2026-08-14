# Spec: observatory-panel

Status: Implemented (P1 + follow-ups)

## Problem
拦截四层（proxy/hooks/blocks/接线）与编排平面（runs/tasks/messages/dispatches/gates）已有完整后端与数据，但 GUI 只有 tab badge + 输入框上方配置条；无观测会话 block 流、无编排状态视图、无 GUI 内消息发送闭环。

## Solution
右侧面板「观测台」（Observatory），三个 tab：
1. 拦截状态头：当前 mode（复用 InterceptSessionsModel）、block 总数、刷新按钮
2. Sessions tab：搜索过滤（session id / block type / raw direction+content，LIKE 子串匹配+通配符转义）→ harness session 列表 → block 时间线（点击选中→详情卡片：parent/metadata/content）→ raw 代理流量（direction badge/预览，点击→载荷详情）
3. Orchestration tab：runs → tasks（点击选中）→ task panel（Dispatch 按钮 + dispatch 明细：dispatch_contexts JOIN worker_dispatches）→ pending gates（选项 chip 一键 resolve / 自定义 resolution）→ 消息 composer（send-message 子进程闭环）
4. Proxy tab：拦截模式三态 chips、活跃拦截会话（GUI 交互 CC tab 的 proxy 端口/hook URL 运行态）、upstream base/auth env 覆盖输入、Claude Code/Codex 双 harness 解析探测、block 计数刷新
5. 交互闭环：全部经 ObservatoryModel（MVU 单源）；5s 自动刷新（面板打开时；`panel_open` gate 关闭时跳过 DB 轮询）；五类列表行点击已选中项 toggle 取消

## Out of Scope
- wasm
- 交互式 CC tab 拦截 → 已另行落地（`harness_intercept::intercept_claude_command`，见 workspace/view.rs `add_tab_with_specific_agent`）
- block 内容编辑/导出

## User Stories
- US1: 用户打开观测台，看到最近 harness session 与其 block 数
- US2: 用户点 session，看到 block 时间线与内容预览；点 block 看 metadata/content 全文；看 raw 代理流量载荷
- US3: 用户查看 runs/tasks 状态；选中 task 派发、看 dispatch 明细；解决 pending gate
- US4: 用户在 composer 填 to/subject/body 发送，刷新后 messages 变化可见
- US5: 用户在 Proxy tab 切换拦截模式、配置上游覆盖、确认活跃会话的 proxy 端口生效

## Implementation Decisions
见 docs/adr/observatory-panel.md（ADR-1..5）。模块 `app/src/ai/observatory/`（model.rs + view.rs）；挂载 util.rs/action.rs/view.rs/header_toolbar_item.rs。
- 数据面：blocks/raw 用 rusqlite 只读直查（harness_blocks.db / harness_raw_cache.db）；orchestration 用 store() + warp.sqlite 直查（messages/dispatches）
- 自动刷新：SpawnedFutureHandle+Timer 轮询（非事件推送），面板关闭时 gate
- GUI 交互 CC tab 拦截：GUI_INTERCEPT registry（terminal view id → session+settings tempfile）

## Testing Decisions
- model 单测：temp sqlite 下 SQL 正确性（sessions/blocks/raw/dispatches 同源查询）、状态转移（select*/toggle/set_draft/gate）
- e2e：`agent run --harness claude` 落库后面板数据源查询非空；GUI 交互路径核心机制（--settings 注入+proxy 捕获）由 acceptance 套件背书

## Acceptance
- A1 面板可从工具栏按钮打开/关闭（ADR-1）✓
- A2 sessions/blocks/raw 数据来自拦截落库快照（ADR-2）✓
- A3 orchestration 数据来自 orchestration store + dispatch 明细 ✓
- A4 send 闭环：composer → 子进程 → DB message → 刷新可见 ✓
- A5 全部状态变更经 model（无视图本地业务状态）✓
- A6 cargo test 相关套件全绿 + workspace check ✓

## Defer（原三项已全部实现）
- ~~block 详情大视图/搜索过滤~~ → 已实现（搜索+详情卡片）
- ~~dispatch/gate 操作~~ → 已实现（Dispatch 按钮+明细、gates resolve chips）
- ~~live 推送~~ → 已实现为 5s 轮询（panel_open gate）
- 剩余 defer：block 增量事件订阅式推送（真 live）、block 内容导出
