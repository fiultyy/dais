# dais 编排器(Orchestration)对外能力清单

> 盘点基准: HEAD `08052015`(2026-08-22)。对比边界: `e6404d60`(binary zap-oss→dais 改名)。
> 验证方式: 全部条目逐一读当前代码 + git 历史核对,不凭记忆。
> 编排平面归属: `crates/ai/src/agent/orchestration/`(纯逻辑层)+ `app/src/ai/orchestration/`(GPUI 接线层)。

## 0. 一句话结论

**能力面自 zap 时代构建完成后零变化。** `git log e6404d60..HEAD` 对全部编排路径(`crates/warp_cli/src/orchestration.rs`、`app/src/ai/orchestration/`、`app/src/ai/agent_sdk/orchestration.rs`、`crates/ai/src/agent/orchestration/`)为空 —— 改名 commit 之后没有任何编排能力增删改。zap→dais 变化全部在**身份面**(二进制名/调用名/指针文案/数据目录),见 §5。

## 1. 调用形态速览

外部协调者(如 DSH maestro)通过 shell 调用 CLI:

```
dais orchestration <subcommand> [args]
```

- CLI 枚举: `crates/warp_cli/src/orchestration.rs:11`(`OrchestrationCommand`,26 个变体)
- 挂载: `crates/warp_cli/src/lib.rs:340`(`CliCommand::Orchestration`)
- 执行体: `app/src/ai/agent_sdk/orchestration.rs:52`(`execute_command`,CLI 与 GUI 转发共用同一份语义)
- 入口二进制: `app/src/bin/dais.rs`(zap 时代为 `bin/zap_oss.rs`,e6404d60 改名)

**执行路径(对调用者透明)**: CLI 进程先探测 GUI 是否存活(`~/.local/state/dais/dais-runtime.json` + `is_pid_alive`),活着则把整条命令 JSON 经 Unix socket 转发给 GUI 进程在 GPUI 主线程执行(L2 转发,`runtime_rpc.rs:462`);无 GUI/超时/stub 则退化为 CLI 进程直连 SQLite。两条路径输出逐字节一致(`agent_sdk/orchestration.rs:405` `try_socket_fast_path`)。

**不转发的两条**(始终在本进程执行/拒绝):
1. `check-messages --wait` —— waiter claim 生命周期必须与本次调用同寿(`agent_sdk/orchestration.rs:422`)
2. `check-messages` 拉取 `orchestrator` 邮箱且 GUI 存活 —— 直接拒绝(单消费者守卫,GUI 的 router 线程独占该邮箱,`agent_sdk/orchestration.rs:412-420`)

## 2. CLI 面: 27 个子命令全表

### 2.1 编排生命周期(run/task/dispatch,6 个,zap 期 df0c3679 引入)
| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `create-run` | `--objective <text>` | 创建 run | stdout: `run_<id>` | zap 期不变 | orchestration.rs:12 / agent_sdk:58 |
| `create-task` | `<run_id> <spec> [--dep <tid>]...` | 创建任务(DAG 依赖) | stderr(若晋级): `promoted <id> -> ready`;stdout: `task_<id>`。无依赖任务自动 pending→ready | zap 期不变 | agent_sdk:63-81 |
| `start-worker` | `<task_id> [--command <cmd>] [--session <session_sid>]` | 为任务建 dispatch(worker_dispatch + dispatch_context 联动)+ **自动绑 pane(D-04)**:`--session` 显式绑定该 session 的 pane(失败=错误);无 `--session` 时 best-effort 绑当前活跃 pane(失败仅 stderr 提示) | stdout: `ctx_<id>`(首行,兼容);绑定成功再加 `ctx_<id> assigned to session_<sid> (pane view <vid>)` | zap 期不变;`--command`(block 驱动结算)为 6f08df11 加;`--session` 自动绑定 + assignee 落库为 D-04 修 | agent_sdk / dispatch_assign.rs |
| `check-status` | `[--run-id <rid>]` | 列 runs 或某 run 的 tasks | 人类可读列表(60 字符 spec 预览) | zap 期不变 | agent_sdk:119-140 |
| `transition-worker` | `<dispatch_id> <state>` | worker 9 态机迁移 | `transitioned <id> -> <state>` | zap 期不变 | agent_sdk:142-152 |
| `promote-tasks` | `<run_id>` | deps 全 completed 的 pending → ready | 每行 `promoted <id> -> ready` 或 `no tasks promoted` | zap 期不变 | agent_sdk:154-165 |

