//! arrival.rs — P5 进程全局到达 hub(dais router 到达事件)。
//!
//! 单一挂点 = `DieselOrchestrationStore::enqueue_message` 成功返回处(P5.1):
//! 生产调用面恰两处(GUI 转发 send-message + block_settle worker_done 自动入队),
//! 全部经 store 层收口,未来新增入队面自动携带事件。
//!
//! 拍板: u64 单调代际 counter(而非裸 `Mutex<()>+Condvar`)——notify 落空窗不是
//! 微秒级: router 单周期含 push_pending,而 deliver_pending 内嵌 500ms
//! split-submit sleep,周期进行中到达的 enqueue 在裸 Condvar 下必然丢事件;
//! counter + 先查后等以约 15 行代价闭掉该窗。正确性不依赖事件——任何 notify
//! 丢失/虚假唤醒/wake_all 竞态的最坏后果 = 退化为现状盲轮询(timeout 兜底)。
//!
//! 进程全局(OnceLock 静态,范式参照 delivery.rs REGISTRY)而非 store 实例字段:
//! router 持专用连接的 store clone,enqueue 调用方各持别的 clone——通知必须跨 clone。

use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

static HUB: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();

fn hub() -> &'static (Mutex<u64>, Condvar) {
    HUB.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

/// 代际 +1 + notify_all,返回新代际(enqueue 成功后调用;Err 路径不调)。
pub fn notify_message_arrived() -> u64 {
    let (lock, cvar) = hub();
    let mut gen = lock.lock().unwrap();
    *gen += 1;
    let current = *gen;
    drop(gen);
    cvar.notify_all();
    current
}

/// 读当前代际(wait 前检查点)。
pub fn current_arrival() -> u64 {
    *hub().0.lock().unwrap()
}

/// 阻塞至代际 > last_seen 或超时,返回(最新代际, 是否超时)。
/// 先查后等: 进入 wait 前代已推进则立即返回——闭"周期进行中 notify 落空"窗。
pub fn wait_for_arrival(last_seen: u64, timeout: Duration) -> (u64, bool) {
    let (lock, cvar) = hub();
    let mut gen = lock.lock().unwrap();
    if *gen > last_seen {
        return (*gen, false);
    }
    let (guard, timed_out) = cvar.wait_timeout_while(gen, timeout, |g| *g <= last_seen).unwrap();
    (*guard, timed_out.timed_out())
}

/// 广播唤醒,仅供 shutdown/Drop。
/// 必须推进代际: `wait_timeout_while` 谓词是"代际 > last_seen",裸 notify_all
/// 唤醒后谓词仍满足、线程继续睡满 timeout——以一次代际推进换所有 wait
/// 立即返回(被唤醒的线程回循环头检出 shutdown flag 退出)。
/// 代价: 进程内正在进行的代际严格断言须容忍并发 shutdown 插入 +1。
pub fn wake_all() {
    notify_message_arrived();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn notify_then_wait_returns_immediately() {
        let before = current_arrival();
        let notified = notify_message_arrived();
        assert!(notified > before);
        let (seen, timed_out) = wait_for_arrival(before, Duration::from_millis(50));
        assert!(!timed_out);
        assert!(seen >= notified);
    }

    #[test]
    fn wait_timeout_returns_stale_generation() {
        // 进程全局代际被并行测试共享: 若等待窗内有并发 notify,则代际推进、
        // 不超时——该情形本身是正确行为。本用例只锁"无并发推进时"的 stale 语义,
        // 允许并发推进时改为断言 seen > before(事件优先于超时返回)。
        let before = current_arrival();
        let (seen, timed_out) = wait_for_arrival(before, Duration::from_millis(30));
        if timed_out {
            assert_eq!(seen, before);
        } else {
            assert!(seen > before, "非超时返回时代际必须推进");
        }
    }

    #[test]
    fn wake_all_wakes_all_waiters() {
        // wake_all 推进代际(设计如此,见函数注): 两个并发 waiter 都须立即
        // 返回且非超时,而非睡满 timeout。
        let before = current_arrival();
        let w1 = thread::spawn(move || wait_for_arrival(before, Duration::from_millis(5000)));
        let w2 = thread::spawn(move || wait_for_arrival(before, Duration::from_millis(5000)));
        thread::sleep(Duration::from_millis(50)); // 让 waiter 进 wait
        wake_all();
        let (g1, t1) = w1.join().unwrap();
        let (g2, t2) = w2.join().unwrap();
        assert!(!t1 && !t2, "wake_all 后不得超时: t1={t1} t2={t2}");
        assert!(g1 > before && g2 > before, "代际必须推进: g1={g1} g2={g2} before={before}");
    }

    #[test]
    fn concurrent_notify_is_monotonic() {
        let before = current_arrival();
        let stop = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let stop = stop.clone();
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        notify_message_arrived();
                    }
                })
            })
            .collect();
        thread::sleep(Duration::from_millis(50));
        notify_message_arrived();
        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        assert!(current_arrival() > before);
    }
}
