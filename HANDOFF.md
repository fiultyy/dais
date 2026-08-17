# Session Handoff — Dais Fork dev/localize
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

---

# 外部捕获全程小节（2026-08-16）

> 分支 `main` @ `4fddeef9`，三票递进: `82e4ce58`(T1c+T2b) →
> `b0d0a85a`(T3) → `4fddeef9`(T5)。目标: 用户在手敲终端里跑的 harness
> （`claude`/`omp`/`pi`）也被 zap 旁观捕获 — 不改 harness 源码、裸命令
> 零改写。接手 agent 读本节 + 关键文件索引即可独立续作。

## 最终架构（T5 口径 = main 现状）

**别名是唯一入口（用户拍板）**，bootstrap 静默武装出三个 shell 函数:
```
cc-zap  = command claude --settings ~/.config/zap/cc-entry-settings.json
omp-zap = command omp --model zap/glm-5.2
pi-zap  = command pi   --model zap/glm-5.2
```
裸命令 `claude`/`omp`/`pi` 行为完全不变。

数据流（omp 为例，括号为本机实证值）:
```
omp-zap → omp 读 ~/.omp/agent/models.yml 的 provider `zap`
        (baseUrl http://127.0.0.1:8787/omp; apiKey "!jq -r
         .env.ANTHROPIC_AUTH_TOKEN ~/.claude/settings.json")
      → zap 入口 :8787 (明文 HTTP, 仅 loopback), 前缀 /omp 即 harness 标识
      → strip 前缀(保留 query) → 每请求解析出口:
        ~/.config/zap/omp-upstream.json (bigmodel /api/coding/paas/v4,
        response_format=openai)
      → auth 头(authorization/x-api-key)原样转发, 不剥不注 → 上游
旁路:  每请求 RawEvent → 常驻观测 session external-omp
        (state_dir/harness_blocks.db + harness_raw_cache.db) → 观测台
```

分层:
- **网络面** `crates/proxy_interceptor/src/entry.rs` — `EntryServer` 绑
  `127.0.0.1:8787` 明文，fallback handler 按最长前缀分流 `/cc` `/omp`
  `/pi`，复用 TLS 路径的 `proxy_handler` 转发核心。出口解析每请求调用
  （配置热更）: `/cc` 三级 = env `ZAP_UPSTREAM_BASE` > 用户
  `~/.claude/settings.json` 的 `env.ANTHROPIC_BASE_URL` > 官方默认;
  `/omp` `/pi` 共用 `UpstreamConfig::from_omp_config()`（读
  omp-upstream.json）。解析失败 → 502，未知前缀 → 404（不猜测目的地）。
- **数据面** `crates/harness_integration/src/entry_gateway.rs` —
  `EntryGateway` = EntryServer + 每前缀一条捕获通道（forwarder +
  `run_raw_processor`），归并到常驻 session `external-{cc,omp,pi}`
  （harness 串 `claude-code`/`omp`/`pi`）。**Spawn 懒发**: 注册只预留
  block，首个 RawEvent 才落库 — 零流量前缀在观测台不可见。`stop()`
  落 Exit(reason=stopped) + 端口释放；常驻无 idle reap。DB 路径由调用方
  注入（app 传观测台同一路径的两个 db，测试传临时文件）。
- **武装面** `app/src/ai/external_capture_rt.rs`（313 行）— zap 进程级
  单例 `GATEWAY`（专属单线程 tokio RT）: `ensure_gateway`（幂等，绑不上
  → Err 降级）/ `shutdown` / `entry_port` / `snapshot`；别名函数定义
  （单行投递安全，bash/zsh Posix 与 fish 两方言）+ heredoc 感知插入
  `insert_arming_into_script`。
- **接线** — `intercept_sessions.rs`: 开关 `external_capture_enabled`
  （默认开，持久化 `<state_dir>/intercept_config.json`），app 启动即起
  网关，toggle 即启停并发 `ExternalCaptureChanged` 事件;
  `pty_controller.rs::external_capture_arming_suffix_for`: 本地 pane
  首个 shell 的 bootstrap 时插别名，条件 = 本地 pane(`local_tty/
  terminal_manager.rs:369` 标记) + 非 subshell + 方言支持 + FeatureFlag
  `AgentHarness` + 开关开 + 入口在跑，任一不满足原样返回（绝不阻塞
  pane 启动）; 观测台 `observatory/{model,view}.rs`:
  `external_registrations` 快照行 + 开关 chip（i18n 键
  `observatory-external-capture-*`）。

