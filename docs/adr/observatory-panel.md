# ADR: observatory-panel — 拦截/编排/观测 GUI 观测台

Date: 2026-08-15
Status: Active
Iteration base: eb40c81d

## ADR-1: 挂载点 = 右侧面板（follow resource-center/AI-assistant 先例）
Decision: 观测台作为 right panel 第三种内容（`CurrentWorkspaceState.is_observatory_open`），互斥开合。
Alternatives: 独立 tab 视图（需要新 tab 类型注册，改动面大）；modal（无法常驻对照）。
Consequences: 复用 render_panels/right_panel_open 既有管线；HeaderToolbarItemKind 加按钮保证可达性。
Constrains: [T3]

## ADR-2: 数据访问 = 主线程直读 sqlite + CLI 子进程写
Decision: refresh() 直接打开 WAL sqlite（BlockStore 只读查询 + orchestration store 读 API）；写操作（send-message）spawn `current_exe orchestration ...` 子进程，完成后 timer 刷新回读。
Alternatives: 全部经 L2 socket RPC（GUI 内 runtime 分发复杂）；后台线程 + 消息回投（先不需要）。
Consequences: 单机数百 block 规模下主线程直读可接受（<10ms）；写闭环 = 子进程退出 → DB → refresh → view 更新。
Constrains: [T1]

## ADR-3: MVU = ObservatoryModel 单一数据源
Decision: 所有状态（快照、选中 session、tab、composer 草稿、busy/error）住 `ObservatoryModel`（warpui Entity + Event），视图纯渲染 + 派发意图；singleton 注册。
Alternatives: 视图本地状态（分裂数据流，违背 mvu 合理诉求）。
Consequences: 视图无自持业务状态；事件 `ObservatoryEvent::SnapshotUpdated` 驱动重绘。
Constrains: [T1, T2]

## ADR-4: 门控 = FeatureFlag::AgentHarness + cfg(feature="orchestration")
Decision: 面板可见性挂 AgentHarness（与 intercept UI 同门）；编排读取段 cfg(feature="orchestration")，关 feature 时编排 tab 显示占位。
Constrains: [T1, T2, T3]

## ADR-5: i18n 键 P1 阶段统一预置
Decision: 全部 ftl 键（zh-CN/en/ja 三文件）在并行实施前由主 agent 一次落盘，实施节点只引用不新增，规避同文件并发写。
Constrains: [T1, T2, T3]
