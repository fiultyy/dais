# dais Cockpit 设计文档

日期:2026-08-17 ｜ 分支:`fiultyy/hub-cockpit`(本 worktree)
目标:把 hub-tui(/home/yy/.orca/hub-tui,ratatui 外置 TUI)的"多 agent 驾驶舱"设计模式移植为 **dais 原生 pane**。本票产出 = 本设计文档 + P0 PoC 骨架(`app/src/ai/cockpit/` + `app/src/pane_group/pane/cockpit_pane.rs`),不要求全功能。

血源政策(FORK.md):cockpit 全部数据与操作进程内本地完成,零云/AI 账号功能、零外部 CLI 子进程调用。

---

## 0. 一页纸结论

hub-tui 作为外置进程,必须用**三路外置轮询**猜测 dais/orca 内部状态(orca CLI 子进程 1.5s/5s 节流轮询、last-status.json mtime 轮询、独立 SQLite WAL)。cockpit 作为 dais 进程内原生 pane,三路数据源全部**蒸发为函数调用**:

| hub-tui 数据源 | 轮询开销 | cockpit 等价物 | 开销 |
|---|---|---|---|
| `orca-ide terminal list --json` 子进程轮询 | 1.5s 发起 + 5s 硬节流(实际 5s 生效,见进度报告 §4-1) | `WorkspaceRegistry` → `Workspace.tabs` → `PaneGroup.terminal_pane_ids()` → `TerminalView` 直读 | 内存遍历,微秒级 |
| `last-status.json` mtime 轮询(1.5s) | 文件系统 stat + JSON parse | `CLIAgentSessionsModel` 单例(L1 检测 + L2 插件富化上下文),且带**事件推送**(`CLIAgentSessionsModelEvent`) | 零轮询(事件驱动)/快照直读 |
| 独立 SQLite `hub-tui.db`(WAL) | 启动 bootstrap + 运行时写 | 映射 dais 既有 persistence / 按需新增表(逐项建议见 §1.3) | 同进程连接 |

PoC(P0)已证明:卡片数据 100% 进程内直取,零 CLI 子进程调用(验证方式见 §4.1)。

---

## 1. 数据源映射(逐项)

### 1.1 终端清单(hub-tui: `orca-ide terminal list --json` → CLI 轮询整层蒸发)

dais 进程内取数路径(全部只读):

```
WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx)      // 单例:WindowId → ViewHandle<Workspace>
  → workspace.tabs: Vec<TabData> (pub(crate))           // 每个 tab 一个 pane group
    → tab.pane_group.as_ref(ctx): &PaneGroup
      → pane_group.terminal_pane_ids()                   // Iterator<PaneId>(is_terminal_pane 过滤)
        → pane_group.terminal_view_from_pane_id(id, ctx) // Option<ViewHandle<TerminalView>>,只读
          → tv.as_ref(ctx): &TerminalView
```

hub-tui 字段逐项映射:

| hub-tui 字段 | dais 进程内来源 | 备注 |
|---|---|---|
| `handle`(term_xxxx 8-hex) | `TerminalView::id()`(EntityId)+ 所在 `PaneId` | dais 无 orca handle;EntityId 是进程内稳定 key,卡片选中态用它 |
| `title` | `TerminalView::pane_configuration().as_ref(ctx).title()` | 与 tab 标题同一来源(terminal view 自己 `set_title`),agent 会话时自动换成 agent 标题 |
| `cwd` | `TerminalView::pwd()`(active_block_metadata,OSC 回报) | 零子进程 |
| `preview`(尾部输出) | TerminalModel block list / grid(`tv.model()` FairMutex) | **P1**:像素 UI 重新设计,见 §5.2 |
| `branch` | dais git 模型(`util/git.rs`)/BlockMetadata repo 信息 | **P1 接线** |
| `connected` / `writable` | `tv.is_shared_session_viewer()` / `tv.is_read_only()` | P1;hub-tui 里 writable 已入模型未展示,对等 |
| `lastOutputAt`(活跃度) | `tv.is_long_running()` / block list 活跃块 | P1 |

