//! StreamRegistry：远端流播放授权（V2-B 实战批次；契约 §36.4）。
//!
//! - grant 是不透明 UUID，绑定 owner 窗口标签与上游主机白名单。
//! - 白名单初始只含资源自身 host；HLS manifest 经代理时把其中出现的主机
//!   收敛进白名单（防 SSRF：代理不会打开 grant 未学习到的任意地址）。
//! - 原始 URL 永不出本注册表。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use haven_common::network::{parse_http_url, validate_host, HttpUrlPolicy};
use haven_common::{AppError, ErrorKind};
use uuid::Uuid;

use haven_application::wire::ProgressSummaryDto;

/// 单个流会话的服务端事实快照。
#[derive(Debug, Clone)]
pub struct StreamGrantFacts {
    pub work_id: String,
    pub edition_id: String,
    pub media_item_id: String,
    pub mime_type: Option<String>,
    pub is_hls: bool,
    pub progress: Option<ProgressSummaryDto>,
    /// 初始上游地址（manifest 或直连媒体）；仅存于服务端。
    pub upstream_url: String,
}

pub struct StreamGrantInner {
    pub facts: StreamGrantFacts,
    pub owner_label: String,
    initial_host: String,
    allowed_hosts: Mutex<HashSet<String>>,
    /// Opaque tokens for HLS manifest targets.  The browser receives only the
    /// token in the proxy URI; the real upstream URL remains in this
    /// owner-bound registry entry.
    targets: Mutex<HashMap<String, String>>,
    /// Insertion order for `targets`. `HashMap` iteration order is deliberately
    /// unspecified, so it cannot be used to implement the bounded eviction
    /// policy safely.
    target_order: Mutex<VecDeque<String>>,
    /// Manifest hosts learned after the initial request. Keep their insertion
    /// order separately so a hostile manifest cannot grow this set without
    /// bound. The initial host is retained independently and never evicted.
    host_order: Mutex<VecDeque<String>>,
    created_at: Instant,
}

const TARGET_CAP: usize = 512;
const LEARNED_HOST_CAP: usize = 64;

impl StreamGrantInner {
    /// 目标主机是否被授权（初始主机或 manifest 学习到的主机）。
    pub fn host_allowed(&self, host: &str) -> bool {
        if host == self.initial_host {
            return true;
        }
        self.allowed_hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(host)
    }

    /// 学习（收敛）manifest 中出现的主机。
    pub fn learn_hosts(&self, hosts: impl IntoIterator<Item = impl Into<String>>) {
        let mut allowed = self.allowed_hosts.lock().unwrap_or_else(|e| e.into_inner());
        let mut order = self.host_order.lock().unwrap_or_else(|e| e.into_inner());
        for host in hosts.into_iter().map(Into::into) {
            // `rewrite_hls_manifest` supplies normalized hosts from `host_of`,
            // but keep this method defensive because it is the registry's
            // security boundary and is also exercised directly by tests.
            let Ok(host) = validate_host(&host) else {
                continue;
            };
            if host == self.initial_host || !allowed.insert(host.clone()) {
                continue;
            }
            order.push_back(host);
            while order.len() > LEARNED_HOST_CAP {
                let Some(oldest) = order.pop_front() else {
                    break;
                };
                allowed.remove(&oldest);
            }
        }
    }

    /// Register one validated absolute target and return an opaque UUID token.
    /// The token is intentionally independent of the target contents so a
    /// browser-visible manifest cannot disclose the upstream URL. Reuse an
    /// existing token for repeated URIs so a manifest with repeated segments
    /// cannot consume the bounded target table unnecessarily.
    pub fn register_target(&self, target: &str) -> String {
        let mut targets = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        let mut order = self.target_order.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(token) = targets
            .iter()
            .find_map(|(token, value)| (value == target).then(|| token.clone()))
        {
            // Refresh the insertion order as a small LRU rule. A target that
            // remains referenced by a later manifest should outlive stale
            // targets when the table reaches its cap.
            if let Some(position) = order.iter().position(|item| item == &token) {
                order.remove(position);
            }
            order.push_back(token.clone());
            return token;
        }

        let token = Uuid::new_v4().to_string();
        while targets.len() >= TARGET_CAP {
            let Some(oldest) = order.pop_front() else {
                // The order list is an internal invariant. If it is ever
                // damaged, fail closed by removing one actual target rather
                // than allowing an unbounded map to grow.
                if let Some(oldest) = targets.keys().next().cloned() {
                    targets.remove(&oldest);
                }
                break;
            };
            targets.remove(&oldest);
        }
        targets.insert(token.clone(), target.to_owned());
        order.push_back(token.clone());
        token
    }