状态字面量(strum,与 SQLite CHECK 约束一致,`types.rs`):
- TaskStatus: `pending/ready/dispatched/completed/failed/blocked`(types.rs:65)
- DispatchStatus: `pending/dispatched/completed/failed/circuit_broken/unknown_dispatch`(types.rs:86)
- WorkerDispatchState 9 态: `starting/ready/start_unknown/failed/succeeded/stopping/stop_unknown/stopped/abandoned`(types.rs:112)

### 2.2 生命周期驱动(4 个,6f08df11 引入)

| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `mark-ready` | `<dispatch_id> [--effects <json>]` | worker→ready, dispatch→dispatched, task→dispatched 三连 | `<id> ready (dispatch + task -> dispatched)` | zap 期不变 | agent_sdk:167-175 |
| `fail-dispatch` | `<dispatch_id> <error>` | 记失败并累加断路器计数 | `circuit_broken` 或 `failed` | zap 期不变 | agent_sdk:177-189 |
| `create-gate` | `<task_id> --question <q> --option <o>...` | 建决策门,任务被 block | `gate_<id>` | zap 期不变 | agent_sdk:191-201 |
| `resolve-gate` / `expire-gate` | `<gate_id> <resolution>` / `<gate_id>` | 解门(放行)/过期门(任务判 failed) | 确认行 | zap 期不变 | agent_sdk:203-215 |

### 2.3 消息(2 个)

| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `send-message` | `<run_id> <from> <to> --message-type <t> --subject <s> --body <b>` | 入队消息(9 种类型,见 §3.1) | `enqueued seq=<n>` | zap 期不变 | agent_sdk:103-117 |
| `check-messages` | `<handle> [--wait] [--timeout-ms <ms>] [--type <t>]...` | 拉取(权威消费路径) | 见 §3.3 | 92956114 引入;`--wait`/`--timeout-ms`/`--type` 语义由 de8de712 定型 | agent_sdk:319-392 |

### 2.4 终端交互(4 个,6f08df11 引入)

| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `inject-prompt` | `<dispatch_id> <text> [--force]` | 向 worker 终端注入 prompt: 括号粘贴帧(`ESC[200~…ESC[201~`)+ 500ms 后单独 `\r`(agent TUI 会吞同帧 CR)。粘贴前做 idle 检查(标题判定,§3.4);Working/Permission 或读不到标题时拒绝,除非 `--force`。ESC 字节替换为 `<ESC>` 防注入逃逸 | `injected N bytes into dispatch/session mailbox ...` | zap 期不变;`session_<sid>` 句柄支持 = 9956866f(跨 harness 直发) | dispatch_send.rs:29 / prompt_injection.rs:40-62 |
| `read-worker` | `<dispatch_id> [--lines N=40] [--after <cursor>]` | 读终端尾屏(渲染文本,64KB 上限)。`--after` 增量: 只回 cursor 之后的行 | stdout: 文本;stderr(仅 --after 时): `cursor: <total>`(机器可解析)。每次读同时归档 terminal_tail 到 DB | zap 期不变;`--after` 增量游标为 7f163b82 期能力 | agent_sdk:227-286 / terminal_tail.rs |
| `scan-wait-blocked` | `<dispatch_id>` | 扫描尾屏(2000 行/256KB)识别 7 类等待阻塞信号: codex-update/cwd/model-migration/hooks-review/trust-workspace/interactive-prompt + permission-prompt | 命中: reason id;未命中: `no wait-blocked signal` | zap 期不变 | prompt_injection.rs:236-297 / interactive.rs |
| `answer` | `<dispatch_id> [--text <t>] [--enter] [--interrupt]` | 交互应答: 文本→500ms→`\r` 或 `\x03`(Ctrl-C) | `action sent to <id>` | zap 期不变 | prompt_injection.rs:81-110 |