### 1.2 agent 结构化状态(hub-tui: last-status.json mtime 轮询 → 进程内单例 + 事件推送)

`CLIAgentSessionsModel`(`app/src/terminal/cli_agent_sessions/mod.rs`,SingletonEntity,按 `terminal_view_id: EntityId` 键控)是 hub-tui last-status.json 的**严格超集**,且 L1/L2 分层:

- **L1(命令检测,零插件)**:终端 spawn 命令识别出 CLIAgent(Claude/Gemini/Codex/Amp/Droid/OpenCode/Copilot/Pi/Auggie/Cursor/Antigravity/DeepSeek…),即建会话。→ hub-tui `source` 图标等价 `agent.display_name()`。
- **L2(插件富化)**:装了 Zap 插件的 CLI agent 上报 `CLIAgentEvent`,`apply_event` 归并进 `CLIAgentSessionContext`。

hub-tui last-status 字段映射:

| hub-tui 字段 | `CLIAgentSession` 等价物 | 事件触发 |
|---|---|---|
| `state` | `status: CLIAgentSessionStatus{InProgress, Success, Blocked{message}}` | `StatusChanged` |
| `prompt` | `session_context.query` | `PromptSubmit` |
| `lastAssistantMessage` | `session_context.response`(或 `summary` 回退) | `Stop` |
| `toolName` | `session_context.tool_name` | `PermissionRequest` |
| `toolInput` | `session_context.tool_input_preview` | `PermissionRequest` |
| `cwd`/`project`/`session_id` | `session_context.{cwd,project,session_id}` | 每事件刷新 |

关键差异:**hub-tui 轮询 mtime 是拉模型,cockpit 直接 `subscribe_to_model(CLIAgentSessionsModel)` 收 `{Started, StatusChanged, InputSessionChanged, Ended, SessionUpdated}` 推送** — 刷新延迟从 1.5s 降为事件粒度,且 `Blocked`(hub-tui 的 waiting/permission 状态)自带 message 文本。P0 用 1s 快照轮询兜底(实现简单、密度足够),P1 换事件化(§4.2)。

hub-tui 有而 L1/L2 目前没有的:`elapsed`(会话起止时间戳)、`model`(Codex/Devin 型号)。开放问题见 §5.4。

### 1.3 持久层(hub-tui: 独立 SQLite hub-tui.db → 逐项建议)

hub-tui.db 15 张表(config/groups/tags/snippets/alert_rules/macros/saved_views/notes/aliases/hotkeys/events/history/pinned/watch/templates)。dais 已有两条持久化通道:app-state SQLite(`app/src/persistence/sqlite.rs`, pane 恢复)与 harness_blocks.db(observatory 只读直查)。逐项建议:

| hub-tui 表 | 建议 | 理由 |
|---|---|---|
| `config`(轮询间隔等) | **不迁移** | dais 侧无轮询;UI 偏好走 dais settings 体系 |
| `groups`(终端分组) | **不迁移**,用 dais 既有概念 | dais 已有 tab 目录色(DirectoryTabColors)、vertical tabs 分组渲染;再造一层分组是双头真相 |
| `tags`(终端标签) | **不迁移** | 同上;卡片身份由 title+cwd+agent 已覆盖 hub-tui 设计目标(报告 §4-3 残留项本身低价值) |
| `snippets`(片段) | **映射 dais workflows/aliases** | dais 已有用户 workflow(可带参数模板)与 command alias;注入场景(P2)直接引用,不建新表 |
| `alert_rules` | **新增 dais persistence 表**(P2) | 基于进程内 `CLIAgentSessionStatus` 转移 hook,规则模型可沿用 hub-tui `AlertRuleType{State,Source,Severity,Message}`(对齐已实现行为而非 root spec 富模型) |
| `macros`(动作序列) | **新增表或复用 workflows**(P2) | 与 snippets 同源;若引入 $N 模板则建独立表更干净 |
| `saved_views`(筛选组合) | **不迁移** | dais 原生 UI 的筛选器状态用 settings 持久即可,价值密度低 |
| `notes`(终端备注) | 择机;P2 再议 | 若做,挂 dais 侧 per-terminal 元数据(pane 快照已有 TerminalPaneSnapshot 通道) |
| `aliases`/`hotkeys` | **不迁移** | dais 已有全局 keymap 体系(keybindings 设置页)与命令别名 |
| `events`(活动日志 5000 FIFO)/`history`(命令历史) | **不迁移** | dais 终端历史已有(history.rs/rich_history);agent 事件流 observatory 的 harness_blocks.db 已覆盖 |
| `pinned`/`watch`/`templates`/`messages` | **不迁移** | pinned 由选中态/置顶 UI 态承载;watch 依赖 inbox 已在 hub-tui 851c718 移除;messages 同 |