    /// Resolve a browser-provided target token.  Callers must still apply the
    /// grant's host policy to the resolved URL before issuing a request.
    pub fn resolve_target(&self, token: &str) -> Option<String> {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(token)
            .cloned()
    }
}

/// 流授权注册表。容量上限淘汰最旧，防无界增长。
#[derive(Default)]
pub struct StreamRegistry {
    grants: Mutex<std::collections::HashMap<Uuid, Arc<StreamGrantInner>>>,
}

const REGISTRY_CAP: usize = 32;

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册新 grant；返回不透明 UUID。
    pub fn register(
        &self,
        facts: StreamGrantFacts,
        upstream_url: &str,
        owner_label: &str,
    ) -> Result<Uuid, AppError> {
        if facts.upstream_url != upstream_url {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "流目标事实与授权地址不一致",
                false,
            ));
        }
        let initial_host = host_of(upstream_url).ok_or_else(|| {
            AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "流目标地址不受安全策略允许",
                false,
            )
        })?;
        Ok(self.insert_grant(facts, initial_host, owner_label))
    }

    /// Test-only transport injection for the local HTTP fixture. The fixture
    /// still has to use an HTTP(S) URL with a safe host, but may select a
    /// random listener port; production registration keeps the common-port
    /// policy from `haven-common`.
    #[cfg(test)]
    pub(crate) fn register_fixture(
        &self,
        facts: StreamGrantFacts,
        upstream_url: &str,
        owner_label: &str,
    ) -> Result<Uuid, AppError> {
        if facts.upstream_url != upstream_url {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标事实与授权地址不一致",
                false,
            ));
        }
        if upstream_url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标地址不受安全策略允许",
                false,
            ));
        }
        let parsed = upstream_url.parse::<reqwest::Url>().map_err(|_| {
            AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标地址不受安全策略允许",
                false,
            )
        })?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标地址不受安全策略允许",
                false,
            ));
        }
        let initial_host = validate_host(parsed.host_str().ok_or_else(|| {
            AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标地址不受安全策略允许",
                false,
            )
        })?)
        .map_err(|_| {
            AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "测试流目标地址不受安全策略允许",
                false,
            )
        })?;
        Ok(self.insert_grant(facts, initial_host, owner_label))
    }

    fn insert_grant(
        &self,
        facts: StreamGrantFacts,
        initial_host: String,
        owner_label: &str,
    ) -> Uuid {
        let grant = Uuid::new_v4();
        let mut hosts = HashSet::new();
        hosts.insert(initial_host.clone());
        let inner = Arc::new(StreamGrantInner {
            facts,
            owner_label: owner_label.to_owned(),
            initial_host,
            allowed_hosts: Mutex::new(hosts),
            targets: Mutex::new(HashMap::new()),
            target_order: Mutex::new(VecDeque::new()),
            host_order: Mutex::new(VecDeque::new()),
            created_at: Instant::now(),
        });
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        while grants.len() >= REGISTRY_CAP {
            if let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, g)| g.created_at)
                .map(|(k, _)| *k)
            {
                grants.remove(&oldest);
            } else {
                break;
            }
        }
        grants.insert(grant, inner);
        grant
    }

    /// 按 owner 校验后返回 grant（撤销/跨窗口访问一律 None）。
    pub fn lookup(&self, grant: &str, owner_label: &str) -> Option<Arc<StreamGrantInner>> {
        let id = Uuid::parse_str(grant).ok()?;
        let grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants
            .get(&id)
            .filter(|g| g.owner_label == owner_label)
            .cloned()
    }

    /// 撤销（幂等；返回是否存在过）。
    pub fn revoke(&self, grant: &str, owner_label: &str) -> bool {
        let Ok(id) = Uuid::parse_str(grant) else {
            return false;
        };
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants
            .get(&id)
            .is_some_and(|g| g.owner_label == owner_label)
            && grants.remove(&id).is_some()
    }
}

