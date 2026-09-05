//! StreamRegistry：远端流播放授权（V2-B 实战批次；契约 §36.4）。
//!
//! - grant 是不透明 UUID，绑定 owner 窗口标签与上游主机白名单。
//! - 白名单初始只含资源自身 host；HLS manifest 经代理时把其中出现的主机
//!   收敛进白名单（防 SSRF：代理不会打开 grant 未学习到的任意地址）。
//! - 原始 URL 永不出本注册表。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(test)]
use haven_common::network::validate_host;
use haven_common::network::{parse_http_url, HttpUrlPolicy};
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
    stream_state: Mutex<StreamTargetState>,
    created_at: Instant,
}

#[derive(Default)]
struct StreamTargetState {
    targets: HashMap<String, RegisteredTarget>,
    manifest_versions: HashMap<String, VecDeque<String>>,
}

struct RegisteredTarget {
    manifest_key: String,
    version: String,
    target: String,
    host: String,
}

// A single long-form VOD manifest can legitimately contain thousands of
// unique segment references. Keep the bound finite while ensuring one
// manifest cannot invalidate references already handed to the player.
const TARGET_CAP: usize = 4096;
// Keep the current manifest plus one retired generation so requests already
// dispatched by the player have a bounded grace period.
const MANIFEST_VERSION_GRACE: usize = 2;

