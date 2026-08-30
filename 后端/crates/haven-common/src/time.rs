//! 时间戳：统一使用 UTC 毫秒（i64），避免时区与精度分歧。

use std::time::{SystemTime, UNIX_EPOCH};

/// UTC 毫秒时间戳。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct UtcMillis(pub i64);

impl UtcMillis {
    /// 当前 UTC 毫秒。
    pub fn now() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        UtcMillis(ms)
    }

    pub fn from_millis(ms: i64) -> Self {
        UtcMillis(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_positive_and_monotonic() {
        let a = UtcMillis::now();
        let b = UtcMillis::now();
        assert!(b.0 >= a.0);
        assert!(a.0 > 0);
    }

    #[test]
    fn serializes_as_plain_number() {
        let ts = UtcMillis(1_700_000_000_000);
        assert_eq!(serde_json::to_string(&ts).unwrap(), "1700000000000");
        let back: UtcMillis = serde_json::from_str("1700000000000").unwrap();
        assert_eq!(back, ts);
    }
}
