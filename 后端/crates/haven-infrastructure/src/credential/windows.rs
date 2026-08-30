//! Windows Credential Manager 实现（ADR-001）。
//!
//! - Generic Credential（CRED_TYPE_GENERIC），显式 `persistence=Local`
//!   （windows-native-keyring-store 0.5.1 的 build 默认是 Enterprise，会随用户配置漫游，必须显式覆盖）。
//! - 错误归一化为稳定错误码 `CREDENTIAL_ACCESS_FAILED`；日志只记录操作类型与脱敏 ref 前缀。
//! - 删除是幂等的：目标不存在返回 `Ok(false)`。

use std::collections::HashMap;
use std::sync::Arc;

use keyring_core::api::CredentialStoreApi;
use keyring_core::{Entry, Error as KeyringError};
use windows_native_keyring_store::Store;
use zeroize::Zeroize;

use haven_common::AppError;
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

/// 稳定错误码（ADR-001：所有 Windows API 错误归一化）。
const CREDENTIAL_ACCESS_FAILED: &str = "CREDENTIAL_ACCESS_FAILED";

/// Windows Credential Manager 实现。
pub struct WindowsCredentialStore {
    store: Arc<Store>,
}

impl WindowsCredentialStore {
    pub fn new() -> Result<Self, AppError> {
        let store = Store::new().map_err(map_error("初始化凭据存储"))?;
        Ok(Self { store })
    }

    fn build_entry(&self, target: &CredentialRef) -> Result<Entry, AppError> {
        // S-01 修复：边界兜底校验——即使上层/DB 值绕过了 Domain 构造器，
        // 也拒绝非 `haven:` 命名空间 target（脏数据纵深防御）。
        haven_domain::credential::parse_scoped(target.as_str()).map_err(|_| {
            AppError::new(
                CREDENTIAL_ACCESS_FAILED,
                haven_common::ErrorKind::Validation,
                "凭据 target 命名空间非法",
                false,
            )
        })?;
        // `target` 显式指定完整 target（约束格式 `haven:<provider>:<profile-id>`）；
        // `persistence=Local` 防止 Enterprise 漫游。
        let mut modifiers = HashMap::new();
        modifiers.insert("target", target.as_str());
        modifiers.insert("persistence", "Local");
        self.store
            .build("haven", "", Some(&modifiers))
            .map_err(map_error("构建凭据条目"))
    }
}

#[async_trait::async_trait]
impl CredentialStore for WindowsCredentialStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        let entry = self.build_entry(target)?;
        entry
            .set_secret(secret.expose().as_bytes())
            .map_err(map_error("写入凭据"))
    }

    async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        let entry = self.build_entry(target)?;
        match entry.get_secret() {
            Ok(buf) => {
                // S-02 修复：原 Vec 直接移入 from_utf8，失败时经 into_bytes()
                // 取回同一缓冲区再 zeroize，避免 clone 副本被普通 Drop。
                match String::from_utf8(buf) {
                    Ok(text) => Ok(Some(SecretString::new(text))),
                    Err(err) => {
                        let mut bytes = err.into_bytes();
                        bytes.zeroize();
                        Err(credential_error("凭据内容不是合法 UTF-8", false))
                    }
                }
            }
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(map_error("读取凭据")(e)),
        }
    }

    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
        let entry = self.build_entry(target)?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(e) => Err(map_error("删除凭据")(e)),
        }
    }
}

fn credential_error(msg: impl Into<String>, retryable: bool) -> AppError {
    AppError::new(
        CREDENTIAL_ACCESS_FAILED,
        haven_common::ErrorKind::Security,
        msg,
        retryable,
    )
}

/// S-04：错误分类——不再把所有系统错误无差别标 retryable=false。
/// 平台运行失败与存储访问失败（锁、暂时性平台错误）允许重试；
/// 确定性错误（无条目、参数非法、数据格式问题）不可重试。
fn retryable_for(e: &KeyringError) -> bool {
    matches!(
        e,
        KeyringError::PlatformFailure(_) | KeyringError::NoStorageAccess(_)
    )
}

