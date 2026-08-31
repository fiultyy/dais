# UI 全界面清点标记 — 2026-08-30 (裁决已更新 2026-08-31)

> 底册: [ui-inventory-2026-08-30.json](./ui-inventory-2026-08-30.json) (213 条, 含 verdict/manual_note/coupling)
> 可视化: [ui-inventory-2026-08-30-tree.html](./ui-inventory-2026-08-30-tree.html)
> 仓库: dais-gui @ feat/gui-cockpit-nav (只读清点, 零代码修改)

## 最终裁决分布 (人工 212 票 + 4 项推断补判, 已合并)

| 裁决 | 数量 | 说明 |
|---|---|---|
| **keep** | **156** | 含 10 项人工翻案(codex→cut 等反转项见下) |
| **trim** | **7** | 结构保留本地化: workflow-view / pane-notebook / pane-ai-document / app-menu-bar / left-panel / rc 系 / code-review-header |
| **cut** | **50** | 原 8 + 人工 42; 云 Drive 全家 / 引导链 / 遗留 stub |
| 裁决来源 | manual 201 · manual-adjusted 10 · inferred 1 · base 1 | |

## 用户目标 → 判决一致性核验

**目标: 去中心化本地终端, 砍云/引导/遗留, notebook+workflow 本地可用(配置化)**

| 检查项 | 结论 |
|---|---|
| Drive 云叙事清除 | ✓ drive 全家 7 项 cut(panel/index/naming/import/workflow modal/breadcrumbs/toast) + left-panel Drive 臂随摘 |
| notebook 本地可用 | ✓ 留: pane-notebook(trim=剥离云)/file-notebook-view/富文本编辑器/omnibar/find-bar/image-viewer; 切云: embedding 搜索/cs-notebooks 源/grab-edit-access/drive-empty-trash |
| workflow 本地配置化 | ✓ 留: workflow-view(trim=去 ObjectStoreModel 改本地 YAML 目录)/pane-workflow/categories/info-box/cs-workflows 源(去云分支); 切: drive-workflow-modal/palette-warp-drive 源 |
| 引导/营销链 | ✓ onboarding-logic/hoa/experiments 两层/one-time-modal-model/dais-launch/tips/welcome-palette/pane-welcome/pane-get-started 全 cut; codex-modal cut(deeplink 入口) |
| 翻案(conservative→果断) | ✓ codex/cloud-grab/experiments×2/node-popup/one-time-modal/ai-fact/exec-profile/wasm-view/root-search-bar → cut; cockpit/observatory/右面板/agents-toolbar/workflows 弹层/env-var 命令框/external-secrets/omnibar/breadcrumb → keep |
| 修正的 4 处冲突 | ✓ warning-box keep(留作通知接线) / pane-header keep / ai-assistant-panel keep / pane-notebook+pane-ai-document keep+剥离云 |

**遗留架构债(非裁决, 落地时的拆分要求)**: `workspace/view.rs`(24.7k 行)过重 — 用户指令按层级/模块继续拆, 通用组件拆单文件。涉及 codex-modal/dais-launch-modal/onboarding-logic/wasm-view-extras 等 9 表面同文件剪辑时顺手执行。

## Top 砍除候选 (cut 50 条)