结论:**cockpit 默认零持久层**(P0/P1);P2 只为 alert_rules/macros 引入表,且落在 dais 既有 persistence 连接,不另开独立 SQLite 文件。

---

## 2. 视图映射(hub-tui 8 行卡片 → warpui View)

### 2.1 卡片

hub-tui `CARD_W=36 × CARD_H=8`(identity 1 行 / recap 5 行 / tool 1 行 / status 1 行)。dais 像素 UI 等价物是 warpui `ConstrainedBox`(定宽卡)× `Wrap::row()`(自动折行网格)× `Container`(圆角边框)。P0 卡片字段(全部进程内直取):

```
┌──────────────────────────────┐
│ ● Claude Code        #4821   │  ← 状态点(status 映射)+ agent.display_name / "Shell" + EntityId 低 16bit
│ repo-ai 优化中…(query 截断)   │  ← recap: query > summary 回退链(P1 加 response > preview)
│ 🔧 Edit                       │  ← tool_name(+tool_input_preview 截断,P1)
│ ~/work/repo · Blocked        │  ← pwd 截断 + status 文本
└──────────────────────────────┘
```

hub-tui recap 回退链 `last_assistant_msg > prompt > preview_tail` 的 cockpit 对应(P0 已实现前两级):`response > query > summary`,P1 补 preview 尾行(TerminalModel 直读)。

### 2.2 分组/筛选/排序/multi-select → Action 化接线

hub-tui 这些都是 update.rs reducer 里的大 match 臂。cockpit 全部收敛为 `CockpitPanelAction` typed action(warpui 六环接线,每个交互 = Action 定义 → dispatch → handler 注册 → 状态更新 → `ctx.notify()` → rerender):

| hub-tui 功能 | cockpit Action(P1) | model 状态 |
|---|---|---|
| 文本/状态/agent 类型筛选 | `SetFilter(String)` / `SetStatusFilter(Option<…>)` | `filter: String` |
| 排序(活跃度/标题/cwd) | `SetSortMode(CockpitSort)` | `sort: CockpitSort` |
| 单选(点击卡片) | `SelectCard(Option<EntityId>)` | `selected: Option<EntityId>`(P0 已实现,六环样板) |
| multi-select(批量注入) | `ToggleCardSelection(EntityId)` / `ClearSelection` | `selected_set: HashSet<EntityId>` |
| worktree 分组视图 | `SetGroupBy(CockpitGroupBy)` | `group_by`(按 cwd 首段聚合,替代 hub-tui worktree 分组) |

