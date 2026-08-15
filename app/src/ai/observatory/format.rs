//! 观测台数据查看格式化（DSH 严格布局规范 §5）。
//!
//! - DV14 相对时间分桶: 列表行尾列（[`relative_time`] 纯分桶 + [`relative_time_text`] 文案）
//! - DV15 绝对时间毫秒: 详情卡
//! - DV17 紧凑计数: token/字节/事件计数
//! - DV18 耗时分档（账本档 / 列表行档）
//! - DV19 未知值占位 "—"

use chrono::{NaiveDateTime, TimeZone, Utc};

/// 未知/缺失值占位（DV19）。
pub const UNKNOWN_DASH: &str = "—";

/// 相对时间桶（DV14）。数值由分桶函数随桶给出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeTimeBucket {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Days(u64),
    Months(u64),
    Years(u64),
}

/// 相对时间分桶（DV14 / AC-27，纯函数）。
///
/// 输入 now 与 t 的 epoch 秒；闭开区间按 now−t 计算:
/// <60s → 刚刚; <1h → N 分钟; <24h → N 小时; <30d → N 天;
/// <365d → N 月（30d = 1 月，整数除）; ≥365d → N 年（365d = 1 年）。
/// 未来时间（now−t < 0）按刚刚处理（时钟偏移容错）。
pub fn relative_time(now_epoch: i64, t_epoch: i64) -> RelativeTimeBucket {
    let delta = (now_epoch - t_epoch).max(0);
    if delta < 60 {
        return RelativeTimeBucket::JustNow;
    }
    if delta < 3600 {
        return RelativeTimeBucket::Minutes((delta / 60) as u64);
    }
    if delta < 86_400 {
        return RelativeTimeBucket::Hours((delta / 3600) as u64);
    }
    let days = (delta / 86_400) as u64;
    if days < 30 {
        return RelativeTimeBucket::Days(days);
    }
    if days < 365 {
        return RelativeTimeBucket::Months(days / 30);
    }
    RelativeTimeBucket::Years(days / 365)
}

/// 相对时间文案（DV16: 桶不变则文本不变；单位标签本地化）。
pub fn relative_time_text(now_epoch: i64, t_epoch: i64) -> String {
    match relative_time(now_epoch, t_epoch) {
        RelativeTimeBucket::JustNow => crate::t!("observatory-time-just-now").to_string(),
        RelativeTimeBucket::Minutes(n) => {
            crate::t!("observatory-time-minutes", n = n).to_string()
        }
        RelativeTimeBucket::Hours(n) => crate::t!("observatory-time-hours", n = n).to_string(),
        RelativeTimeBucket::Days(n) => crate::t!("observatory-time-days", n = n).to_string(),
        RelativeTimeBucket::Months(n) => crate::t!("observatory-time-months", n = n).to_string(),
        RelativeTimeBucket::Years(n) => crate::t!("observatory-time-years", n = n).to_string(),
    }
}

/// SQLite `CURRENT_TIMESTAMP` 文本（UTC "YYYY-MM-DD HH:MM:SS"）→ epoch 秒。
/// 解析失败返回 None（调用方回落占位）。
pub fn sqlite_text_to_epoch(s: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc().timestamp())
}

/// 绝对时间（DV15 / AC-28）: epoch 秒 → 本地时区
/// `YYYY-MM-DD HH:MM:SS.mmm`（毫秒 3 位；epoch 秒来源补 .000）。
/// 无值（None）显示占位 "—"。
pub fn absolute_time_millis(t_epoch: Option<i64>) -> String {
    match t_epoch {
        None => UNKNOWN_DASH.to_string(),
        Some(t) => match Utc.timestamp_opt(t, 0) {
            chrono::LocalResult::Single(utc) => utc
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
            _ => UNKNOWN_DASH.to_string(),
        },
    }
}

/// 绝对时间（SQLite 文本变体）: 解析失败显示占位。
pub fn absolute_time_sqlite_text(s: &str) -> String {
    absolute_time_millis(sqlite_text_to_epoch(s))
}

/// 紧凑计数（DV17 / AC-29）: <1000 原值; <10⁶ K 后缀; ≥10⁶ M 后缀;
/// 缩放值 <100 保留 1 位小数，≥100 取整（517 / 12.2K / 517K / 1.2M）。
pub fn compact_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let (scaled, suffix) = if n < 1_000_000 {
        (n as f64 / 1000., "K")
    } else {
        (n as f64 / 1_000_000., "M")
    };
    if scaled < 100. {
        format!("{scaled:.1}{suffix}")
    } else {
        format!("{}{suffix}", scaled.round() as u64)
    }
}

/// 字节计数紧凑化（DV17 同构，与 token 一致走十进制 K/M 档）。
pub fn compact_bytes(n: usize) -> String {
    compact_count(n as u64)
}

/// 耗时（DV18 账本/检查器档 / AC-29）:
/// None → "—"; <1s → "N ms"（取整）; <10s → "X.XX s"（2 位）; ≥10s → "X.X s"（1 位）。
pub fn format_duration_ledger_ms(ms: Option<u64>) -> String {
    match ms {
        None => UNKNOWN_DASH.to_string(),
        Some(ms) if ms < 1000 => format!("{ms} ms"),
        Some(ms) if ms < 10_000 => format!("{:.2} s", ms as f64 / 1000.),
        Some(ms) => format!("{:.1} s", ms as f64 / 1000.),
    }
}