| id | 名称 | 类别 | 耦合 | 裁决说明 |
|---|---|---|---|---|
| `blocklist-suggested-rule-modal` | SuggestedRuleModal 建议规则弹窗 | modal | isolated | AI 建议规则编辑弹窗,与 SyncId/网络状态耦合,随 AI 建议功能取舍。 |
| `blocklist-suggested-workflow-modal` | SuggestedAgentModeWorkflowModal 建议工作流弹窗 | modal | isolated | AI 建议工作流保存/运行弹窗,与 Drive 同步耦合,随 AI 建议功能取舍。 |
| `blocklist-suggestion-chip-view` | SuggestionChipView 建议 chips | other | isolated | AI 建议入口 chip,与 Warp Drive SyncId 事件耦合,随建议功能取舍。 |
| `cloud-breadcrumbs` | cloud_object 面包屑数据模型 | other | isolated | Space/Folder→ContainingObject 适配层,Drive 导航依赖。 |
| `cloud-grab-edit-access-modal` | GrabEditAccessModal 抢占编辑权弹窗 | modal | isolated | 服务端 grab 调用仍是 TODO(:47),且 edit lease 已在本分支折叠为本地,弹窗近乎空壳。 |
| `cloud-toast-message` | StoredObjectToastMessage 云对象操作 toast 文案 | other | isolated | Drive 对象增删改 toast 文案唯一来源,体积极小。 |
| `codex-modal` | Codex接入弹窗(CodexModal) | modal | shared-file | 仅deeplink可达的第三方接入弹窗, 非主路径, 保留价值随Codex合作策略。 |
| `cs-ai-queries-source` | AI提问历史数据源(ai_history:) | palette | isolated | 与history源重叠的AI变体 |
| `cs-notebooks-source` | 笔记本数据源(notebooks:) | palette | isolated | 云端笔记本检索 |
| `cs-warp-ai-source` | 自然语言命令源(#, WarpAIDataSource) | palette | shared-file | 云端功能删除后仅剩本地写回/打开Agent两动作 |
| `dais-launch-modal` | Dais发布宣传弹窗(DaisLaunchModal) | modal | shared-file | 一次性营销弹窗, 生命周期短, flag名zap-purge清理时应一并退役。 |
| `directory-color-add-picker` | 目录标签色添加 Picker | picker | isolated | 功能依赖边缘的目录着色实验 flag，小众。 |
| `drive-empty-trash-dialog` | 回收站清空确认对话框 | modal | isolated | 随 drive-index 裁(用户 drive 全家 cut) |
| `drive-import-modal` | Drive 导入 Modal | modal | isolated | 本地→云对象导入唯一 UI |
| `drive-index` | Drive 主视图(对象网格/排序/右键菜单/回收站) | panel | isolated | 对象管理中枢,feature 门控均可解释 |
| `drive-object-naming-dialog` | Drive 新建/重命名对话框 | modal | isolated | 所有 Drive 对象命名必经 |
| `drive-panel` | Dais Drive 外层面板 | panel | isolated | Drive 入口薄壳,逻辑在 DriveIndex |
| `drive-workflow-modal` | Workflow 编辑器 Modal(Drive 内) | modal | isolated | workflow 创建/编辑主表单,与 workflow_view 场景不同(Drive vs pane) |
| `experiments-block-onboarding-layer` | BlockOnboardingLayer 实验分层 | other | isolated | 同 LoginLayer,纯实验桶配置,非 UI 表面。 |
| `experiments-login-layer` | LoginLayer 实验分层 | other | isolated | 流量分配配置非界面;去中心化分支登录流程已大改,实验桶意义存疑。 |
| `features-external-editor` | 外部编辑器选择子视图 | other | isolated | 双宿主互斥复用结构脆弱，随 ZapNewSettingsModes 裁决可收敛为单路径。 |
| `hoa-onboarding-flow` | HOA 新手引导流(HoaOnboardingFlow) | overlay | isolated | 一次性引导流, 内容宣讲垂直 tab 等功能, 生命周期与 dais_launch modal 同类。 |
| `launch-modal-framework` | 通用LaunchModal分步框架(LaunchModal<S:Slide>) | modal | isolated | 泛型框架零具体Slide实现, 是被DaisLaunchModal取代后的孤儿代码, 可整体删除。 |
| `mcp-installation-modal` | MCP 模板安装确认模态 | modal | isolated | gallery 本地化后恒空，安装流仅剩本地模板场景。 |
| `mcp-update-modal` | MCP 安装更新模态 | modal | isolated | 服务 gallery/模板更新流，gallery 已本地化为空。 |
| `node-version-popup` | Node版本弹窗(NodeVersionPopupView) | overlay | isolated | 单一用途弹窗,仅node芯片场景 |
| `notebook-embedding-search` | 笔记本嵌入对象搜索(EmbeddingSearchMenu) | picker | isolated | 仅笔记本编辑器深路径可用,依赖云端Drive对象 |
| `onboarding-logic` | agent引导教程逻辑(onboarding.rs) | other | shared-file | 引导教程的调度逻辑, 无UI本体, agent onboarding主路径可见。 |
| `one-time-modal-model` | 一次性弹窗调度模型(OneTimeModalModel) | other | isolated | 仅服务两个一次性引导弹窗, 弹窗若裁此模型随之收缩。 |
| `palette-launch-config-source` | 启动配置数据源(launch_configs:) | palette | isolated | 实验性ContextFlag门控 |
| `palette-warp-drive-source` | Drive对象数据源(notebooks:/workflows:/env_vars:/drive:/plans:) | palette | isolated | 强云端耦合,离线场景价值存疑 |
| `pane-ai-fact` | AI规则pane(AIFactPane/AIFactView) | tab | isolated | 规则管理页功能尚未默认放开, gate关闭时恢复直接报错。 |
| `pane-code-diff` | 代码diff pane(CodeDiffPane/CodeDiffView) | tab | isolated | pane包装与CodeReview快照已死(CodeDiffPane/CodeDiffPaneModel/LeafContents::Cod |
| `pane-execution-profile-editor` | 执行配置编辑pane(ExecutionProfileEditorPane/ExecutionProfileEditorView) | tab | isolated | transient编辑器, 持久化缺失是刻意设计, 存续随执行profile功能。 |
| `pane-get-started` | GetStarted新手引导pane(GetStartedPane/GetStartedView) | tab | isolated | 与Welcome/onboarding引导重叠的一次性新手页, gate关闭后不可达。 |
| `pane-welcome` | Welcome起始页pane(WelcomePane/WelcomeView) | tab | isolated | 新tab默认入口页, 与GetStarted功能重叠, gate关闭即退化为普通会话tab。 |
| `root-search-bar` | 独立搜索栏(search_bar::SearchBar) | other | isolated | 与调色板SearchBar同名易混,候选合并 |
| `search-results-menu` | 通用搜索结果菜单(SearchResultsMenuView) | menu | isolated | 泛型壳子无实例化点,功能与AIContextMenu重复 |
| `settings-cloud-sync` | Cloud Sync 设置页 | settings | isolated | 非残留死码：这是去中心化分支自带的 GitHub/Gitee 设置同步功能，侧栏活跃入口。 |
| `settings-dais-drive` | Dais Drive 设置页 | settings | isolated | 侧栏无入口、内容仅一个开关且 OpenUrl 为空串，深链价值存疑。 |
| `settings-features` | Features 设置页(实验开关页) | settings | isolated | 7246 行实验开关堆积页，大量分支服务已下线(ZapNewSettingsModes 反向分支)，适合按 flag 清理。 |
| `settings-file-footer` | settings.toml 侧栏 Footer(打开/报错警示) | infobar | isolated | flag 未推广则永久隐藏，属 Preview 通道半成品。 |
| `shared-session-inactivity-modal` | SharerInactivityModal 共享不活动倒计时弹窗 | modal | isolated | 共享会话防挂机确认,继续/停止分享出口清晰。 |
| `shared-session-participant-avatar` | ParticipantAvatarView 参与者头像 | other | isolated | 共享会话在场展示与角色管理入口,presence 数据来自服务器。 |
| `tips-view` | Welcome Tips 浮层 | overlay | isolated | 一次性新手引导，内容仍宣讲 Warp 特性集，本地分支可精简或砍。 |
| `view-components-clickable-text-input` | ClickableTextInput 点击编辑文本 | other | isolated | 模块私有且无任何调用点,死代码。 |
| `wasm-nux-dialog` | WasmNUXDialog wasm 跳转桌面引导 | modal | isolated | 仅 wasm 构建相关,桌面 fork 完全用不到。 |
| `wasm-view-extras` | WASM附加表面(wasm_view) | panel | shared-file | web版专属面板, 桌面构建完全裁除, 清点需单独标记平台。 |
| `welcome-palette` | 欢迎调色板(WelcomePalette) | palette | isolated | 新会话主入口,门控在数据源注册层 |
| `word-block-editor` | WordBlockEditorView 词汇 chip 输入器 | other | isolated | 无任何调用点(仅 lib.rs:106 挂模块),疑似历史表单遗留。 |

## trim 候选 (7 条 — 结构保留+本地化改造)

| id | 名称 | 耦合 | 裁决说明 |
|---|---|---|---|
| `app-menu-bar` | 原生应用菜单栏(app_menus) | isolated | 留,裁空菜单 |
| `code-review-header` | 代码审查头部(legacy+revamp 双实现) | isolated | 删legacy |
| `left-panel` | 左侧工具面板(LeftPanelView, 7子面板) | isolated | 留结构,砍空功能(可全空留框架) |
| `pane-ai-document` | AI文档pane(AIDocumentPane/AIDocumentView) | isolated | 留 pane 本地本体; 剥离云字段, 保留会话恢复时序耦合 |
| `pane-notebook` | Notebook工作流画布pane(NotebookPane/NotebookView) | isolated | 留 pane 本地本体; 剥离 cloud_object 同步/GrabEditAccess 云业务, 定位本地结构化文档 |
| `rc-main` | Resource Center 主页(右侧面板) | isolated | 注意留setting/cockpit/观测台 |
| `rc-sections` | Resource Center 内容装配(sections 数据源) | isolated | 留本地化内容装配的框架 |

## 人工翻案 keep(4 条 — 原判保守/死码复活)

| id | 名称 | 原判→新判 | 说明 |
|---|---|---|---|
| `ai-assistant-panel` | AI 助手面板(Warp AI) | keep-core→keep | AI 主入口面板, 与 transcript 共生 |
| `cs-workflows-source` | Workflow数据源(workflows:) | keep-core→keep | 保留搜索入口; WorkflowSource 去云端存储分支, 仅本地 workflows |
| `pane-header` | Pane通用头栏(PaneHeader) | keep-core→keep | pane 体系共享 chrome 头, 基础设施保留 |
| `view-components-warning-box` | WarningBox 警告盒 | cut→keep | 留作后续通知接线; 现为死码, 接线时对齐 HOA 通知体系 |

## 耦合查验(codebase-mem, 不变)

| 标记 | 数量 |
|---|---|
| isolated(裁切零波及) | 185 |
| shared-file(同文件协同) | 15 |
| hub(全局共享件) | 7 |
| cross-edge(跨域边, 裁切需摘对端线) | 6 |

cut 集中在 isolated/shared-file 类, 无 hub 被裁, 六个 cross-edge 当事者全部保留 ✓。shared-file 协同剪辑点: workspace/view.rs 群(codex/dais-launch/onboarding/wasm-view 4 项 cut 与 5 keep 表面同文件)、cs-warp-ai-source 与 command-search 同文件、mcp 2 modal 与 directory-color-picker 需从宿主页摘挂载线。

## 数据质量说明
- verdict=最终生效; manual_verdict 仅记与原判不同的人工改判; trim_verdict=清点原判(可追溯)。
- 未投项 root-view 视为 keep(窗口根壳, 不可裁)。
- 4 项推断补判(workflow-view/file-notebook-view/image-viewer-view/drive-empty-trash-dialog)依据用户明示目标(notebook/workflow 本地化), source=inferred 可审计。

---

## 实施记录 (2026-08-31 workflowz 多轮滚动编排)

**执行链**: W1 14 路并发勘察(9 域+5 枢纽, 55 actions/547 ref_sites) → W2 5 枢纽单 owner 摘线(view.rs/settings/mod.rs/pane factory/search data_sources/left_panel) → W3 9 域 owner 删文件+trim 本地化 → W4 编译收敛 6 轮(249 错→0, cargo check -p warp --all-targets 全绿) → W5 三透镜对抗审查(leak/overcut/integrity)9 findings → R10 修复(blocker: sqlite 升级兼容/notebook 误伤恢复/integration crate 编译)。

**终态**: 92 文件删除 / 100 文件修改 / `cargo check --all-targets` 0 error。
**审查后修复**: sqlite 老库升级兼容(legacy pane 跳过不炸 tab)、notebook 插入菜单文本块+init 快捷键恢复、integration crate launch_config 测试清理。
**留档(非阻塞)**: i18n 约 400 死键未清(三语)、4 处 minor 遥测死值 — 后续清理轮处理。


---

## 清理轮记录 (2026-05-31 第二轮 workflowz, 5 席位滚动)

**执行**: codebase-mem reindex(moderate) 定位 → 5 席位并发: ①dais_sync 退役(cloud_sync 2 模块+crates/dais_sync 整删, 全仓零引用) ②integration_testing 云残留清(cloud_object 目录删/notebook+workflow step 修签名) ③telemetry 死枚举(-87 行) ④i18n 死键(en -871/zh -901/ja -854) ⑤persistence 全清(drop_legacy_pane_tables 迁移+schema/model 三表删+sqlite legacy 分支摘)+drive 精准小裁(export_tests; folders/items/workflows/sharing 经图可达性裁定为保留面共享模型层)。

**主控直修**: telemetry RemoteServerBinaryCheck 拼接臂拆分; 迁移 up.sql 补 legacy 行 DELETE(防老库升级恢复期整 tab 丢失——reviewer major finding); schema.rs welcome_panes 死表声明删除。

**终态**: `cargo check -p warp --all-targets` 0 error; 累计 104 删除 / 115 修改。
**复审留档(minor)**: warp_ssh_manager sync_meta 孤儿层、restored_settings.sqlite-wal/librust_out.rmeta 草稿勿入库、pane_content_factory_tests is_persisted 锁定测试被删。workspace 全量 check 受 command-signatures-v2 build.rs node 版本环境问题阻断(与本轮无关的既有问题)。

---

## 终态快照 (2026-08-31 reindex 后)

- **索引**: codebase-mem moderate 重建, 114,076 节点 / 443,881 边(裁切前 116,906/460,882); dais_sync/cloud_sync 文件已从图中消失(抽验 0 命中)。
- **工作区**: 104 文件删除 / 115 文件修改(含两轮实施+清理), 未 commit。
- **编译**: `cargo check -p warp --all-targets` 复验 0 error。
- **图验证**: drive 残余保留面(items/workflows/sharing)在图中引用健康; folders 经席位现场复核保留。

## Cargo 死 feature flag 清理 (2026-08-31b)

**背景**: 上轮 cleanup_round 漏记 flag(仅被动删 welcome_tab/get_started_tab)。本轮全量清点 230 个 flag, `cfg(feature=)` 引用扫描+双确认。

**删除 12 个死 flag**(零 cfg 引用; grep 同名命中均为无关标识符如 quake_mode 窗口 id): command_predictor / ssh_enable_host_denylist_in_settings / grab_the_baton_editing / inline_ssh_banner / notebook_parameter / toggle_bootstrap_block / permanent_autosuggestion_hint / conversation_filter / quake_mode / rich_history / system_theme / agent_management_popup。

**并入 default 构建保留面 4 项**: `local_fs` / `local_tty`(桌面构建的文件/终端基座, build.rs 本按平台 cfg, 现 cargo 层显式)、`changelog`(resource_center 更新日志保留面)、`voice_input`(AI 设置页语音分支)。

**跳过(记账留决策)**: `completions_v2`(保留面依赖但拉 plugin_host/rquickjs/warp_js 重树+需 node18 构建环境)、`enable_crash_recovery`(build.rs cfg_alias 非 cargo feature)、`gui`(meta, voice_input 已直入)。

**连带**: app/Cargo.toml 删 dais_sync 依赖残留行(上轮删 crate 时漏了这行, 导致 cargo 无法解析 — 本次修复)。

**验证**: `cargo check -p warp --all-targets` 0 error(含新激活 cfg 分支)。230→220 flags, default 143→148。

### 补遗: completions_v2 并入 default (2026-08-31b 续)

环境修复验证: `corepack` 可用(yarn 4.0.1), `js/build` 产物一次构建成功(`.gitignore:15` 已忽略该目录, build.rs 另有 stale 兜底不阻断)。`--features completions_v2` 单独编译与并入 default 后 `--all-targets` 均 0 error。按"保留面功能默认构建"指令并入 default(代码编辑器补全 v2)。default 143→149。

## R3: drive 彻底砍云 + flag 浮面 (2026-08-31c)

**R3-1 flag 浮面**: 删 5 纯死声明(hoa_onboarding_flow/code_launch_modal/fast_dev/gui/welcome_tab); 5 项去门控(shared_with_me/open_warp_launch_modal/directory_tab_colors/discard_per_file_and_all_changes/diff_set_as_context — lib.rs FeatureFlag 无条件注册)。flags 210→205。

**R3-2 drive 模型层砍云(5 席位 S1-S5 + F1-F3 修复)**:
- 云同步引擎: resync_object/fetch_single_cloud_object/remove_team_objects/refresh_updated_objects 等 8 方法+spawned_futures 链删除
- DB: 8 张云表 drop(迁移 2026-08-31-010000_drop_cloud_object_tables): cloud_objects_refreshes/object_actions/object_metadata/object_permissions/team_members/team_settings/teams/current_user_information
- 整删: persistence/cloud_objects.rs(315行)/cloud_object_tests.rs/drive/sharing/style.rs
- 消费方: cs 数据源无条件注册+Personal 直查; UserWorkspaces 多空间迭代×2/WarpDriveSettings 云闸门/auth_state 死字段删; workflows/notebook 28 云分支删
- **记录不砍**: StoredObjectPermissions/Statuses(42 处 permissions().owner 模型行为依赖, 深砍需重构 StoredObject trait)/ObjectStoreModel/SyncId(keep 本体骨架)/WorkflowSource::Team 等枚举变体(序列化兼容)

**编译收敛**: 55→43→9→2→0 五轮(S2 过切 model.rs 误删 Project/UpsertAIQuery 等, F1 内联恢复+主控直修 import 链)。

**验收**: cargo check -p warp --all-targets **0 errors**; 云符号(resync/fetch/remove_team/Guest/云表访问)零残留; 工作区 108 删/138 改。

---

## 终态快照 (2026-08-31c R3 后 reindex)

- **索引**: codebase-mem moderate 重建, **113,803 节点 / 442,516 边**(演进: 116,906/460,882 初版 → 114,076/443,881 清理轮 → 113,803/442,516 R3 砍云)。cloud_objects/sharing-style 文件图中 0 命中, 删除完全反映。
- **工作区**: 108 删 / 138 改, 未 commit。
- **编译**: `cargo check -p warp --all-targets` 0 error(R3 后复验)。
- **flags**: 230→205, default 143→144(5 去门控项移出 default, 功能无条件编译)。
- **提交卫生**: commit 前须 unstage `librust_out.rmeta`、`*.sqlite-shm/wal`(测试残渣)、`.workflowz-scan-order.tmp`; 须 add 两个新迁移目录 `2026-08-31-000000_drop_legacy_pane_tables/`、`2026-08-31-010000_drop_cloud_object_tables/`(untracked)。

## R4: voice 砍 + fuzzy 删 + tantivy 转正 (2026-08-31d)

**voice_input 反转砍**(b 轮浮面决策反转): crates/voice_input/ 整 crate + app/src/voice/ + editor/view/voice.rs 删; 30+ 文件 cfg 织入拆(terminal input/view/action/use_agent_footer/alt_screen/block_list + editor + settings + blocklist 等); default/feature/dep 声明三级清。

**fuzzy 删 + tantivy 转正**: Fuzzy{Session,NewSession,Action}Searcher 三实现删; 4 个 data_source 硬编码 full_text(UseTantivySearch flag 分支结构与 flag 本体一并删); ImprovedPaletteSearch 100% 实验层删; searcher.rs 1316 行去 feature 门控无条件编译; tantivy 0.26 optional→正式依赖。索引源核实全本地(CommandBinding/SessionNavigationData/NewSessionOption, RAM 索引无云)。

**V3 席位过切修复**(主控从 HEAD 恢复): EditorAction 18 个非 voice 变体 + ViewSnapshot.view_id + EditorView.is_ai_input + with_input_editor_icons 前两参。

**验证**: cargo check -p warp --all-targets 0 errors (36→3→2→0); voice_input/fuzzy/UseTantivySearch 全仓 grep 零残留。features 205→204。


### 终态快照 (2026-08-31d R4 后 reindex)

- **索引**: moderate 重建, **113,635 节点 / 441,225 边**(演进: 116,906 → 114,076 → 113,803 → 113,635)。
- **图验证**: voice/tantivy 相关仅剩 `app/src/ai/voice/mod.rs`(7 行墓碑注释) — 已物理删+mod 声明卸载, check 复验 0 err。
- **工作区**: HEAD=bf0229b5(R3 已 commit), R4 改动 7 删/49 改未 commit。

## 悬空依赖核实 (2026-08-31e)

7 域 reviewer 并行核实 R1-R4 删除面 54 符号: 40 clean, 其余命中绝大多数良性(底册历史/迁移自身/同名活符号)。**4 类真悬空已修**:
1. `get_started_tab` default+声明 — R3-1 checkout 事故致重放遗漏(记账有实物无), 删
2. `FeatureFlag::{WelcomeTab,GetStartedTab,HOAOnboardingFlow}` 死枚举变体, 删
3. `crates/i18n` 三语孤儿 key -42(welcome-palette×2+wasm-nux×12 ×3 locale) — 上轮 i18n 清扫只扫 app/i18n 漏了 crates/i18n
4. AGENTS.md 过期模块行(wasm_nux_dialog.rs)

留档未动: specs/APP-3909/TECH.md 引用 FuzzyActionSearcher 旧址(过期规格指针非代码, 规格活跃性待用户定夺)。
验证: cargo check -p warp --all-targets 0 errors, 悬空符号复扫全零。