### 2.5 绑定(1 个,9ab132c9 引入)

| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `assign` | `<dispatch_id>` | 把 dispatch 绑到**当前活跃终端 pane**: 注册 ViewRegistry(dispatch→TerminalView)+ SessionDispatchMap(session→dispatch)+ push delivery 注册 + **assignee 落库**(D-04:`dispatch_contexts.assignee_handle=session_<sid>`, `assignee_pane_key=view_<vid>`;status 归状态机管,不动) | `<id> assigned to session_<sid> (pane view <vid>)` | zap 期不变;落库为 D-04 修 | dispatch_assign.rs |

DAG 编排用 `start-worker --session`(§2.1)而非 `assign`——"活跃 pane"是人最后聚焦的面板,多 worker DAG 下不是正确目标;按 session 绑定才是。

### 2.6 未暴露的已建能力

- `send_task_dispatch`(dispatch_send.rs:125): 由 task spec 构建派发前导词(build_dispatch_preamble,prompt_injection.rs:335)注入 worker。**无 CLI 入口,无生产调用方** —— 预建未接线。
- `deliveries` 表(migration `2026-08-13-000000`): 联邦投递的预留端口(4b87c0fb),当前消息投递合同不经过它。

### 2.7 管理三件套 + 实例回收(orch-caps-v2 + v2-fix-13,2026-08-23)

| 命令 | 签名 | 功能 | 输出 | 状态 | 锚点 |
|---|---|---|---|---|---|
| `project-add` | `<abs_path>` | 写 projects 表(存在校验+幂等);GUI 内经 ProjectManagementModel(DB+ProjectEvent,rail 事件驱动刷新);headless 直连 SQLite | `project added/exists: <path>` | 新增 | projects_cli.rs |
| `project-remove` | `<abs_path> [--force]` | 无 force: 有关联 tab 拒绝(报明细);--force: **连 tab 全回收**(中断+PTY shutdown→关 tab→session 邮箱自然 retire;竞态新到 tab 一并清扫;cwd 驻留进程 SIGTERM→SIGKILL 兜底)后删行,最后 ProjectEvent::Removed | `project removed: <path> (N tab(s) closed)` | 新增 | projects_cli.rs |
| `project-list` | — | 全部项目 TSV | `path\tadded\tlast_opened` 行 | 新增 | projects_cli.rs |
| `worktree-create` | `<project> <name> [--agent <cmd>] [--prompt <text>]` | git worktree add 兄弟目录 `<repo>-<name>`(新分支)+ 自动 project-add;**`--agent`/`--prompt` 一步到位 spawn(Orca 对齐,D-缺注入面修)**: GUI 侧建 tab → CLI 端解析 session → 注入 agent 启动行(回车)→ 6s 稳定窗 → 粘贴 prompt(force)。需 GUI 运行 | worktree 路径;带注入时再加 tab 行 + `session_<sid>` + 注入摘要 | 新增;`--agent/--prompt` 为 LB-002 修 | worktrees.rs / agent_sdk |
| `gc-runs` | `[--days N=7] [--dry-run]` | 回收**已完成**(无 pending/ready/dispatched/blocked 任务)且早于 N 天的 run 及全部子行(messages/deliveries/gates/contexts/workers/tasks),单事务(D-05) | 每行 `deleted <run_id>` / `would delete <run_id>`;无目标: `no runs ...` | 新增(D-05 修) | store.rs `gc_runs` |
| `worktree-list` | `[project]` | porcelain 包装;无参数=全部已注册 git 项目(按 git-dir 去重) | worktree 路径行 | 新增 | worktrees.rs |
| `worktree-remove` | `<path> [--force]` | 终端守卫同上;--force 同全回收语义;git 定位经 `.git` gitfile 回溯主仓(任意命令顺序幂等,票2b);再 `git worktree remove --force` + 项目行清理 | `worktree removed: ...` | 新增 | worktrees.rs |
| `new-terminal` | `<project> [--cwd <dir>]` | GUI 动作(L2 转发):switch_project+建 tab(与 GUI 新建同路径);CLI 端轮询 L1 `latest-session`(shell_event_bridge 注册邮箱的权威打点)12s 窗口;**harness 启动交给调用方**(别名已武装在每个新 shell bootstrap,注入即可) | stdout: `session_<sid>`;超时 stderr 指引查 GUI log | 新增(--alias 已撤,v2-fix-13 票2) | new_terminal.rs |
| `close-terminal` | `<session_sid> [--force]` | 关单个实例 tab;--force 先 Ctrl-C+PTY shutdown;邮箱经 shell exit 自然 retire | `closed <sid> (tab#N)` | 新增(票3) | new_terminal.rs |

