# UI 精简 Grill 决策记录 — 2026-08-30

> 方案：**C = 精简**（warpui 上砍）。底册：ui-inventory-2026-08-30.json（213 条）。裁决人：尤远。
> **协议（22:08 修订）**：单条 grill，每轮 5 条，逐条调研业务后裁决；此前批量裁决（R2/R3「全砍」）**作废**。

## 轮次
| 轮 | 内容 | 状态 |
|---|---|---|
| G1 | 死码前 5 条（本文件下方调研） | 待裁 |
| G2 | 死码后 3 条：wasm-nux-dialog / pane-code-diff壳 / settings-dais-drive | 待开 |
| G3+ | 引导 5 → 云残留 3 → 双实现与边缘 16（逐轮 5 条） | 待开 |
| G8+ | needs-review 22 条（自建 3 条已撤出） | 待开 |
| 末轮 | 独立依赖树 8 棵 | 待开 |

## 已定结论（重审 22:08）
- **自建编排核心 keep**：pane-cockpit / pane-observatory / pane-execution-profile-editor（撤出砍单）
- **清点误入 keep**：terminal-session-settings（模型非表面）/ root-search-bar（共享组件，4 处复用）
- R2/R3 批量裁决作废，逐条重烤

## G8 调研记录（00:2x 三批 grep/awk 实证：依赖树 5 棵）
| # | 依赖树 | 默认构建状态 | 关键证据 | 砍法 |
|---|---|---|---|---|
| 1 | tantivy 0.26 | **未编入**（use_tantivy_search 非 default） | Cargo.toml:303 optional；searcher.rs:1 整文件 cfg 门控(1313+行)；实验层 100% Experiment 但 feature 关着→默认构建实际走 fuzzy，矛盾死重；消费：command_palette navigation/new_session、command_search/projects | 删 dep+feature+searcher.rs，lib.rs:1034 硬编码 fuzzy |
| 2 | rquickjs+warp_js | **未编入**（仅 completions_v2→plugin_host，均非 default；default=classic_completions） | Cargo.toml:689 plugin_host=[dep:rquickjs,dep:warp_js]；433 completions_v2；全仓仅 completions_v2 一处拉 plugin_host（warp_packs/file_based_mcp 不依赖，已核）；消费：plugin/host/native/*、completer/js.rs、plugin/service/call_js_function.rs | 删 plugin_host/completions_v2 feature+两个 optional dep+JS 宿主文件树 |
| 3 | voice_input | **常驻**（gui=[voice_input]:621） | terminal/input.rs:2185 toggle_voice_input 麦克风入口；view/action.rs、use_agent_footer、alt_screen、block_list_element 织入；workspace crate | gui 列表去 voice_input+cfg 块全摡，workspace crate 移出 dais 构建 |
| 4 | aws 四件套 | aws-config 已 optional(:280 aws_sdk_config 默认关，stub 错误态)；但 credential-types/sdk-sts/types(:281-283) **常驻** | aws_credentials.rs 443行；消费：terminal/view.rs、terminal_manager、ai_page、agent_sdk；Bedrock BYO 链路 | 整族删+Bedrock 提供商分支下线（不用 Bedrock 则纯死重） |
| 5 | remote_server | **常驻**（workspace dep :236） | crates/remote_server 3961行；消费面 terminal session/executor、code/file_tree、global_buffer_model 全域；远程会话执行核心 | 砍=SSH 远程终端整体下线，高风险 |

**Default 列表死 feature 清单（已裁决待执行期清理）**：get_started_tab(G2)、hoa_onboarding_flow(G3)、directory_tab_colors(G5)、discard_per_file_and_all_changes+diff_set_as_context(G4-4)、code_launch_modal+open_warp_launch_modal(G1/G3)、shared_with_me(G4-1 连带)；另注 directory_tab_colors 尾部注释(拖3相关 runtime flag)随 G5-2 一并清。

**G8 裁决**：待用户

## G7 调研记录（23:4x 三批 grep/wc 实证）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | cs-notebooks-云源 | 245行 | notebooks_data_source.rs:28 "async data source for cloud notebooks"，ObjectStoreModel+SyncId；command_search 挂载 | cloud_object、search |
| 2 | cs-workflows-stored-云源 | 383行 | stored_workflows_data_source.rs:33 "cloud workflows (Dais Drive)"；⚠同目录 workflows_data_source.rs(118行)是本地 LocalWorkflows 源，保留 | cloud_object、search |
| 3 | warp-drive-聚合器 | 1741行 | data_source.rs:24 "searches against all Dais Drive objects"(1118)+fuzzy_match(142)+search items(3款)；消费：welcome_palette:241、terminal/prompts:38、zero_state/filter_chip | cloud_object、ObjectStoreModel |
| 4 | improved-palette-layer | 70行 | 实验 100% 全量(Control 0% vs Experiment 100%)；lib.rs:1034 门控；砍则硬编码 tantivy 分支；三 layer 全下线→experiments/ 框架(913行)整体终评 | experiments、tantivy |
| 5 | welcome_palette | 859行 | 唯一消费方 pane_group/pane/welcome_view.rs；G2-4 welcome 已裁→连带死；它同时是大 SearchBar 最后主要消费方(G4-2 注记) | search、warp_drive、大SearchBar |

连带确认：features/external_editor.rs(416行) 仅挂载在 features_page.rs:1286/2212，随 G4-3 整页砍连带死，util/file/external_editor 逻辑层（实际启动外部编辑器）不受影响；search/env_var_collections 环境变量集合源随云对象判定一并评估。

**G7 裁决（00:27 尤远：全砍）**：1 cs-notebooks-云源 **砍**；2 cs-workflows-stored-云源 **砍**（本地 LocalWorkflows 源不动）；3 warp-drive-聚合器 **砍（选项a全砍，面板不再搜对象）**（连带 welcome_palette:241、terminal/prompts:38 挂载点、zero_state/filter_chip 引用）；4 improved-palette-layer **砍**（lib.rs:1034 硬编码 tantivy 分支，experiments/ 框架 913行整体下线）；5 welcome_palette **砍**（连带 welcome_view 挂载，G2-4 已定）；external_editor 连带确认已记

## G6 调研记录（23:3x 两批 grep/wc/git 实证）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | mcp-installation-modal | 641行 | mcp_servers_page.rs:70/85-92 构造 Modal<InstallationModalBody>；gallery 本地化后恒空(inventory:78)，安装流仅剩本地模板；92c77b89 触达 | warpui、mcp_servers |
| 2 | mcp-update-modal | 499行 | list_page.rs:47/106/182-187 构造；服务 gallery/模板更新流，gallery 已本地化为空(inventory:79) | warpui、mcp_servers |
| 3 | notebook-embedding-search | 513行 | block_insertion_menu.rs:58/69 embedded_objects_enabled 门控懒创建；数据源 AcceptWorkflow/AcceptNotebook(SyncId) 云端 Drive 对象，云删后恒空；块插入菜单本体不受连带 | warpui、search mixer、SyncId |
| 4 | experiments-login-layer | 65行 | 纯流量桶配置(AuthFlowInstructions 25/25)；全仓无外部消费点(仅 experiments/mod.rs:13 注册)；登录流已大改 | experiments 框架 |
| 5 | experiments-block-onboarding-layer | 64行 | 同族(BlockOnboardingLayer)；无外部消费点(mod.rs:11 注册) | experiments 框架 |

附注：同目录 improved_palette_search_layer(70行) 留 G7 palette 轮一并裁；若三层全下线，experiments/ 框架(913行)可整体评估下线。

**G6 裁决（23:49 尤远：全砍）**：1 mcp-installation-modal **砍**（mcp_servers_page.rs 构造臂连带）；2 mcp-update-modal **砍**（list_page.rs 构造臂连带）；3 notebook-embedding-search **砍**（block_insertion_menu 嵌入分支+embedded_objects_enabled 门控连带，插入菜单本体不动）；4 experiments-login-layer **砍**；5 experiments-block-onboarding-layer **砍**（连带 mod.rs 注册行）；experiments/ 框架整体下线待 G7 裁完 improved_palette_search_layer 后终评

## G5 调研记录（23:2x 两批 grep/wc/git 溯源）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | sftp-breadcrumb ⚠自建 | 96行 | f8cb92d6(2026-06-02 LogicHe #201 Dais PR)；browser.rs:939/1768 消费+集成测试:379；逐段导航活组件 | warpui、sftp_manager |
| 2 | directory-color-add-picker | 309行 | appearance_page.rs:1595/3012 挂载；flag directory_tab_colors 默认开(Cargo.toml:592)；app_state/vertical_tabs/vtab_projection 联动 | warpui、FeatureFlag |
| 3 | node-version-popup | 602行 | display_chip.rs:725 构造（node 芯片点击弹窗）；context_chips 族单一用途；48d4c616 触达 | warpui、context_chips |
| 4 | external-secrets-menu | 608行 | env_vars/view/secrets.rs:28 + env_var_collection.rs:46 挂载；环境变量编辑器“外部密钥”选择器；底层 provider（1Password 等本地集成）存活性执行期核查 | warpui、env_vars、search |
| 5 | cs-ai-queries-source | 258行 | 0dbd3d56 Warp 上游原生；command_search/view.rs:50；AI 提问历史源(ai_history:)，与 history 源重叠变体 | search 框架 |

**G5 裁决（23:33 尤远）**：1 sftp-breadcrumb **砍（并入 browser.rs，选项a，功能不变仅内联）**；2 directory-color-add-picker **砍**（连 flag directory_tab_colors + appearance_page 段 + app_state/vertical_tabs/vtab_projection 联动，目录 tab 着色整体下线）；3 node-version-popup **留**；4 external-secrets-menu **砍**（连 env_vars 两处挂载点 + search/external_secrets 目录）；5 cs-ai-queries-source **砍**（command_search/view.rs:50 挂载点摘除）

## G4 调研记录（23:10-23:2x 四批 grep/wc/Cargo 实证）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | cloud-grab-edit-access-modal | 152行 | 抢占编辑权弹窗（云端共享 notebook "Edit anyway"）；挂 notebook.rs:44/235/401/681；门控 SharedWithMe && !can_edit()（:1125/:1639）；**服务端 grab 调用是 TODO（:47）从未实现**；云删+lease 本地化后不可达 | warpui、cloud_object |
| 2 | SearchBar 双实现 | 173+1044行 | 小件(search_bar.rs)4消费方：keybindings_page(迁出保留)、mcp_servers/list、settings/keybindings、ai facts rule；大件(search/search_bar.rs)服 palette 族：command_palette、ai_context_menu、welcome_palette(welcome 已砍→悬空)、search_results_menu(G1-2 已砍) | warpui、editor |
| 3 | settings-features | 7246行 | 实验开关堆呬页；ZapNewSettingsModes 反向分支 :2519/:2554；enable_legacy_ssh_wrapper 残留 4 处；8/18+8/22 两次扫荡 commit | warpui、FeatureFlag 体系 |
| 4 | code-review-header 双实现 | legacy 501+revamp 169行 | header 新旧两套并存；revamp 门控 GitOperationsInCodeReview；入口 code_review_view.rs:4288 render_header | warpui、FeatureFlag |
| 5 | notebook-omnibar+compact-dropdown | 546+301行耦合对 | omnibar=块类型转换工具条；CompactDropdown 唯一消费方就是 omnibar(:53)；挂 editor/view.rs+tests | warpui |

**G4 裁决（23:23+23:25 尤远）**：1 cloud-grab-edit-access-modal **砍**（notebook.rs 弹窗臂+SharedWithMe 触发点连带摘）；2 SearchBar **砍小件留大件**（search_bar.rs 173行删，4 消费页迁移：keybindings_page 随 G3-4 迁出一并处理、mcp_servers/list、settings/keybindings、ai facts rule）；3 settings-features **整页砍**（7246行删，flag 回默认值，设置入口路由+i18n key 连带）；4 code-review-header **砍 legacy 保 revamp**（连带清 DiscardPerFileAndAllChanges/DiffSetAsContext/FileAndDiffSetComments 废弃分支）；5 omnibar **留 + CompactDropdown 内联进 omnibar**（view_components 公共组件位释放）

## G3 调研记录（23:05-23:18 四批 grep 实证）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | dais-launch-modal ⚠自建 | 422行 | 一次性发布弹窗（OneTimeModalModel did_check_to_trigger 门控）：三卡 contribute/OAD/auto-model + visit-repo 链接 + NEW 徽章；挂 workspace/view.rs 5处+terminal/view.rs:11582+util.rs:171 | warpui、OneTimeModalModel |
| 2 | tips 功能巡礼浮层 | 730行(含测试) | welcome_tips_view 挂 view.rs:969/1569、WelcomeTipsViewState、telemetry WelcomeTipFeature、lib.rs:1622 init | warpui、telemetry |
| 3 | hoa-onboarding ⚠自建 | 1096行 | agent inbox 引导流两步（WelcomeBanner→TabConfig）；文案"Meet your new agent inbox"/"unified notification center"；私有键 HasCompletedHOAOnboarding 门控；挂 view.rs:2215+vertical_tabs/action/root_view 6文件；OneTimeModalModel:82 与 launch-modal 联动 | warpui、vertical_tabs、private_user_preferences |
| 4 | resource_center | 3548行 | Warp 上游资源中心：changelog流443+feature卡481+tips feed234+快捷键速查页584；入口与 AI 助手面板互斥(view.rs:13428)；ToggleKeybindingsPage 独立菜单入口(8831)；耦合 BannerState/TipsCompleted（terminal input/general_settings/global_resource_handles） | warpui、changelog model、BannerState |
| 5 | cs-warp-ai-source | warp_ai.rs:134区 | 命令面板 `#` 自然语言命令源；云删后仅剩本地"写回/打开Agent"两动作（inventory md:55） | search 框架、telemetry(OpenedWarpAISource) |

**G3 裁决（23:10 尤远）**：1 dais-launch-modal **砍**（自建但用户明裁）；2 tips **砍**；3 hoa-onboarding **砍**（vertical_tabs 本体保留，只砍引导流）；4 resource_center **砍主体+keybindings_page 迁出保留**（迁至独立入口，ToggleKeybindingsPage 菜单保留）；5 cs-warp-ai-source **砍**；连带：OneTimeModalModel 收缩（is_dais_launch_modal/is_hoa_onboarding 开关释放）、telemetry WelcomeTipFeature/OpenedWarpAISource 摘除

## G2 调研记录（22:17-22:25 四批 grep/git/wc 实证）
| # | 条目 | 规模 | 关键证据 | 依赖 |
|---|---|---|---|---|
| 1 | wasm-nux-dialog | 292行 | 挂载点 view.rs:23339 `#[cfg(wasm)]` 桌面不渲染，但字段/构造(1028-1030,2948,3164)桌面无条件编译；8/22 刚吃品牌扫荡 commit 33eb39ef | warpui、NativePreferenceSettings |
| 2 | pane-code-diff 壳 | 167+22=189行 | `CodeDiffPane::new` 零构造点；恢复臂 factory:455 Err("no longer supported")上游判死；view.rs:13562 查询恒空集无副作用；**View本体3190行inline在用→保留** | warpui |
| 3 | settings-dais-drive | 143行 | 侧栏无入口，仅深链 'DaisDrive'(FromStr mod.rs:298)+路由2373；OpenUrl 空串；ZapNewSettingsModes 再门控 | warpui/warp_core/settings |
| 4 | welcome 页 | 105薄壳+337View=442行 | factory:501恢复臂(WelcomeTab门控)；view.rs:3883/20430 新tab落点；app_state:185/launch_config:158 特判；sqlite 持久化；WelcomeView 无模块外引用 | warpui、welcome_tab flag |
| 5 | get-started | 100+396=496行+子件 | 固定快捷键 FTUX(mod.rs:189-194→AddGetStartedTab)；flag **默认开**(Cargo.toml default:502)；三入口: OpenRepo/CreateProject(prompt驱动)/CloneRepo；CreateProjectView+CloneRepoView 唯一宿主 | warpui、get_started_tab flag |

**G2 裁决（23:05 尤远）**：1、3、4、5 **砍**；2 **留**（code-review 壳——预留未来独立审阅tab，恢复臂/查询线一并保留）
- 4 welcome 砍→新tab/新窗口直落终端会话；持久化臂与 sqlite 序列化一并摘
- 5 get-started 砍→连带 CreateProjectView/CloneRepoView 子件+AddGetStartedTab 快捷键注册
- 1 wasm-nux-dialog：桌面构建剔除全部挂点+字段+构造点（view.rs 5处）；不出 web 版则零影响

## G1 调研记录（22:10 grep+git log 实证，全部 Warp 上游初始版 0dbd3d56 遗留）
| # | 条目 | 规模 | 调用方（复核） | 维护成本证据 |
|---|---|---|---|---|
| 1 | launch-modal-framework | 688行 | 零 `impl Slide for`；外部 LaunchModal 命中均为 DaisLaunchModal 同名异件 | 死着仍吃 3 次扫荡 commit：BYOP清理5/6、i18n 5/13、字号6/16 |
| 2 | search-results-menu | 347行 | 零外部引用（grep -l 空） | 吃 i18n 扫荡 commit 5/3 |
| 3 | warning-box | 226行 | 仅 view_components/mod.rs:19 挂载 | 吃字体适配 commit 5/24 |
| 4 | clickable-text-input | 183行 | 仅 view_components/mod.rs:7 挂载 | 首版后零 commit |
| 5 | word-block-editor | 456行 | 仅 lib.rs:106 声明 | 首版后零 commit |

**G1 裁决（22:17 尤远）**：1、2 **砍**（未提异议，按推荐执行）；3、4、5 **留**
- 3 warning-box：**转正**——留做重要通知通道（未来业务复用）
- 4 clickable-text-input：**转正**——留做标记文本切输入框相关业务（如标记文本后 BTW）
- 5 word-block-editor：**转正**——留做 ctx 事件流标签化显示（chip 流渲染复用，22:17:44 补充）
- 注：3/4/5 由死码转为「预留复用组件」，依赖锁定 warpui，后续依赖轮不动它们