fn map_error(op: &'static str) -> impl Fn(KeyringError) -> AppError {
    move |e| {
        let retryable = retryable_for(&e);
        credential_error(format!("{op}失败"), retryable).with_source(e)
    }
}

impl std::fmt::Debug for WindowsCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsCredentialStore")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroize;

    /// 生成一次测试专用的唯一 target，并保证结束后清理。
    fn unique_ref(scope: &str) -> CredentialRef {
        let uuid = uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext));
        CredentialRef::new_scoped("test", &format!("{scope}-{uuid}")).unwrap()
    }

    /// 测试凭据 RAII 清理（防 cmdkey 残留）：无论断言成败，Drop 时删除系统凭据。
    struct TestCredentialGuard {
        store: Option<WindowsCredentialStore>,
        target: CredentialRef,
    }

    impl TestCredentialGuard {
        fn new(target: CredentialRef) -> Self {
            let store = WindowsCredentialStore::new().expect("store init");
            Self {
                store: Some(store),
                target,
            }
        }
    }

    impl Drop for TestCredentialGuard {
        fn drop(&mut self) {
            // 在独立线程中删除：Drop 可能处于 tokio 测试的异步上下文，
            // 原地 block_on 会触发 runtime-in-runtime panic（tokio 禁止）。
            let Some(store) = self.store.take() else {
                return;
            };
            let target = self.target.clone();
            let _ = std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    eprintln!("[cred-guard] runtime 创建失败，{} 未清理", target.as_str());
                    return;
                };
                // 删除 + 轮询确认收敛（DEBT-CRED-001：删除存在传播延迟/偶发失败），
                // 最多 5 次重试；最终失败写入诊断日志（测试通过时 stderr 会被 cargo 吞掉）。
                let mut last_error = String::new();
                for attempt in 0..5 {
                    if let Err(e) = rt.block_on(store.delete(&target)) {
                        last_error = format!("{} ({})", e.code().as_str(), e.user_message());
                        eprintln!(
                            "[cred-guard] 清理 {} 第 {} 次失败: {}",
                            target.as_str(),
                            attempt + 1,
                            last_error
                        );
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                    // 删除语义级确认：再次 delete 返回 Ok(false)=NoEntry 才说明真的删掉了
                    //（get 确认受 LSA 缓存方向影响，不可靠；NoEntry 是权威终态）。
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    match rt.block_on(store.delete(&target)) {
                        Ok(false) => return,
                        Ok(true) => {
                            last_error = "第二次 delete 仍命中（首次删除未生效）".to_string();
                        }
                        Err(e) => {
                            last_error = format!(
                                "确认删除错误: {} ({})",
                                e.code().as_str(),
                                e.user_message()
                            );
                        }
                    }
                }
                let msg = format!(
                    "[cred-guard] {} 清理最终失败: {last_error}",
                    target.as_str()
                );
                eprintln!("{msg}");
                // 诊断日志：优先环境变量 TEMP，缺省退回当前目录（AG-V2 note：不硬编码用户路径）。
                let log_dir = std::env::var("TEMP").unwrap_or_else(|_| ".".into());
                let log_path = std::path::Path::new(&log_dir).join("haven-cred-guard-failures.log");
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(f, "{msg}");
                }
            })
            .join();
        }
    }

    #[tokio::test]
    async fn roundtrip_set_get_delete() {
        let store = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("roundtrip");
        let secret = SecretString::new("s3cr3t-value");

        store.set(&target, &secret).await.expect("set");
        let _guard = TestCredentialGuard::new(target.clone());
        let read = store.get(&target).await.expect("get");
        assert_eq!(read.expect("secret exists").expose(), "s3cr3t-value");

        store.delete(&target).await.expect("delete");
        // 已知平台行为（DEBT-CRED-001 登记）：CredDeleteW 成功后，LSA 凭据缓存
        // 可能短暂仍可读（最终一致）。轮询重读直到收敛，避免把平台传播延迟
        // 误判为测试失败；超时后附带脱敏 target 前缀诊断。
        let mut remaining = 50;
        loop {
            let after = store.get(&target).await.expect("get after delete");
            if after.is_none() {
                break;
            }
            remaining -= 1;
            if remaining == 0 {
                // 测试 target 为随机生成（无敏感信息），可直接作为诊断。
                panic!(
                    "删除后凭据持续可读（op=roundtrip, target={}）",
                    target.as_str()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }

    #[tokio::test]
    async fn overwrite_replaces_value() {
        let store = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("overwrite");

        store
            .set(&target, &SecretString::new("first"))
            .await
            .unwrap();
        let _guard = TestCredentialGuard::new(target.clone());
        store
            .set(&target, &SecretString::new("second"))
            .await
            .unwrap();
        let read = store.get(&target).await.unwrap().unwrap();
        assert_eq!(read.expose(), "second");
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let store = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("missing");
        let read = store.get(&target).await.unwrap();
        assert!(read.is_none(), "不存在的 target 应返回 None");
    }

    #[tokio::test]
    async fn delete_missing_is_idempotent() {
        let store = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("missing-delete");
        assert!(!store.delete(&target).await.unwrap(), "不存在应返回 false");
        assert!(!store.delete(&target).await.unwrap(), "重复删除仍 false");
    }

    #[tokio::test]
    async fn cross_instance_reads_same_credential() {
        let store_a = WindowsCredentialStore::new().unwrap();
        let store_b = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("cross-instance");

        store_a
            .set(&target, &SecretString::new("shared-secret"))
            .await
            .unwrap();
        let _guard = TestCredentialGuard::new(target.clone());
        let read = store_b.get(&target).await.unwrap().expect("跨实例可读");
        assert_eq!(read.expose(), "shared-secret");
    }

    #[tokio::test]
    async fn no_residue_after_delete() {
        let store = WindowsCredentialStore::new().unwrap();
        let target = unique_ref("residue-check");

        store
            .set(&target, &SecretString::new("temp"))
            .await
            .unwrap();
        // RAII guard：set 后立即注册，测试任何路径（含断言失败）都保证清理（AG-V2 note）。
        let _guard = TestCredentialGuard::new(target.clone());
        store.delete(&target).await.unwrap();
        // 功能断言：删除后经另一实例读不到（CredReadW 交叉检查的等价物）。
        // LSA 缓存传播延迟（DEBT-CRED-001）容忍：轮询至收敛。
        // 清理的最终确认由 guard 的"二次 delete 返回 NoEntry"权威语义负责。
        let store2 = WindowsCredentialStore::new().unwrap();
        let mut remaining = 50;
        loop {
            if store2.get(&target).await.unwrap().is_none() {
                break;
            }
            remaining -= 1;
            assert!(remaining > 0, "删除后另一实例仍可读到凭据");
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }

    #[test]
    fn secret_buffer_zeroized_after_utf8_failure_path() {
        // S-02 修复：原 Vec 移入 from_utf8，失败路径经 into_bytes() 取回同一缓冲区并清零，
        // 不产生被普通 Drop 的未清零副本（旧实现 clone 后被 FromUtf8Error 持有）。
        let buf = vec![0xFFu8, 0xFE, 0x01];
        match String::from_utf8(buf) {
            Ok(_) => panic!("应为非法 UTF-8"),
            Err(err) => {
                let mut bytes = err.into_bytes();
                assert_eq!(bytes, vec![0xFF, 0xFE, 0x01]);
                bytes.zeroize();
                assert!(bytes.iter().all(|b| *b == 0));
            }
        }
    }
}