## 演进与取舍（为什么长这样）

### T1c 骨架 + T2b 登记链路（82e4ce58，已被 T5 整体移除）
- `ExternalCaptureManager` 放 harness_integration（集成层定位:
  proxy_interceptor 与 hook/block 数据层之间的缝），headless 可测。
- T2b 形状: 每登记一个 uuid session + **专属 HookServer**（hook→session
  归属由构造解决，token 隔离）+ 专属 TLS proxy（共享 ProxyManager，
  CA 单次生成）+ raw 转发器 + Spawn（metadata `mode:"external"` —
  刻意不加 `InterceptMode` 变体，避免破坏 app 侧 exhaustive match）。
- 回收: 30min 无 RawEvent `reap_idle` / 显式 `stop_registration`，
  blocks 留库供观测台历史; 时钟可注入（with_clock）测试不 sleep。
- 测试: 双登记并行不串/seq 单调、hook token 归属隔离、注入时钟闲置
  回收、端口释放（当时 lib 20/20 + 集成 1/1）。
- **为何移除**: per-registration TLS proxy + hook server 是"劫持注入"
  路线的地基，T5 改透明管道后整层作废。考古勿复活:
  `git show 82e4ce58:crates/harness_integration/src/external_capture.rs`。

### T3 手敲路径武装（b0d0a85a，一半被 T5 重写）
保留至今的遗产:
- **heredoc 感知插入**: zsh/bash 的 bootstrap 是 `read ... << 'EOM'`
  结构，`EOM` 之后的字节会被 ZLE 当作用户命令回显执行（实测尾部追加
  = pane 里可见的一大串污染）; 插入点必须在 EOM 标记**之前**（函数定义
  随 `WARP_BOOTSTRAP_VAR` 一起 eval，零回显）。fish 走临时文件 source
  本不回显，尾部追加即可。
- **CC `--settings` 深覆盖实证**: CC 的 settings.json `env` 块优先级
  **压过进程 env** — 裸 `export ANTHROPIC_BASE_URL=...` 会被用户
  `~/.claude/settings.json` 静默覆盖（T3 三轮实证）。所以 cc-zap 必须
  走 `--settings` 文件深覆盖。T5 把 T3 的临时文件固化为静态文件
  `~/.config/zap/cc-entry-settings.json`: 用户 settings **全量透传合并**
  （env/permissions/hooks/模型映射等所有顶层键）+ 仅覆盖
  `ANTHROPIC_BASE_URL` → `http://127.0.0.1:8787/cc`; 每次武装前重生成
  （端口/用户配置变化即生效），写失败仅记日志、旧文件降级可用。
- **omp 源码级结论（T3）**: omp 的 baseUrl 硬编码在模型配置里，**无
  进程级/env 覆盖入口** — env 注入路线对 omp 无效。T3 当时保留 env
  形状等上游支持，T5 直接改走 **models.yml + `--model` 别名路线**:
  编排侧在 omp 自己的 models.yml 里登记 provider `zap`（baseUrl 指向
  入口 `/omp`），`omp-zap --model zap/glm-5.2` 命中该 provider。
- codex 查证（T3）: 本机 config.toml 无 base_url 压制，
  `OPENAI_BASE_URL` env 前缀有效。T5 未给 codex 前缀。

被 T5 移除的 T3 旧路径: pane(view)级登记 `by_view`、ExecuteCommand
嗅探 + `export` 前缀注入、claude wrapper 临时 settings 文件（NamedTempFile
保活）、60s tick + `tick_except` 武装豁免回收、view drop 反查
`stop_registration`。

### T5 单端口 + 别名（4fddeef9，现行）
- **单端口明文 vs TLS 反代**: 别名指向 `http://127.0.0.1:8787`
  （loopback），客户端明文连入口 → 无 MITM 就无需 CA/
  `NODE_EXTRA_CA_CERTS`。GUI 拦截路径（zap 自己 spawn 的 harness 走
  `ProxyServer` TLS 反代 + CA 注入）不变，两路平行。
