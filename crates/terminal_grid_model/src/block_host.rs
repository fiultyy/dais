//! R18: Block 域宿主缝(R17-C 重件兑现)。
//! BlockTimerHost 抽象 warpui Background+Timer(测试可控性;wasm/native 由
//! warpui 内部切换);延迟选择语义留在 Block 内。

/// 延迟后在后台触发 `f`(detach 语义:返回即 forget)。
pub trait BlockTimerHost: Send + Sync {
    fn after_delay(&self, delay_ms: u64, f: Box<dyn FnOnce() + Send>);
}

/// 测试实现:立即同步执行(等价原 #[cfg(test)] wakeup_after_delay 语义)。
#[cfg(any(test, feature = "test-support"))]
pub struct ImmediateBlockTimerHost;

#[cfg(any(test, feature = "test-support"))]
impl BlockTimerHost for ImmediateBlockTimerHost {
    fn after_delay(&self, _delay_ms: u64, f: Box<dyn FnOnce() + Send>) {
        f();
    }
}
