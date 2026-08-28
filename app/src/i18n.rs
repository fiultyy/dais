//! i18n re-export 壳 (2026-08-28 布局拆分 v1)。
//!
//! 实现已下沉到独立 crate `i18n` (crates/i18n): `#[macro_export]` 宏 `t!`/
//! `t_static!` 的 `$crate` 展开指向**定义 crate**, 留在 app 会迫使任何独立
//! crate (nav/layout/…) 为用宏而依赖 app, 形成循环。下沉后 app 内 870+ 个
//! `t!` 使用点、`crate::i18n::init/set_locale/...` 调用点全部保持原路径 —
//! 本壳 re-export 即可, 零调用点改动。
//!
//! 资源权威来源: `app/i18n/{locale}/*.ftl`; crate 内 `crates/i18n/i18n/`
//! 是构建用副本, 由 script/sync-i18n-ftl.sh 同步 (幂等, md5 校验)。

pub use i18n::{
    current_languages, init, loader, reset_to_system_locale, set_locale, t_or,
};

/// `app/src/i18n` 模块路径兼容层: `crate::i18n::t!` 在下沉后仍可用。
/// 注意: 宏的 `$crate` 现在展开为 `::i18n` (定义 crate), 不再是 app。

// `#[macro_export]` 宏在 crate 根命名空间: 仓库调用点是 `crate::t!` /
// `crate::t_static!` (解析到 app 根), 必须在根重导出 (约 3200 处零改动)。
pub use i18n::{t, t_static};

#[cfg(test)]
mod tests {
    /// 幂等验收 (split-plan §4.1): 下沉后调用点行为不变。
    /// - 宏从旧路径 `crate::i18n::t!` 展开成功 (re-export 链成立)
    /// - init 幂等 (重复调用不 panic 不换 loader)
    /// - 译文正确加载 (en bundle 嵌入 crate, app 侧读到)
    #[test]
    fn i18n_shell_preserves_call_sites() {
        crate::i18n::init(Some("en"));
        crate::i18n::init(Some("en")); // idempotent
        assert!(crate::i18n::loader().is_some());
        let s: String = crate::i18n::t!("common-ok");
        assert_eq!(s, "OK");
    }
}