- **auth 透明管道**: `handler.rs` 的 `SKIPPED_REQUEST_HEADERS` 从 5 个
  减到 3 个 — `authorization`/`x-api-key` 不再剥掉重注，原样透传;
  同时删除"从 `api_key_env` 读 env 重注 auth 头"的逻辑（该字段仍在
  `UpstreamConfig` schema 里但不再用于注入）。客户端凭据自带（omp/pi
  的 models 配置用 `!jq` 引用户 token），zap 只改目的地 + 旁观捕获。
- **前缀即 harness 标识**: 单端口无连接身份，用户拍板每前缀归并一个
  常驻 session，不再 per-registration。
- 集成测试 `tests/entry_gateway.rs`: 前缀分流正确、未知前缀 404、
  透明 auth（假上游断言收到的客户端凭据原样）、懒发归并
  （external-pi 零流量零块）、stop 落 Exit + 端口释放（连接拒绝断言）。

## 编排侧前置配置（zap 仓外，接手须知）

zap 代码只读不写下列文件（唯一例外: cc-entry-settings.json 由 zap
每次武装前重生成）:

| 文件 | 作用 | 本机实值（2026-08-16） |
|---|---|---|
| `~/.config/zap/omp-upstream.json` | `/omp` `/pi` 出口，每请求热读 | api_base `https://open.bigmodel.cn/api/coding/paas/v4`; api_key_env ANTHROPIC_API_KEY; response_format openai |
| `~/.omp/agent/models.yml` | omp 的 provider `zap`（编排侧写） | baseUrl `http://127.0.0.1:8787/omp`; apiKey `!jq -r .env.ANTHROPIC_AUTH_TOKEN ~/.claude/settings.json`; 模型 glm-5.2 / glm-5-turbo; api openai-completions |
| `~/.pi/agent/models.json` | pi（**独立项目**，自有配置体系）同上 | baseUrl `http://127.0.0.1:8787/pi`; 同形状 |
| `~/.config/zap/cc-entry-settings.json` | cc-zap 的 `--settings` 深覆盖 | 用户 settings 全量透传（env/hooks/permissions）+ BASE_URL 覆盖为 `/cc` |
| `~/.claude/settings.json` | `/cc` 出口解析源 + token 源 | env.ANTHROPIC_BASE_URL `https://open.bigmodel.cn/api/anthropic` |

敏感注意: cc-entry-settings.json 是用户 settings 的全量合并（含
ANTHROPIC_AUTH_TOKEN 明文与 hooks/permissions），属本机敏感文件，
截图/外发/贴 issue 时必须先脱敏。

## 已知边界 / 遗留

1. **端口硬编码**: `ENTRY_PORT=8787` 无配置口。被占 → 启动降级 warn
   （开关开着但入口不可用、快照空），toggle 可重试。zap 双实例并行会
   抢端口。
2. **武装时机 = bootstrap**: 别名只在 pane 首个 shell 的 bootstrap 时
   插入。开关后开/入口后起 → 已存在 pane 无别名，需开新 pane; 存量
   pane 不回溯武装（设计内）。
3. **武装面**: 仅本地 pane + 非 subshell + bash/zsh/fish。PowerShell
   （permanent bootstrap 共享文件 + 非验收环境）与远端 pane（入口是
   本机 loopback，远端 shell 里 127.0.0.1 指向错误主机）永不武装。
4. **外部通道无 hook 事件**: T2b 曾有 per-registration HookServer，
   T5 后外部 session 只有流量块（Spawn/请求/Exit），无 CC hooks
   生命周期事件。需要时是新增项（hook → 归并 session 的归属设计需
   重新解），不是回归。
5. **codex 无前缀**: T5 只做了 cc/omp/pi。加回 = `entry.rs` 的 spec
   扩一行 + 出口解析（openai 形状; T3 已查证 env 口当时有效，但现行
   口径下同样应走别名 + codex 侧模型配置）。
6. **`/omp` `/pi` 共用一份 omp-upstream.json**: 两前缀出口无法分化;
   需要分化时扩配置 schema（如 per-prefix 文件）。
7. **cc 模型映射靠透传**: cc-entry-settings 只覆盖 BASE_URL，模型映射
   （ANTHROPIC_DEFAULT_*_MODEL）全靠用户 settings env 透传 — 用户没配
   则 cc-zap 发官方模型名到中转出口，成败取决于出口兼容性。