选中态清理(warpui 陷阱 #4):每次 refresh 快照后校验 `selected` 是否仍在卡片集内,失配即清 — P0 已实现。

### 2.3 pane 挂载

完全复刻 observatory 挂载范本(`app/src/pane_group/pane/observatory_pane.rs`):

- 业务态全在 `CockpitModel` 单例(快照/选中),pane 只是 view 壳(标题/焦点/关闭),**不持久化**(重启后从工具条重开)。
- 枚举接线点:`IPaneType::Cockpit`(Display/render/PaneId 构造)、`LeafContents::Cockpit`(is_persisted=false / restore Err / sqlite 不可达臂)、`HeaderToolbarItemKind::Cockpit`(工具条按钮,gate `FeatureFlag::AgentHarness` — dais bin 显式启用)、`WorkspaceAction::ToggleCockpit`、vertical tabs `TypedPane::Other` 分类。
- cfg 门控粒度对齐 observatory 实况:`LeafContents::Cockpit` 变体**无 cfg**(枚举进 launch_config/sqlite 的 match 模式,match 臂上放 cfg 属性非法),pane 模块/PaneId 构造/render 臂/Action 按钮按 `not(wasm)` 门控。

---

## 3. 交互映射(hub-tui PTY master 直写 → dais 注入通道)

hub-tui 对终端的一切消息/命令注入 = 打开 `/dev/pts/N` master 直写。dais 进程内已有两条成熟通道,**不需要也不应该再碰 PTY 设备文件**:

1. **裸 PTY 写**:`TerminalView::write_to_pty(bytes, ctx)`(pub(crate))— 等价 hub-tui 直写 master fd,走 dais 事件循环(含转义序列处理一致性)。
2. **agent 感知写**:`TerminalView::try_send_text_to_cli_agent_or_rich_input(text, ctx)`(pub)— 若该终端有活跃 CLI agent 且富输入打开,进 rich input composer(不炸 agent 的 TUI),否则落 PTY。**批量注入默认走这条**(hub-tui multi-inject 的正确移植)。

批量操作枚举:`PaneGroup::for_all_terminal_panes(cb)`(现成)或 cockpit model 持有的 EntityId → `ViewHandle<TerminalView>` 集合逐个 `update`。P1 实现"选中集批量注入";hub-tui 的 broadcast(group_broadcast)已在其 0270a53 清理中删除,不移植。

---

## 4. 分阶段里程碑

### 4.1 P0 = 骨架 pane(本票,已交付)

范围:`CockpitModel` 单例(快照/选中)+ `CockpitPanelView`(Refresh/SelectCard 两个 Action + 1s 自动刷新 timer)+ `CockpitPane` 挂载 + 工具条按钮。

**验收标准(全部满足)**:
- [x] 工具条按钮 → 打开 cockpit pane 为独立 tab;再次点击聚焦既有 pane
- [x] 卡片网格渲染存活终端:agent 名(或 Shell)/recap(query>summary 回退)/tool 行/pwd+status 行
- [x] **数据通路证明:卡片数据 100% 来自进程内直取**(`CockpitModel::refresh` 全链路 `WorkspaceRegistry`/`TerminalView`/`CLIAgentSessionsModel` 函数调用,grep 零 `Command::new`/`std::process` 于 cockpit 模块;无任何 orca CLI 调用)
- [x] 六环接线逐环可验证:Refresh 按钮 on_click → `dispatch_typed_action(CockpitPanelAction::Refresh)` → `on_action` handler → `model.refresh`(重建快照+选中态清理)→ `ctx.emit(CockpitEvent::SnapshotUpdated)` → view 订阅回调 `ctx.notify()` → rerender 读新快照;SelectCard 同构(卡片点击 → 选中态高亮变化)
- [x] `cargo build` 0 errors;既有测试不破坏(与基线对照)
- [x] 不引入云/AI 账号功能

### 4.2 P1 = 交互(选择/注入/筛选)

- 事件化刷新:订阅 `CLIAgentSessionsModelEvent`(StatusChanged/Started/Ended)替代 1s timer 兜底(timer 降级为低频对账);terminal 开关经 pane attach/detach 或 workspace 快照差分
- 点击卡片 → 跨 tab 聚焦对应 terminal pane(PaneViewLocator 复用 `focus_pane`)
- multi-select + 批量注入(`try_send_text_to_cli_agent_or_rich_input`);注入前确认对话框
- 筛选(文本/状态/agent 类型)+ 排序 + cwd 分组(§2.2 Action 表)
- recap 补全:`response > query > summary > preview_tail` 四级回退;preview 尾行接 TerminalModel
- branch/connected/writable 字段接入卡片

**验收**:对任一存活 CLI agent 终端,卡片状态变化(如 Blocked)在事件后 1 帧内反映;批量注入 3 终端生效且不破坏 agent TUI;筛选/排序/分组全部走 typed action 且状态可恢复。

### 4.3 P2 = 高级(alert rules/宏/片段)

- alert_rules:dais persistence 新表;hook 挂 `CLIAgentSessionStatus` 转移(进程内,无轮询);toast 复用 dais `ToastStack`(注意 hub-tui 报告 §4-6 的 toast 去重补缺教训:同 (severity,text) 短窗口去重)
- 宏/片段:映射 dais workflows(§1.3);注入模板 `$N` 参数化
- dashboard 指标(按状态计数、活跃 agent 数)与 sparkline

**验收**:alert 规则命中 → toast 一次且去重;片段从 workflows 列表选取注入;指标实时。

---

## 5. 风险与开放问题

1. **pane 注册机制约束**(已在本 PoC 走通,留给后续维护者):新增 pane 类型需同步改 7 处 exhaustive match — `IPaneType`(Display/render/PaneId 构造)、`LeafContents`(is_persisted/restore/sqlite×2/launch_config)、vertical_tabs `TypedPane`。漏一处 = 编译错误(exhaustive match 保护),风险可控但 diff 面广。`HeaderToolbarItemKind` 变体进用户 settings schema(chip 选择序列化),旧配置反序列化兼容(serde 默认忽略未知枚举值的行为需在升级说明中提示)。
2. **卡片数据密度重设计空间**:hub-tui 36×8 字符卡是 TUI 约束下的产物;像素 UI 下可做可折叠卡(hover 展开完整 tool_input)、右栏详情面板(点卡显示 terminal preview 全文+事件时间线)、或直接复用 vertical tabs 的信息密度模型。P0 刻意朴素(单行文本列),密度设计放 P1 做用户可见性验证后再定。preview 直读 TerminalModel 需注意 FairMutex 持有窗口(快照拷贝,不在锁内做渲染)。
3. **刷新模型权衡**:P0 的 1s 全量快照在终端数 <100 时成本可忽略(纯内存遍历);事件化(P1)引入订阅生命周期管理(pane 关闭时退订),需防 warpui 陷阱 #4(快照失配选中残留 — 已在 P0 处理)与 #5(render 中改状态 — refresh 只在 Action/timer 回调触发)。
4. **开放问题**:(a) `CLIAgentSession` 无 started_at 时间戳 → hub-tui 的 elapsed/spinner 无直接等价,需在 sessions model 补字段或由 cockpit 侧记首见时间(后者重启丢失);(b) branch 字段进程内取数路径未定(git 模型调用成本 vs 缓存);(c) wasm 目标下 pane 全链路被 cfg 掉 — dais 桌面版无影响,web 版 cockpit 是否需要另议;(d) 跨窗口多 Workspace 时卡片按窗口分组的 UI 形态(P1 决策)。

---

## 附:P0 落地文件清单

新增:`app/src/ai/cockpit/{mod.rs, model.rs, view.rs}`、`app/src/pane_group/pane/cockpit_pane.rs`
修改:`app/src/ai/mod.rs`、`app/src/pane_group/pane/mod.rs`、`app/src/pane_group/mod.rs`、`app/src/app_state.rs`、`app/src/persistence/sqlite.rs`、`app/src/launch_configs/launch_config.rs`、`app/src/workspace/{action.rs, view.rs, header_toolbar_item.rs, view/vertical_tabs.rs}`、i18n `app/i18n/{en,ja,zh-CN}/warp.ftl`