impl StreamGrantInner {
    /// Atomically replace one manifest slot. Candidates are prepared privately
    /// by the request; capacity failure leaves every committed manifest intact.
    pub(crate) fn commit_manifest(
        &self,
        manifest_key: &str,
        version: &str,
        candidates: &[(String, String)],
    ) -> bool {
        let Some(validated) = candidates
            .iter()
            .map(|(token, target)| {
                host_of(target).map(|host| (token.clone(), target.clone(), host))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };

        let mut state = self.stream_state.lock().unwrap_or_else(|e| e.into_inner());
        let mut versions = state
            .manifest_versions
            .get(manifest_key)
            .cloned()
            .unwrap_or_default();
        versions.push_back(version.to_owned());
        let mut retired = HashSet::new();
        while versions.len() > MANIFEST_VERSION_GRACE {
            if let Some(value) = versions.pop_front() {
                retired.insert(value);
            }
        }
        let retained = state
            .targets
            .values()
            .filter(|entry| entry.manifest_key != manifest_key || !retired.contains(&entry.version))
            .count();
        if retained.saturating_add(validated.len()) > TARGET_CAP {
            return false;
        }

        state.targets.retain(|_, entry| {
            entry.manifest_key != manifest_key || !retired.contains(&entry.version)
        });
        for (token, target, host) in validated {
            state.targets.insert(
                token,
                RegisteredTarget {
                    manifest_key: manifest_key.to_owned(),
                    version: version.to_owned(),
                    target,
                    host,
                },
            );
        }
        state
            .manifest_versions
            .insert(manifest_key.to_owned(), versions);
        true
    }

    /// 目标主机是否被授权（初始主机或 manifest 学习到的主机）。
    pub fn host_allowed(&self, host: &str) -> bool {
        if host == self.initial_host {
            return true;
        }
        self.stream_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .targets
            .values()
            .any(|entry| entry.host == host)
    }

    /// Resolve a browser-provided target token.  Callers must still apply the
    /// grant's host policy to the resolved URL before issuing a request.
    pub fn resolve_target(&self, manifest_version: &str, token: &str) -> Option<String> {
        self.stream_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .targets
            .get(token)
            .filter(|entry| entry.version == manifest_version)
            .map(|entry| entry.target.clone())
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
        let inner = Arc::new(StreamGrantInner {
            facts,
            owner_label: owner_label.to_owned(),
            initial_host,
            stream_state: Mutex::new(StreamTargetState::default()),
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

/// Signed query parameters and fragments identify a fetch authorization, not
/// a different logical HLS playlist. Redirects may change those parameters on
/// every refresh, so use the stable HTTP URL components as the manifest slot.
pub(crate) fn manifest_identity(url: &str) -> Option<String> {
    let mut parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string())
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
    fn committed_manifest_hosts_are_authorized_others_denied() {
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
        assert!(inner.commit_manifest(
            "https://cdn.example.com/video.m3u8",
            "v1",
            &[(
                "token-1".into(),
                "https://seg.other-cdn.net/segment.ts".into()
            )],
        ));
        assert!(inner.host_allowed("seg.other-cdn.net"));
        assert!(!inner.commit_manifest(
            "https://cdn.example.com/unsafe.m3u8",
            "v1",
            &[("unsafe".into(), "http://127.0.0.1/segment.ts".into())],
        ));
        assert!(!inner.host_allowed("127.0.0.1"));
    }

    #[test]
    fn repeated_manifest_refresh_reclaims_retired_versions() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();

        let mut previous: Option<(String, String)> = None;
        for refresh in 0..1_000 {
            let version = format!("v{refresh}");
            let candidates = (0..6)
                .map(|segment| {
                    (
                        format!("t-{refresh}-{segment}"),
                        format!("https://cdn.example.com/segment-{}.ts", refresh + segment),
                    )
                })
                .collect::<Vec<_>>();
            assert!(inner.commit_manifest(
                "https://cdn.example.com/live.m3u8",
                &version,
                &candidates
            ));
            if let Some((old_version, old_token)) = previous.take() {
                assert!(inner.resolve_target(&old_version, &old_token).is_some());
            }
            previous = Some((version, candidates[0].0.clone()));
        }
        let state = inner.stream_state.lock().unwrap();
        assert_eq!(state.targets.len(), 12);
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
    fn manifest_identity_ignores_rotating_signature_and_fragment() {
        assert_eq!(
            manifest_identity("https://cdn.example.com/live/index.m3u8?sig=one&expires=1#x"),
            Some("https://cdn.example.com/live/index.m3u8".into())
        );
        assert_eq!(
            manifest_identity("https://cdn.example.com/live/index.m3u8?sig=two&expires=2"),
            manifest_identity("https://cdn.example.com/live/index.m3u8?sig=one&expires=1")
        );
    }

    #[test]
    fn concurrent_manifest_commit_has_isolated_atomic_result() {
        use std::sync::Barrier;
        use std::thread;

        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let existing = (0..TARGET_CAP - 1)
            .map(|index| {
                (
                    format!("base-{index}"),
                    format!("https://cdn.example.com/base-{index}.ts"),
                )
            })
            .collect::<Vec<_>>();
        assert!(inner.commit_manifest("base", "v1", &existing));

        let barrier = Arc::new(Barrier::new(2));
        let left = Arc::clone(&inner);
        let left_barrier = Arc::clone(&barrier);
        let left_thread = thread::spawn(move || {
            left_barrier.wait();
            left.commit_manifest(
                "video",
                "video-v1",
                &[(
                    "video-token".into(),
                    "https://video.example.net/v.ts".into(),
                )],
            )
        });
        let right = Arc::clone(&inner);
        let right_barrier = Arc::clone(&barrier);
        let right_thread = thread::spawn(move || {
            right_barrier.wait();
            right.commit_manifest(
                "audio",
                "audio-v1",
                &[(
                    "audio-token".into(),
                    "https://audio.example.net/a.ts".into(),
                )],
            )
        });
        let left_ok = left_thread.join().unwrap();
        let right_ok = right_thread.join().unwrap();
        assert_eq!(left_ok as u8 + right_ok as u8, 1);
        if left_ok {
            assert!(inner.resolve_target("video-v1", "video-token").is_some());
            assert!(inner.host_allowed("video.example.net"));
        } else {
            assert!(inner.resolve_target("audio-v1", "audio-token").is_some());
            assert!(inner.host_allowed("audio.example.net"));
        }
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
        let token = Uuid::new_v4().to_string();
        assert!(inner.commit_manifest(
            "https://cdn.example.com/a.m3u8",
            "v1",
            &[(token.clone(), "https://cdn.example.com/seg-01.ts".into())],
        ));
        assert_eq!(token.len(), 36);
        assert!(!token.contains("://"));
        assert_eq!(
            inner.resolve_target("v1", &token).as_deref(),
            Some("https://cdn.example.com/seg-01.ts")
        );
        assert!(inner.resolve_target("other-version", &token).is_none());
        assert!(registry.lookup(&grant.to_string(), "other").is_none());
    }

    #[test]
    fn failed_manifest_commit_does_not_damage_another_manifest() {
        let registry = StreamRegistry::new();
        let grant = registry
            .register(
                facts("https://cdn.example.com/a.m3u8"),
                "https://cdn.example.com/a.m3u8",
                "main",
            )
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let existing = (0..TARGET_CAP - 1)
            .map(|index| {
                (
                    format!("base-{index}"),
                    format!("https://cdn.example.com/base-{index}.ts"),
                )
            })
            .collect::<Vec<_>>();
        assert!(inner.commit_manifest("base", "v1", &existing));

        let successful_token = "successful-token".to_owned();
        assert!(inner.commit_manifest(
            "video",
            "v1",
            &[(
                successful_token.clone(),
                "https://video.example.net/segment.ts".into()
            )],
        ));
        assert!(!inner.commit_manifest(
            "audio",
            "v1",
            &[(
                "overflow".into(),
                "https://audio.example.net/segment.aac".into()
            )],
        ));
        assert!(inner.resolve_target("v1", &successful_token).is_some());
        assert!(inner.host_allowed("video.example.net"));
        assert!(!inner.host_allowed("audio.example.net"));
    }
}