8. **response_format 三态** anthropic/openai/generic 决定 raw 解析器
   分派; 本机 /omp /pi 出口为 openai（bigmodel coding 端点）。
9. T1c/T2b/T3 旧路径已删干净（ExternalCaptureManager / tick_except /
   pane 级登记），复活任何一块前先读上文"为何移除"。

## 关键文件索引
```
crates/proxy_interceptor/src/entry.rs            EntryServer 网络面(214 行)
crates/proxy_interceptor/src/handler.rs          proxy_handler 透明管道(auth 头透传)
crates/proxy_interceptor/src/upstream.rs         UpstreamConfig 三级解析 + from_omp_config
crates/harness_integration/src/entry_gateway.rs  EntryGateway 数据面(241 行)
app/src/ai/external_capture_rt.rs                网关单例 + 别名武装(313 行)
app/src/terminal/intercept_sessions.rs           开关持久化 + 网关启停
app/src/terminal/writeable_pty/pty_controller.rs bootstrap 武装调用点
app/src/terminal/local_tty/terminal_manager.rs   本地 pane 标记(:369)
app/src/ai/observatory/{model,view}.rs           external_registrations 行 + 开关 chip
crates/harness_integration/tests/entry_gateway.rs 端到端集成测试
```

## 验证命令
```bash
cargo test -p harness_integration --test entry_gateway   # 端到端(前缀/透明auth/懒发/stop)
cargo test -p proxy_interceptor                          # 入口单测
cargo test -p warp --features orchestration --lib external_capture  # rt 别名/合并单测
cargo check -p warp --features orchestration
```
手动冒烟: 开关开 → 新开 pane 敲 `omp-zap` 跑一句 → 观测台 Sessions
出现 external-omp 行且有请求块; `curl http://127.0.0.1:8787/nope` → 404。

---

# T8 session 按实例分离小节（2026-08-17）

> 现象: 相同 harness 不同 CLI 实例（两个独立 omp 进程）的流量在观测里
> 归并为同一个 `external-omp` session。T5 口径"每前缀一常驻 session"的
> 已知代价, 本节按 T8 修为**一实例一 session**。

## 实证（身份源评估, 全部本机真 CLI 抓包）

| CLI | UA | 原生实例头 | 结论 |
|---|---|---|---|
| claude 2.1.179 | `claude-cli/...` 无 pid | `x-claude-code-session-id`（会话粒度, `--resume` 跨进程同 id） | 不可用 |
| omp 17.3.4 | `Bun/1.3.14` | 无 | 不可用 |
| pi 0.84.1 | `OpenAI/JS` | 无 | 不可用 |

连接层: CC(undici) 同连接复用多请求, omp(Bun) 每请求新连接 — **每连接
分组会把单实例拆碎, 否决**。定案: **客户端标记** — zap 别名铸造一次性
实例标记（pid-hex16-epoch）, 经 CLI 自带的身份信道到网关:
- **CC**: `ANTHROPIC_CUSTOM_HEADERS='x-zap-instance: <tag>'`（进程 env,
  `Name: Value` 冒号格式; JSON 格式报 Invalid header name）。
- **omp**: models.yml provider `headers` 的 env 整串引用
  （`x-zap-instance: ZAP_INSTANCE_TAG`, `resolveConfigValue` env 优先）。
- **pi**: models.json provider `headers` 的模板引用
  （`"x-zap-instance": "${ZAP_INSTANCE_TAG}"`, pi 用 `${VAR}` 语法）。

别名函数体**调用时**铸标记(定义时铸死会让同一 shell 多次调用共享标记,
违反一实例一 session):
```
cc-zap(){ ANTHROPIC_CUSTOM_HEADERS="x-zap-instance: $(date +%s%N)-$$" command claude --settings ...; }
omp-zap(){ ZAP_INSTANCE_TAG="$(date +%s%N)-$$" command omp --model zap/glm-5.2 "$@"; }
```
CC 必须走进程 env 赋值前缀: settings env 块优先级压过进程 env(T3 实证),
而 `ANTHROPIC_CUSTOM_HEADERS` 只从进程 env 读。fish 方言用
`set -lx VAR (date +%s%N)-$fish_pid`(引号外命令拼接)。