**别名透明代理(alias-transparent)**: `omp-dais` ≡ `omp` + zhipu-coding-plan 流量过 8787;别名体只带 env `ZHIPU_CODING_PLAN_BASE_URL`(omp 上游 catalog 按 MOONSHOT_BASE_URL 同款约定),零 `--model` 篡改;models.yml 对内置 provider 传输覆盖(凭据+x-dais-instance 头)。pi 无 env 面(baseUrl 不插值 `${VAR}`),env 暂 no-op 待上游跟进(external_capture_rt.rs 模块 doc)。

### 2.8 错误契约(D-03,2026-08-24 定型,跨重建守恒)

所有 `dais orchestration` 子命令统一错误形制:

| 通道 | 成功 | 失败 |
|---|---|---|
| 本地直连(无 GUI,直接落库) | 结果 → **stdout**,exit 0 | 错误链(`{err:#}`)→ **stderr**,exit **1** |
| L2 转发(GUI 在跑,经 runtime RPC) | GUI 捕获的 stdout 原样回放 → stdout,exit 0 | 错误信封 `{"error","executed":true}` → **stderr**,exit **1** |
| 拒绝(如 GUI 在跑时拉 `orchestrator` 邮箱) | — | 拒绝原因 → stderr,exit **1** |

修复前的漂移:转发道把 GUI 执行错误当成功回放(exit 0 + `{"error":...}` 打到 stdout)——消费侧按 exit code 判错会漏。判定器:`classify_forwarded_response`(顶层无 `output` 键而有 `error` 键 = 失败;成功信封恒带 `output`)。`~/.local/bin/dais-build` 的构建报告含 read-worker 错误形制快照,与本节互为基线。半成功面(worktree 建好但 tab/inject 失败)如实 exit 1,错误信息带已完成的副作用说明。

## 3. 消息面: session mailbox 机制

### 3.1 消息类型与生命周期接线

9 种 MessageType(types.rs:14): `status/dispatch/worker_done/merge_ready/escalation/handoff/decision_gate/question/heartbeat`。

**只有 2 种有生命周期副作用**(messaging.rs:16-35,router 消费时触发 reconciliation):

- `worker_done`: payload 必须是 JSON 对象 `{"task_id": "...", "dispatch_id": "...", "outcome": "succeeded|failed"}`,缺字段/非法 outcome 以结构化 `Rejected` 拒绝(reconciliation.rs:56-95)。成功路径 settle 任务+dispatch,产出审计 JSON(provenance: worker_report)。
- `heartbeat`: payload `{"dispatch_id": "..."}`,记录 dispatch 存活时间戳(reconciliation.rs:137-163)。

其余 7 种: 仅存储审计/UI 可见,路由返回 `Ignored`。

### 3.2 邮箱句柄(handle)三种形态

| 形态 | 格式 | 注册时机 | 锚点 |
|---|---|---|---|
| dispatch 邮箱 | `ctx_<id>`(StartWorker 输出) | `assign` 命令显式绑定到活跃 pane | dispatch_assign.rs:54 |
| **session 邮箱** | `session_<SessionId(u64)>` | **自动**: 每个 shell bootstrap 时 ShellEventBridge 自动注册(shell 退出时注销,dispatch 随之 retire) | shell_event_bridge.rs:110-134, 223-242 |
| orchestrator 邮箱 | `orchestrator` | GUI 启动时 router 线程持有,独占消费 | lib.rs:1213 / agent_sdk:412 |

