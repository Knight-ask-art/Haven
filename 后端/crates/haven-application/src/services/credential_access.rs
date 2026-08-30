//! CredentialAccessService：Provider Profile 凭据
//! （契约 §36.5 / CONTRACT-V02-CREDENTIAL-PROFILE-001，WebDAV 前置）。
//!
//! 规则：
//! - Secret 单向写入 Windows Credential Store（ADR-001）；`credentialRef`、
//!   target 名与 secret 派生材料禁止出 IPC、日志、Fixture。
//! - `profileId=null` → 默认 profile（"default"）；空白 profileId → INVALID_ARGUMENT。
//! - 空 secret → INVALID_ARGUMENT；target 字符校验按 ADR-001（控制字符/冒号/
//!   反斜杠/空白拒绝），失败统一映射为稳定 `INVALID_ARGUMENT`。
//! - `credential_status` 只返回 configured 布尔事实；凭据存储不提供写入时间，
//!   `updatedAt` 恒为 null（诚实缺省，不伪造时间）。
//! - `credential_delete` 幂等：不存在视为成功。
//!
//! 与既有 `CredentialDeletionService` 的关系：后者编排 StorageLocation 绑定凭据的
//! 删除顺序（S-04）；本服务面向 Provider Profile 凭据（无 DB 引用行），直接走
//! CredentialStore 端口，不触碰 StorageLocationRepository。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

use crate::wire::{
    CredentialDeleteRequest, CredentialProviderDto, CredentialSetRequest, CredentialStatusDto,
    CredentialStatusRequest,
};

/// 未指定 profileId 时的默认 profile。
pub const DEFAULT_PROFILE_ID: &str = "default";

/// Provider Profile 凭据服务。
#[derive(Clone)]
pub struct CredentialAccessService {
    store: Arc<dyn CredentialStore>,
}

impl CredentialAccessService {
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }

    /// `credential_status`：只暴露配置与否的事实投影。
    pub async fn status(
        &self,
        request: CredentialStatusRequest,
    ) -> Result<CredentialStatusDto, AppError> {
        let target = self.scoped_ref(request.provider, request.profile_id.as_deref())?;
        let configured = self.store.get(&target).await?.is_some();
        Ok(CredentialStatusDto {
            configured,
            updated_at: None,
        })
    }

    /// `credential_set`：幂等覆盖写入。Secret 在本调用栈内以可清零类型存在。
    pub async fn set(&self, request: CredentialSetRequest) -> Result<(), AppError> {
        if request.secret.is_empty() {
            return Err(invalid_argument("凭据内容不能为空"));
        }
        let target = self.scoped_ref(request.provider, request.profile_id.as_deref())?;
        let secret = SecretString::new(request.secret);
        self.store.set(&target, &secret).await
    }

    /// `credential_delete`：幂等删除；不存在视为成功。
    pub async fn delete(&self, request: CredentialDeleteRequest) -> Result<(), AppError> {
        let target = self.scoped_ref(request.provider, request.profile_id.as_deref())?;
        let _deleted = self.store.delete(&target).await?;
        Ok(())
    }

    fn scoped_ref(
        &self,
        provider: CredentialProviderDto,
        profile_id: Option<&str>,
    ) -> Result<CredentialRef, AppError> {
        let profile = match profile_id {
            None => DEFAULT_PROFILE_ID,
            Some(id) if id.trim().is_empty() => {
                return Err(invalid_argument("profileId 不能为空白"));
            }
            Some(id) => id,
        };
        CredentialRef::new_scoped(provider.as_str(), profile)
            .map_err(|err| invalid_argument(err.user_message()))
    }
}

fn invalid_argument(message: impl Into<String>) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::credential::CredentialStore;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore {
        entries: Mutex<HashMap<String, String>>,
    }

    #[async_trait::async_trait]
    impl CredentialStore for MemoryStore {
        async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(target.as_str().to_owned(), secret.expose().to_owned());
            Ok(())
        }

        async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(target.as_str())
                .map(|value| SecretString::new(value.clone())))
        }

        async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(target.as_str())
                .is_some())
        }
    }

    fn service() -> CredentialAccessService {
        CredentialAccessService::new(Arc::new(MemoryStore::default()))
    }

    #[tokio::test]
    async fn default_profile_roundtrip_and_idempotent_delete() {
        let service = service();

        let before = service
            .status(CredentialStatusRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
            })
            .await
            .unwrap();
        assert!(!before.configured);
        assert!(before.updated_at.is_none(), "凭据存储不提供写入时间");

        service
            .set(CredentialSetRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
                secret: "test-secret-value".into(),
            })
            .await
            .unwrap();
        let after_set = service
            .status(CredentialStatusRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
            })
            .await
            .unwrap();
        assert!(after_set.configured);

        service
            .delete(CredentialDeleteRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
            })
            .await
            .unwrap();
        let after_delete = service
            .status(CredentialStatusRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
            })
            .await
            .unwrap();
        assert!(!after_delete.configured);

        // 幂等：重复删除不存在凭据仍成功。
        service
            .delete(CredentialDeleteRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn named_profiles_are_isolated() {
        let service = service();
        service
            .set(CredentialSetRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: Some("nas-main".into()),
                secret: "a".into(),
            })
            .await
            .unwrap();
        let other = service
            .status(CredentialStatusRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: Some("nas-backup".into()),
            })
            .await
            .unwrap();
        assert!(!other.configured, "不同 profile 必须互相隔离");
    }

    #[tokio::test]
    async fn invalid_inputs_map_to_stable_invalid_argument() {
        let service = service();
        for request in [
            CredentialSetRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: None,
                secret: String::new(),
            },
            CredentialSetRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: Some("  ".into()),
                secret: "x".into(),
            },
            CredentialSetRequest {
                provider: CredentialProviderDto::Webdav,
                profile_id: Some("bad\r\nprofile".into()),
                secret: "x".into(),
            },
        ] {
            let err = service.set(request).await.unwrap_err();
            assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
        }
    }
}
