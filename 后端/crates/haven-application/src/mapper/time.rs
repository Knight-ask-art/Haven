//! 时间映射：domain `UtcMillis(i64)` → Wire UTC RFC 3339 字符串（契约 §11.1）。
//!
//! 无效时间戳（负值/溢出）回退 `1970-01-01T00:00:00Z` 并保持确定性。

use chrono::{DateTime, SecondsFormat, Utc};

use haven_common::UtcMillis;

pub fn utc_millis_to_rfc3339(ms: UtcMillis) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms.0)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_utc_rfc3339() {
        // 1700000000000 ms = 2023-11-14T22:13:20Z
        assert_eq!(
            utc_millis_to_rfc3339(UtcMillis(1_700_000_000_000)),
            "2023-11-14T22:13:20Z"
        );
    }

    #[test]
    fn invalid_timestamp_falls_back_deterministically() {
        // 负值仍是合法时刻（epoch 前）；仅溢出（超出 chrono 范围）回退。
        assert_eq!(utc_millis_to_rfc3339(UtcMillis(-1)), "1969-12-31T23:59:59Z");
        assert_eq!(
            utc_millis_to_rfc3339(UtcMillis(i64::MAX)),
            "1970-01-01T00:00:00Z"
        );
    }
}