session 邮箱自动注册(d2d9b0f5)意味着: **dais 里任何终端(哪怕不是编排 worker)都天然拥有一个可寻址邮箱** —— 跨 harness 直发(inject-prompt 到 `session_<sid>` / send-message 到该句柄)对任意 pane 可用,无需先建 run/task。

### 3.3 推(push)/拉(pull)双通道

**pull = 权威路径**(`check-messages`):
- 无 `--wait`: 单次 drain,打印后 mark read。
- `--wait`(默认 timeout 120s): 注册 waiter claim(TTL 15s,每 500ms 轮询刷新;claim 带 type filter,`[]`=全部);命中即返;超时**不是错误** —— stderr 打 `timed_out` 后仍做一次同 filter 终读再返回(Orca waitForMessage 语义,agent_sdk:331-366)。
- `--type` 过滤是**客户端侧**的: 不匹配的行保持 unread(留待后续拉取)。
- 输出格式: `--- seq <n> from <handle> [<type>] <subject> ---\n<body>`;无消息: `no unread messages for <handle>`。
- waiter claim 死进程自愈: TTL 过期即失效,无需清理协议。

**push = 加速器**(仅 GUI 进程运行,delivery.rs):
- router 线程每 500ms 轮询注册的 dispatch(空转 3 次退避 2s,router.rs:27-29)。
- **idle 边沿检测**多信号: 终端标题(agent 状态判定,§3.4)+ alt_screen + 输出静默时长 + Precmd 新近度;Unknown 永不触发(保守,delivery.rs:95-110)。
- 空闲且有待投消息 → 向 PTY 注入**纯指针**(消息体永不全推): 
  `\nYou have N orchestration message(s). Run \`dais orchestration check-messages <handle>\`.\n` + 500ms 后单独 `\r`(delivery.rs:49)。
- **push/pull 互斥**: 有活跃 waiter claim 的类型跳过 push(防阻塞等待与指针推送双消费同一行,mod.rs:123-127 / de8de712)。
- 失败安全: 只有指针成功写入 PTY 才置 `delivered_at`;内存水位防重复推送;重绑定时清零(reborn PTY 绝不收 stale Enter)。

### 3.4 agent 状态判定(标题 OSC 0/1/2,prompt_injection.rs:167-227)

Idle/Working/Permission 三态: gemini 显式标记、claude `✳ claude` 前缀、spinner 字形(braille/半月)、`* `/`. ` 前缀、边界感知 idle/working 关键词、agent 名片段(claude/codex/gemini/grok/omp/pi/droid/hermes/agy/cursor)归因;无信号返回 None(视为可注入)。

### 3.5 事件入口(GUI 内,非 CLI 但属于编排闭环)

- **OSC 133 shell 事件桥**(shell_event_bridge.rs): Precmd/Preexec/块完成 → DcsHookEvent → worker 状态迁移;shell 退出 → 邮箱/dispatch 注销 + retire。
- **block 驱动结算**(block_settle.rs): `start-worker --command <cmd>` 后,shell 块以**精确匹配**(trim 后全等,可存 wrapper 形式)该命令结束时,自动 enqueue `worker_done`(outcome 按 exit code: 0=succeeded,非 0=failed)—— 无需协调者轮询任务完成。

## 4. 其他对外面

### 4.1 runtime RPC socket(L1/L2)