## 实现

- `proxy_interceptor/src/handler.rs`: `SKIPPED_REQUEST_HEADERS` 3→4,
  `x-zap-instance` 转发前剥（zap 内部信号不进上游; auth 头仍原样透传,
  透明管道其余字节不动）。
- `harness_integration/src/entry_gateway.rs` 重写数据面: 每前缀
  `PrefixPlane`（默认 lane + 实例 lane 表 + 单任务串行 demux）。请求
  事件读标记建/取 lane（`external-omp-<tag>`）, 登记请求 id→lane;
  响应 chunk/done 经登记回路由。每 lane 独立 `SessionContext` + 专属
  `run_raw_processor`（seq 各自单调, Spawn 懒发语义不变）。无标记流量
  回落默认 `external-omp`（T5 行为, 零回归）; 标记校验
  `[A-Za-z0-9._-]{1,64}`, 非法/超限（lane 上限 64/前缀）回落默认。
  `stop()` 落所有活跃 lane 的 Exit。
- `app/src/ai/external_capture_rt.rs`: `mint_instance_tag()` 铸标记,
  `alias_defs` 三别名函数体携带 `ZAP_INSTANCE_TAG=<tag>` 前缀。
- 观测台快照行 `EntrySessionInfo` 形状不变, 只是行数 = 默认+活跃实例。

## 编排侧配置（本次已写入本机）

- `~/.omp/agent/models.yml` zap provider 增 `headers:
  {x-zap-instance: ZAP_INSTANCE_TAG}`。
- `~/.pi/agent/models.json` zap provider 增 `"headers":
  {"x-zap-instance": "${ZAP_INSTANCE_TAG}"}`。
- CC 侧零配置（别名 env 自带）。

## 验证

- 单测: `entry_gateway::marker_validation`; `external_capture_rt` 8/8
  （含 T8 标记铸造/共享/字母表钉, T4 别名钉更新为带标记前缀断言）。
- 集成: 新 `tests/entry_gateway_instances.rs` — 同前缀两标记+无标记
  → 三 session 各恰一 Spawn、请求互不串、**标记头不进上游**、快照三行
  、stop 各落 Exit。T4 e2e 与 entry_gateway.rs 原断言零改动全过（语义
  保持: 无标记 = 默认 session 恰一 Spawn; T4 体逐字节断言全保）。
- 真链冒烟（临时 example+临时 provider, 已清理）:
  1. 两个真实 omp 实例 + 一个真实 pi 实例（不同 shell）→ 各自独立
     session 各恰一 Spawn, 上游真实回包（标记已剥）。
  2. **同 shell 修正点验证**: 同一 bash（同 pid）连续两次 `omp-zap` +
    一次 `cc-zap` → `external-omp-<ns>-<pid>` × 2、`external-cc-<ns>-<pid>`
    × 1, 三 session 各恰一 Spawn（CC 侧上游 529 重试 11 次也全归并
    同一实例 session）— 证实标记是**调用时**生成, 非定义时铸死。

## 边界

- 运行中的旧 zap（未含 T8）会把 `x-zap-instance` 原样转发上游（无害,
  未知头忽略）; 重启 zap 后才生效剥离+键控。
- 实例 lane 随网关常驻（无 idle reap, 同 T5 口径）; lane 上限 64/前缀
  超限回落默认 session 并告警。
- 标记唯一性依赖 `date +%s%N`（GNU date, Linux 验收环境）+ `$$`/
  `$fish_pid`; 理论上同 ns 同 pid 碰撞需进程 pid 复用+纳秒重合,
  实践可忽略。

---

# T10 外部 session 上下文占用显示小节（2026-08-17）

> 现象: 观测台选中外部捕获 session（external-omp 等）后无上下文占用
> 显示 — T5-T8 只做了透明管道捕获, T6 把 openai 形 usage 解析进了块
> metadata, 但观测台 UI 从未消费。本节补读取/派生/UI 三环, 协议字节
> 零改动。

## 实证（缺哪环）

p0review 实例 DB（`~/.local/state/zap-p0review/harness_blocks.db`）
external-omp 块实测:
- T6 解析产物在位: response 块 metadata 带
  `{"model":"glm-5.3","source":"openai_response","usage":{"input_tokens":22221,"output_tokens":7}}`;