/// 耗时（DV18 列表行档，自然语言桶）:
/// None → "—"; <60s → "X.Xs"; <1h → "Nm Ns"; ≥1h → "Nh Nm"。
pub fn format_duration_row_ms(ms: Option<u64>) -> String {
    match ms {
        None => UNKNOWN_DASH.to_string(),
        Some(ms) if ms < 60_000 => format!("{:.1}s", ms as f64 / 1000.),
        Some(ms) => {
            let total_secs = ms / 1000;
            let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
            if h > 0 {
                format!("{h}h{m}m")
            } else {
                format!("{m}m{s}s")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC-27: 相对时间桶边界（闭开区间）
    #[test]
    fn test_relative_time_buckets() {
        let now = 1_800_000_000;
        assert_eq!(relative_time(now, now), RelativeTimeBucket::JustNow);
        assert_eq!(relative_time(now, now - 59), RelativeTimeBucket::JustNow);
        assert_eq!(
            relative_time(now, now - 60),
            RelativeTimeBucket::Minutes(1)
        );
        assert_eq!(
            relative_time(now, now - 3599),
            RelativeTimeBucket::Minutes(59)
        );
        assert_eq!(
            relative_time(now, now - 3600),
            RelativeTimeBucket::Hours(1)
        );
        assert_eq!(
            relative_time(now, now - 86_399),
            RelativeTimeBucket::Hours(23)
        );
        assert_eq!(relative_time(now, now - 86_400), RelativeTimeBucket::Days(1));
        assert_eq!(
            relative_time(now, now - 29 * 86_400),
            RelativeTimeBucket::Days(29)
        );
        assert_eq!(
            relative_time(now, now - 30 * 86_400),
            RelativeTimeBucket::Months(1)
        );
        assert_eq!(
            relative_time(now, now - 364 * 86_400),
            RelativeTimeBucket::Months(12) // 364/30 = 12
        );
        assert_eq!(
            relative_time(now, now - 365 * 86_400),
            RelativeTimeBucket::Years(1)
        );
        // 时钟偏移容错: 未来时间按刚刚
        assert_eq!(relative_time(now, now + 5), RelativeTimeBucket::JustNow);
    }

    // AC-27 附: 桶数值正确性
    #[test]
    fn test_relative_time_values() {
        let now = 1_800_000_000;
        assert_eq!(
            relative_time(now, now - 120),
            RelativeTimeBucket::Minutes(2)
        );
        assert_eq!(
            relative_time(now, now - 7200),
            RelativeTimeBucket::Hours(2)
        );
        assert_eq!(
            relative_time(now, now - 5 * 86_400),
            RelativeTimeBucket::Days(5)
        );
        assert_eq!(
            relative_time(now, now - 90 * 86_400),
            RelativeTimeBucket::Months(3)
        );
        assert_eq!(
            relative_time(now, now - 730 * 86_400),
            RelativeTimeBucket::Years(2)
        );
    }

    // AC-28: 绝对时间格式 + 占位
    #[test]
    fn test_absolute_time_format() {
        assert_eq!(absolute_time_millis(None), "—");
        let got = absolute_time_millis(Some(1_800_000_000));
        // 本地时区因环境而异; 断言格式长度（YYYY-MM-DD HH:MM:SS.mmm = 23 字符）
        // 与毫秒 3 位（epoch 秒来源补 .000）。
        assert_eq!(got.len(), 23, "format with millis: {got}");
        assert!(got.ends_with(".000"), "millis 3 digits: {got}");
        assert!(got.starts_with("20"), "absolute date: {got}");
    }

    #[test]
    fn test_sqlite_text_to_epoch() {
        let expect = Utc
            .with_ymd_and_hms(2026, 8, 15, 12, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(sqlite_text_to_epoch("2026-08-15 12:00:00"), Some(expect));
        assert_eq!(sqlite_text_to_epoch("garbage"), None);
        assert_eq!(absolute_time_sqlite_text("garbage"), "—");
        // round-trip: 文本 → epoch → 绝对格式（UTC 环境下日期/时间部分一致）
        let epoch = sqlite_text_to_epoch("2026-08-15 12:00:00").unwrap();
        let abs = absolute_time_millis(Some(epoch));
        assert!(abs.contains(":00:00.000"), "round-trip time part: {abs}");
    }

    // AC-29: 紧凑计数
    #[test]
    fn test_compact_count() {
        assert_eq!(compact_count(517), "517");
        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1000), "1.0K");
        assert_eq!(compact_count(12_234), "12.2K");
        assert_eq!(compact_count(99_999), "100.0K");
        assert_eq!(compact_count(517_000), "517K");
        assert_eq!(compact_count(1_234_567), "1.2M");
        assert_eq!(compact_bytes(8_234), "8.2K");
    }

    // AC-29: 耗时分档（账本档）
    #[test]
    fn test_duration_ledger() {
        assert_eq!(format_duration_ledger_ms(None), "—");
        assert_eq!(format_duration_ledger_ms(Some(450)), "450 ms");
        assert_eq!(format_duration_ledger_ms(Some(999)), "999 ms");
        assert_eq!(format_duration_ledger_ms(Some(9_876)), "9.88 s");
        assert_eq!(format_duration_ledger_ms(Some(10_000)), "10.0 s");
        assert_eq!(format_duration_ledger_ms(Some(12_345)), "12.3 s");
    }

    // DV18 列表行档
    #[test]
    fn test_duration_row() {
        assert_eq!(format_duration_row_ms(None), "—");
        assert_eq!(format_duration_row_ms(Some(45_200)), "45.2s");
        assert_eq!(format_duration_row_ms(Some(162_000)), "2m42s");
        assert_eq!(format_duration_row_ms(Some(7_500_000)), "2h5m");
    }
}