- 元数据: `~/.local/state/dais/dais-runtime.json`,含 `{socket_path, pid, mode: app|serve}`;CLI 以 `is_pid_alive` 判活(runtime_rpc.rs)。过渡期读侧回退旧名 `zap-runtime.json` 一次(见 §5.3)。
- socket: `~/.local/state/dais/dais-runtime-<pid>.sock`,NDJSON 单请求单响应。
- L1 方法: `status`/`echo`/`latest-session`(new-terminal 的 bootstrap 探测,注册点打点)真实;`send-message`/`check-messages`/`check-status` 返回 fallback stub(runtime_rpc.rs:283-296)。
- L2 方法: `orchestration` —— 整条 `OrchestrationCommand` JSON 反序列化后在 GPUI 主线程经**同一份 execute_command** 执行,stdout 捕获回传 `{"output": "..."}`(runtime_rpc.rs:462 / RpcDispatcher:689)。
- GUI 进程独占路由/push 线程(单写者原则);CLI 进程永不启动 router(lib.rs:1170)。

### 4.2 `dais serve` 无头模式(bin/dais.rs:83)

`dais serve` 启动轻量无头 RPC 服务器(无 GPUI): 写 runtime metadata + 监听 socket,直接 DB 访问处理转发命令,Ctrl-C 退出。用途: 无桌面环境下的编排端点。当前生产形态是 GUI 常驻(`mode: app`),serve 是备用形态。

### 4.3 DB 直读面(外部只读)

`~/.local/state/dais/warp.sqlite`(WAL + busy_timeout=2000,多进程安全)。编排表(migration `2026-08-13-*`): `runs/tasks/dispatch_contexts/worker_dispatches/messages/deliveries/decision_gates/worker_terminal_archives/orchestration_waiters`。外部进程可安全只读查询(观测台也这么做,observatory/model.rs:1064 直查)。

注意历史: 上游 warp 时代的 `orchestration_events/orchestration_messages` 表(2026-02-28 migration)已于 2026-03-23 移除(`2026-03-23-180000_remove_orchestration_persistence`)—— 与当前本地编排平面无关,勿混淆。

### 4.4 归档面

`read-worker` 每次调用自动写 `worker_terminal_archives`(kind: `terminal_tail`,JSON 结构 TerminalTailContent,64KB 截断标记;`transcript_pin` 类型已定义未使用)(output.rs:36 / agent_sdk:262-277)。外部可经 DB 直读消费 worker 输出快照。

### 4.5 GUI 面(非 CLI,同一平面的图形消费者)

观测台(Observatory)编排 tab: runs/tasks/messages 只读 + 决策门 `resolve`(observatory/model.rs:563/598)。cockpit(编排器面板)是终端仪表盘,不属编排消息面。

## 5. zap → dais 能力变化

### 5.1 能力面: 零变化

改名点(`e6404d60`)前后,CLI 枚举 18 变体、参数、语义、消息面、RPC 协议**逐字节一致**(`git log e6404d60..HEAD -- <全部编排路径>` 为空)。

zap 时代内部演进(全部早于改名,供追溯):

| 时点 | commit | 增量 |
|---|---|---|
| 2026-08-13 | `8bf3b51d` | 从 Orca TS 移植编排平面(P0 骨架: 6 表+14 枚举+store trait+9 态机) |
| | `df0c3679` | CLI 挂载,首版 6 命令(create-run/create-task/start-worker/send-message/check-status/transition-worker) |
| | `6f08df11` | +10 命令(promote/mark-ready/fail/gate×3/inject/read/scan/answer),闭合业务环 |
| | `9ab132c9` | +assign(dispatch→pane 绑定) |
| | `92956114` | +check-messages(邮箱推拉) |
| | `de8de712` | waiter/push 互斥 + `--wait` 语义定型 + PTY 退出 retire |
| | `d2d9b0f5` | session 邮箱自动注册(跨 harness 直发 e2e) |
| | `a47e3c81` | L2 全命令转发(CLI 枚举加 Serialize/Deserialize) |
| | `44b91713` | orchestrator 邮箱单消费者守卫 |
| | `9956866f` | inject-prompt 接受 `session_*` 句柄 |
| | `7f163b82`/`34855fbd` | 结构优势总结 + idle 信号接 shell 时序 |

### 5.2 身份面变化(改名 commit 族)