- 响应侧模型与请求侧不同（请求 `zap/glm-5.2`, 上游 bigmodel 上报
  `glm-5.3`）;
- 旧 anthropic 形残留块 usage 全 0（需跳过）;
- 窗口映射: omp `~/.omp/agent/models.yml` 声明 glm-5.2
  `contextWindow: 131072`; models.dev 上 glm-5.3 多 provider 窗口
  不一致（1048576 vs 1000000）→ 不可采信。

缺的三环: ① UI 无上下文数据结构 ② 无派生逻辑 ③ 无渲染。

## 实现（`app/src/ai/observatory/`）

- **context_usage.rs（新, ~450 行含测试）**:
  - `derive_session_context(conn, session_id, catalog)`: 只读 SQL 扫
    最近 200 个相关块（新→旧）, 取第一个 usage 非零的 response 块的
    `input+output` 为占用; model 取响应侧上游上报, 空则回落请求侧声明
    （zap/glm-5.2 裸名匹配 glm-5.2）。
  - 窗口分层映射: ① harness 自身模型配置（omp models.yml / pi
    models.json 的 `contextWindow`, 即该 harness UI 自己用的分母,
    zap 只读不写）② models.dev catalog（同名模型所有 provider 窗口
    **一致才采信**, 歧义/未命中=None）。harness 归类: session 前缀
    `external-{cc,omp,pi}` 优先, 回落块 harness_type（GUI 拦截路径
    的 omp/pi session 同样读各自配置; CC 无配置文件 → 只走 catalog）。
  - 与 app 自有会话 chat_stream `context_window_usage`（末轮
    prompt+completion/window）同语义 — 聊天形 API 每请求携带全史,
    末次响应即当前占用。
- **model.rs**: `ObservatorySnapshot.session_context: Option<SessionContextInfo>`
  ; `reload_selected_session_data()` 统一 blocks/raw/context 重载
  （select_session / set_search_filter / refresh_auto 5s 轮询三口收拢,
  无重复查询）; `models_dev_catalog()` 只读缓存辅助（cached() →
  load_from_disk() 同步兜底 → None, 不触发网络拉取）。
- **view.rs**: Blocks 侧栏标题下加一行
  `模型 · 上下文 used / window tok · pct%`; 窗口未知降级
  `模型 · 上下文 used tok`。i18n 键 `observatory-session-context[-unknown-window]`
  三语言（en/zh-CN/ja）。

## 红线遵守

- 透明管道（entry.rs/handler.rs/raw_processor/block_builder）零改动;
  T6 解析产物只读消费, 未回退。
- harness 配置文件（models.yml/models.json）只读。

## 验证

- 单测 7 新增全绿 + 观测台既有 27 全绿: 末次非零 usage 选取/响应侧
  model 优先、旧 0-usage 跳过+请求侧回落、omp yaml/pi json/catalog
  一致性（歧义 None）/未声明 None、harness 前缀+类型归类。
- 真实例（zap-p0review, WARP_DATA_PROFILE=p0review）:
  1. 重建 zap-oss 重启实例, entry gateway :8787 正常;
  2. 真链冒烟 `omp --model zap/glm-5.2 -p` → 实例 lane
     `external-omp-t10final-*` 落库, usage input=22505 output=55
     （T6/T8 链无回归）;
  3. 以真实 DB 复现派生（与 Rust 实现逐字段同逻辑）:
     `glm-5.3 · 21774 / 131072 tok · 17%`。
- 已知限制: worker 环境无法像素级验证 GUI（zap 无 AT-SPI 树、
  GNOME 截图 D-Bus 拒绝、无输入注入）; 渲染胶水（Container+Text）
  与相邻标题行同构, 数据链已实证。用户开观测台选中外部 session
  即见该行。

## 边界

- 窗口未知（zap/* 别名无 catalog 一致条目且 harness 配置缺失）只显
  tokens 不显百分比 — 诚实降级, 不猜窗口。
- catalog 依赖 models.dev 缓存（Providers 设置页打开时拉取）;
  未拉取时 omp/pi 走配置文件不受影响。
- occupancy 只含末轮 input+output, 不含缓存的 cache_read 分离量
  （openai usage 未细分, 上游不报）。
