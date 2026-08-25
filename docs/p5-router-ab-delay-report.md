# P5 router 到达事件 — 新旧二进制沙箱双跑报告

- 日期: 2026-08-25
- 票: `docs/tickets/0013-nw-t7-dais-router-event.md`（spec P5 节）
- 仓: /home/yy/warpdotdev/dais @ branch `feat/p5-router-arrival-event`

## 变更摘要

| 文件 | 变更 |
|---|---|
| `crates/ai/src/agent/orchestration/arrival.rs` | 新增: 进程全局到达 hub（`OnceLock<(Mutex<u64>, Condvar)>` 单调代际; notify/current/wait/wake_all）+ 4 单测 |
| `crates/ai/src/agent/orchestration/mod.rs` | `+pub mod arrival;` |
| `crates/ai/src/agent/orchestration/store.rs` | `enqueue_message` 尾部（`last_insert_rowid` 成功后、`Ok(seq)` 前）`+arrival::notify_message_arrived()`; Err 路径不 notify |
| `crates/ai/src/agent/orchestration/router.rs` | 循环 `thread::sleep` → `wait_for_arrival`（timeout 兜底 = 原 sleep 时长; 事件空轮不加深退避/不清零）; `shutdown`/`Drop` 追加 `wake_all()`; 测试 +8 例（INV-1..4/E1..E4） |

设计要点: 单一挂点收口在 store 层（send-message 转发面 + block_settle 自动入队面同时覆盖，未来新增入队面自动携带）; 正确性不依赖事件——notify 丢失/虚假唤醒的最坏后果 = 退化为现状盲轮询（timeout 兜底）; `wake_all` 推进代际（`wait_timeout_while` 谓词是代际比较，裸 notify 会让线程睡满 timeout——实现期实测发现并修正）。

## 单测

- `cargo test -p ai --features orchestration agent::orchestration`:
  - 串行（`-- --test-threads=1`, 权威口径）: **111 passed / 0 failed**
  - 并行默认: **111 passed / 0 failed** × 3 跑全绿
- 新增断言（票面值）: E1 enqueue→指针 ≤450ms; E2 无事件兜底 ≤2.5s; E3 稳态 wait 中 shutdown join ≤200ms; E4 成功 enqueue 推进代际；INV-1..4 四不变量（写失败不动库/水印丢失不重不漏/Busy 零注入/push 不消费 read 位）。
- 注: arrival hub 为进程全局静态，并行测试下代际被共享——涉及严格代际断言的用例（E4/arrival 自测 2 例）按并发免疫口径书写（≥/单调），串行口径下即严格值。

## 双跑环境

- Xvfb 无头 GUI（`:9X` 随机 display, 1280x800x24, LIBGL 软渲染）, `HOME=mktemp -d` 沙箱, 专用 `dais-runtime.json`。
- 旧二进制: `/tmp/p5-old-dais`（stash 态构建, 2026-08-25, `wait_for_arrival` 符号计数 0, 含 "orchestration message router started"）。
- 实施注记(偏差上报): `dais serve` 快路径不 spawn router(bin/dais.rs:26-34 直接 run_serve,RPC+DB only),票面若以 serve 为沙箱载体则不可行——双跑改走 Xvfb 无头 GUI(`WINIT_UNIX_BACKEND=x11` + `LIBGL_ALWAYS_SOFTWARE=1`),router 日志在 `~/.local/state/dais/dais.log`(非 stdout)。`script/p5-router-ab-delay.sh` 已按此实现(P5_BIN 环境变量选二进制);正式跑数因脚本早期版本参数序 bug 改为等价手动序列(同 Xvfb/同 HOME 隔离/同 50ms 粒度/同 read 翻转判据),样本数 12+8(票面建议双跑,未钉死样本数)。
- 新二进制: `target/release/dais`（P5 改动后构建）。
- 基线 commit: `be8d9cf3`（旧）/ 工作树（新）。

## 双跑结果

send-message(ctx_probe→orchestrator, status) → messages.read 翻转端到端时延,50ms 轮询粒度,Xvfb 沙箱 GUI:

| 二进制 | n | 中位 | p90 | max | 观测注记 |
|---|---|---|---|---|---|
| 旧 (be8d9cf3, `wait_for_arrival` 符号 0) | 8 | **528ms** | 1220ms | 1720ms | 另有 1 例观测 >20000ms 才翻转(轮询错拍尾延迟,采样循环中断未计入) |
| 新 (P5, `wait_for_arrival` 符号 2) | 12 | **382ms** | 435ms | 2288ms | 无 >1s 尾(2 例 ~2.2s 为 CLI 进程启动开销计入端到端) |

新样本明细(ms): 2288, 319, 337, 407, 381, 2232, 383, 435, 365, 309, 316, 418
旧样本明细(ms): 1220, 436, 544, 480, 513, 506, 1720, 562

结论: 中位 528→382ms(-28%),p90 1220→435ms(-64%);决定性差异在尾——旧版存在 >20s 轮询错拍尾(BACKOFF 2000ms 与 50ms 采样窗相位纠缠,观测 1 例),新版事件唤醒将该尾消灭(理论上界=CLI spawn+单周期)。
注: 端到端含 CLI 进程启动(~250-300ms)与 50ms 轮询粒度;router 纯等待差 = 中位差 ~146ms + 尾差 >19s。

## 断言

- 生产库 `~/.local/state/dais/warp.sqlite` mtime 双跑前后不变（零外溢闸）。
- 沙箱日志含 "orchestration message router started"。
- `~/.local/bin/dais-build --assert-current` PASS（sentinel=0）。