| 项 | zap 时代 | dais 现在 | commit |
|---|---|---|---|
| 二进制源文件 | `app/src/bin/zap_oss.rs` | `app/src/bin/dais.rs` | e6404d60 |
| CLI 命令名(Oss channel) | `zap-oss` | `dais` | e6404d60(channel/mod.rs:38) |
| 推送指针文案 | `` `zap orchestration check-messages` `` | `` `dais orchestration check-messages` `` | e6404d60(delivery.rs:49) |
| app 身份(Wayland/D-Bus) | `dev.zap.Zap` | `dev.dais.Dais` | 65c6d5fc |
| 数据根目录 | `~/.local/state/zap` | `~/.local/state/dais` | 36b0f27a(D5) |
| 终端别名族(T9 intercept) | `cc/omp/pi-zap` | `cc/omp/pi-dais` 等 | c502c979 |
| 配置目录 | `~/.config/zap` | `~/.config/dais` | 36b0f27a |

### 5.3 残留清理(zap-purge,2026-08-22)

runtime RPC 身份面已收尾(本 commit):

| 项 | 旧(zap 时代) | 新契约 | 过渡 |
|---|---|---|---|
| 元数据文件 | `zap-runtime.json` | **`dais-runtime.json`** | 读侧新名优先、缺失回退旧名一次(覆盖旧版 GUI 仍在跑的窗口);写侧只写新名并顺手删除旧名残留文件 |
| socket | `zap-runtime-<pid>.sock` | `dais-runtime-<pid>.sock` | 无需过渡(路径经 metadata 间接获得) |
| RPC 线程名 | `zap-rpc-server` | `dais-rpc-server` | — |

**外部协调者探测契约**: 元数据文件名为 `dais-runtime.json`(位于 `~/.local/state/dais/`);旧名文件在首个新版进程写 metadata 时被删除,不再产生。

## 6. 对外部协调者的使用建议

**稳定面(可依赖,契约级)**:
1. 18 个 CLI 子命令签名与输出格式 —— zap→dais 零漂移,是最稳的接口层。
2. `check-messages <handle> --wait --timeout-ms --type` 语义: 超时非错误 + 终读兜底 + claim TTL 自愈 —— 长轮询安全。
3. session 邮箱 `session_<sid>` 自动注册/退出注销 + `inject-prompt` 直发 —— 跨 harness 驱动任意终端不需要编排实体(run/task)。
4. push 只推指针、消息体永不全推;pull 是唯一权威消费 —— 不会因 GUI 重启丢消息(pending = `read=0 AND delivered_at IS NULL`,SQLite 持久)。
5. L2 转发对调用者透明: 同一命令在"GUI 活/不活"两种拓扑下输出一致;失败自动退直连 DB。

**注意事项**:
- `orchestrator` 邮箱在 GUI 存活时仅 router 消费,CLI 拉取被拒 —— 协调者不要把关键消息发给 `orchestrator`,用自有 handle(如 maestro@session-xxx)。
- `inject-prompt` 依赖标题 idle 判定;裸 shell(无 agent 标题)返回 None 视为可注入,但**读不到标题**时必须 `--force`(a47e3c81 修过 bare-shell 误判)。
- `read-worker --after` 的 cursor 在 stderr(`cursor: N`),stdout 纯内容 —— 解析别混流。
- `--type` 过滤是客户端侧,不匹配消息保持 unread —— 类型化轮询不会吞掉其他类型,但也意味着混型邮箱需要一次无过滤拉取清底。
- 生命周期自动化消息(worker_done/heartbeat)payload 是 JSON 且字段名固定(`task_id/dispatch_id/outcome`),结构错=Rejected(存审计不生效)。
- `start-worker --command` 的 block 驱动结算是自动 worker_done 的最短路径(无需协调者盯完成)。

**弃用/演进风险**:
- `send_task_dispatch` 未暴露 CLI,若未来接线可能新增命令(dispatch 前导词注入)。
- `deliveries` 表是联邦投递预留位,未来 message 投递合同若启用 batch 表,`read=0` 判定语义可能变化。
- runtime 元数据探测契约已切换为 `dais-runtime.json`(§5.3);仍按旧文件名探测的外部脚本需跟随,旧名文件在首个新版进程启动后被删除。