/// 从 URL 提取小写 host。
pub(crate) fn host_of(url: &str) -> Option<String> {
    parse_http_url(url, HttpUrlPolicy::MediaResource)
        .ok()
        .map(|safe| safe.host().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(upstream_url: &str) -> StreamGrantFacts {
        StreamGrantFacts {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            mime_type: Some("application/vnd.apple.mpegurl".into()),
            is_hls: true,
            progress: None,
            upstream_url: upstream_url.into(),
        }
    }

    #[test]
    fn register_lookup_revoke_by_owner() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a/index.m3u8"),
                "https://cdn.example.com/a/index.m3u8",
                "main",
            )
            .unwrap();
        let id = grant.to_string();
        assert!(registry.lookup(&id, "main").is_some());
        assert!(registry.lookup(&id, "other").is_none(), "跨窗口拒绝");
        assert!(registry.revoke(&id, "main"));
        assert!(!registry.revoke(&id, "main"), "幂等撤销");
        assert!(registry.lookup(&id, "main").is_none());
    }

    #[test]
    fn registration_rejects_unsafe_initial_targets() {
        let registry = StreamRegistry::new();
        for url in [
            "http://127.0.0.1/video.mp4",
            "http://10.0.0.1/video.mp4",
            "http://169.254.169.254/latest/meta-data",
            "https://[::1]/video.mp4",
            "https://media.internal/video.mp4",
            "https://cdn.example.com:12345/video.mp4",
        ] {
            let error = registry.register(facts(url), url, "main").unwrap_err();
            assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED", "{url}");
        }
    }

    #[test]
    fn hosts_learned_via_manifest_are_authorized_others_denied() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        assert!(inner.host_allowed("cdn.example.com"));
        assert!(
            !inner.host_allowed("seg.other-cdn.net"),
            "未学习主机默认拒绝"
        );
        inner.learn_hosts(["seg.other-cdn.net"]);
        assert!(inner.host_allowed("seg.other-cdn.net"));
        inner.learn_hosts([
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "localhost",
            "media.internal",
            "2001:db8::1",
        ]);
        assert!(!inner.host_allowed("127.0.0.1"));
        assert!(!inner.host_allowed("media.internal"));
    }

    #[test]
    fn learned_hosts_are_bounded_and_evict_oldest_non_initial_host() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();

        inner.learn_hosts(
            (0..(LEARNED_HOST_CAP + 2)).map(|index| format!("segment-{index}.example.net")),
        );

        assert!(inner.host_allowed("cdn.example.com"), "初始主机不能被淘汰");
        assert!(!inner.host_allowed("segment-0.example.net"));
        assert!(!inner.host_allowed("segment-1.example.net"));
        assert!(inner.host_allowed("segment-2.example.net"));
        assert!(inner.host_allowed("segment-65.example.net"));
    }

    #[test]
    fn host_of_extracts_lowercased_host() {
        assert_eq!(
            host_of("HTTPS://CDN.Example.COM:8443/a/x.ts").as_deref(),
            Some("cdn.example.com")
        );
        assert_eq!(host_of("ftp://x/y"), None);
        assert_eq!(host_of("not-a-url"), None);
        assert_eq!(
            host_of("https://[2001:4860:4860::8888]:8443/a/x.ts").as_deref(),
            Some("2001:4860:4860::8888")
        );
        assert_eq!(host_of("https://user:secret@cdn.example.com/a"), None);
        assert_eq!(host_of("https://cdn.example.com/a\n.ts"), None);
    }

    #[test]
    fn target_tokens_are_opaque_and_owner_bound() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let token = inner.register_target("https://cdn.example.com/seg-01.ts");
        assert_eq!(token.len(), 36);
        assert!(!token.contains("://"));
        assert_eq!(
            inner.resolve_target(&token).as_deref(),
            Some("https://cdn.example.com/seg-01.ts")
        );
        assert!(registry.lookup(&grant.to_string(), "other").is_none());
    }

    #[test]
    fn target_tokens_evict_in_insertion_order() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let first = inner.register_target("https://cdn.example.com/first.ts");
        let second = inner.register_target("https://cdn.example.com/second.ts");

        for index in 0..TARGET_CAP {
            inner.register_target(&format!("https://cdn.example.com/segment-{index}.ts"));
        }

        assert_eq!(inner.resolve_target(&first), None, "最早令牌应先淘汰");
        assert_eq!(inner.resolve_target(&second), None, "第二早令牌也应被淘汰");
        let last = inner.register_target("https://cdn.example.com/last.ts");
        assert_eq!(
            inner.resolve_target(&last).as_deref(),
            Some("https://cdn.example.com/last.ts")
        );
    }

    #[test]
    fn repeated_target_reuses_token_and_refreshes_eviction_order() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let repeated = "https://cdn.example.com/repeated.ts";
        let first = inner.register_target(repeated);

        for index in 0..TARGET_CAP - 1 {
            inner.register_target(&format!("https://cdn.example.com/segment-{index}.ts"));
        }
        let refreshed = inner.register_target(repeated);
        assert_eq!(refreshed, first);

        inner.register_target("https://cdn.example.com/evict-oldest.ts");
        assert!(inner.resolve_target(&first).is_some());
    }
}
