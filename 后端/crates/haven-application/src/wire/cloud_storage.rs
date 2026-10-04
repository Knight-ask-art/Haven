//! 云盘（Google Drive 只读切片）Wire DTO（契约 §11–§12、§22）。
//!
//! 边界——线上类型只承载**内部 UUID** 与**不透明浏览句柄**：每个响应都是 Application
//! 视图的逐字段投影，请求方向也没有可传入 Provider 身份、原始分页 token、凭据引用或
//! 凭据值的字段，因此前端既带不进远端标识，也无法重放 token。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::services::cloud_storage::browse::{CloudBrowseEntryView, CloudBrowsePage};
use crate::services::cloud_storage::views::{
    CloudAccountView, CloudConnectAttemptView, CloudConnectBeginRequest as CloudConnectBeginInput,
    CloudConnectPollView, CloudConnectStatus, CloudFolderView, CloudObjectView,
};

/// 授权尝试状态（闭合集合，与 Application `CloudConnectStatus` 逐项对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum CloudConnectStatusDto {
    Pending,
    Completing,
    Authorized,
    Cancelled,
    Expired,
    Failed,
    Completed,
}

impl From<CloudConnectStatus> for CloudConnectStatusDto {
    fn from(value: CloudConnectStatus) -> Self {
        match value {
            CloudConnectStatus::Pending => Self::Pending,
            CloudConnectStatus::Completing => Self::Completing,
            CloudConnectStatus::Authorized => Self::Authorized,
            CloudConnectStatus::Cancelled => Self::Cancelled,
            CloudConnectStatus::Expired => Self::Expired,
            CloudConnectStatus::Failed => Self::Failed,
            CloudConnectStatus::Completed => Self::Completed,
        }
    }
}

/// 账户投影：内部 UUID 与非敏感事实（无 Provider 身份、无凭据引用、无邮箱）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudAccountDto {
    pub id: String,
    pub display_name: String,
    pub connected: bool,
    /// 账户代际：断开与重新授权都会前进；写操作必须带着它做 CAS。
    #[ts(type = "number")]
    pub generation: i64,
}

impl From<CloudAccountView> for CloudAccountDto {
    fn from(value: CloudAccountView) -> Self {
        let CloudAccountView {
            id,
            display_name,
            connected,
            generation,
        } = value;
        Self {
            id,
            display_name,
            connected,
            generation,
        }
    }
}

/// `cloud_account_connect_begin` 响应：本地授权尝试句柄与到期时刻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudConnectAttemptDto {
    pub attempt_id: String,
    #[ts(type = "number")]
    pub expires_at_ms: i64,
}

impl From<CloudConnectAttemptView> for CloudConnectAttemptDto {
    fn from(value: CloudConnectAttemptView) -> Self {
        Self {
            attempt_id: value.attempt_id,
            expires_at_ms: value.expires_at_ms,
        }
    }
}

/// `cloud_account_connect_poll` 响应：轮询到的状态与到期时刻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudConnectPollDto {
    pub status: CloudConnectStatusDto,
    #[ts(type = "number")]
    pub expires_at_ms: i64,
}

impl From<CloudConnectPollView> for CloudConnectPollDto {
    fn from(value: CloudConnectPollView) -> Self {
        Self {
            status: value.status.into(),
            expires_at_ms: value.expires_at_ms,
        }
    }
}

/// 目录绑定投影：`locationId` 是内部位置 UUID。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudFolderDto {
    pub location_id: String,
    pub account_id: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

impl From<CloudFolderView> for CloudFolderDto {
    fn from(value: CloudFolderView) -> Self {
        let CloudFolderView {
            location_id,
            account_id,
            created_at_ms,
        } = value;
        Self {
            location_id,
            account_id,
            created_at_ms,
        }
    }
}

/// 浏览条目：只有不透明句柄与展示事实；`handle = null` 表示当前不可选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudBrowseEntryDto {
    pub handle: Option<String>,
    pub display_name: String,
    pub is_folder: bool,
    pub pdf_supported: bool,
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
}

impl From<CloudBrowseEntryView> for CloudBrowseEntryDto {
    fn from(value: CloudBrowseEntryView) -> Self {
        Self {
            handle: value.handle,
            display_name: value.display_name,
            is_folder: value.is_folder,
            pdf_supported: value.pdf_supported,
            size_bytes: value.size_bytes,
        }
    }
}

/// 一页浏览结果：条目 + 一次性光标 + 本页所属目录句柄（三者都只是句柄）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudBrowsePageDto {
    pub folder_handle: Option<String>,
    pub entries: Vec<CloudBrowseEntryDto>,
    pub next_cursor: Option<String>,
}

impl From<CloudBrowsePage> for CloudBrowsePageDto {
    fn from(value: CloudBrowsePage) -> Self {
        Self {
            folder_handle: value.folder_handle,
            entries: value
                .entries
                .into_iter()
                .map(CloudBrowseEntryDto::from)
                .collect(),
            next_cursor: value.next_cursor,
        }
    }
}

/// 导入对象投影：全部是内部 UUID（对象 / 媒体条目 / 资源）与展示事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudObjectDto {
    pub object_id: String,
    pub media_item_id: String,
    pub resource_id: String,
    pub display_name: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

impl From<CloudObjectView> for CloudObjectDto {
    fn from(value: CloudObjectView) -> Self {
        Self {
            object_id: value.object_id,
            media_item_id: value.media_item_id,
            resource_id: value.resource_id,
            display_name: value.display_name,
            size_bytes: value.size_bytes,
        }
    }
}

/// `cloud_storage_list` 响应：本机是否已配置授权 + 全部账户（含断开墓碑）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudStorageListDto {
    pub oauth_available: bool,
    pub accounts: Vec<CloudAccountDto>,
}

/// `cloud_account_connect_begin` 请求：`accountId` 省略或 `null` 即新连接。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudConnectBeginRequest {
    #[serde(default)]
    #[ts(optional)]
    pub account_id: Option<String>,
}

impl From<CloudConnectBeginRequest> for CloudConnectBeginInput {
    fn from(value: CloudConnectBeginRequest) -> Self {
        Self {
            account_id: value.account_id,
        }
    }
}

/// `cloud_account_disconnect` 请求：内部账户 UUID + 调用方读到的代际（CAS）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudAccountDisconnectRequest {
    pub account_id: String,
    #[ts(type = "number")]
    pub expected_generation: i64,
}

/// `cloud_register_folder` 请求：不透明目录句柄 + 用户起的显示名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudRegisterFolderRequest {
    pub folder_handle: String,
    pub display_name: String,
}

/// `cloud_import_pdf` 请求：内部位置 UUID + 该目录下浏览得到的文件句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CloudImportPdfRequest {
    pub location_id: String,
    pub file_handle: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_rs::Config;

    /// 本模块全部类型的 TS 声明；下面的脱敏断言只扫这一份投影。
    fn declarations() -> String {
        let config = Config::default();
        [
            CloudConnectStatusDto::export_to_string(&config).unwrap(),
            CloudAccountDto::export_to_string(&config).unwrap(),
            CloudConnectAttemptDto::export_to_string(&config).unwrap(),
            CloudConnectPollDto::export_to_string(&config).unwrap(),
            CloudFolderDto::export_to_string(&config).unwrap(),
            CloudBrowseEntryDto::export_to_string(&config).unwrap(),
            CloudBrowsePageDto::export_to_string(&config).unwrap(),
            CloudObjectDto::export_to_string(&config).unwrap(),
            CloudStorageListDto::export_to_string(&config).unwrap(),
            CloudConnectBeginRequest::export_to_string(&config).unwrap(),
            CloudAccountDisconnectRequest::export_to_string(&config).unwrap(),
            CloudRegisterFolderRequest::export_to_string(&config).unwrap(),
            CloudImportPdfRequest::export_to_string(&config).unwrap(),
        ]
        .join("\n")
    }

    #[test]
    fn declarations_expose_only_internal_ids_and_opaque_handles() {
        // ts-rs 的生成器署名带公开文档链接；它不是 Wire 字段或值。
        let out = declarations()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "driveId",
            "fileId",
            "folderId",
            "pageToken",
            "credentialRef",
            "email",
            "accessToken",
            "refreshToken",
            "https://",
        ] {
            assert!(
                !out.contains(forbidden),
                "云盘 wire 类型不得包含 {forbidden}"
            );
        }
        assert!(out.contains("handle: string | null"));
        assert!(out.contains("nextCursor: string | null"));
        assert!(out.contains("oauthAvailable: boolean"));
        assert!(out.contains("accountId?: string"));
        assert!(out.contains(
            "export type CloudConnectStatusDto = \"pending\" | \"completing\" | \"authorized\" \
             | \"cancelled\" | \"expired\" | \"failed\" | \"completed\";"
        ));
    }

    #[test]
    fn serde_shapes_are_camel_case_and_requests_reject_extra_fields() {
        let poll = CloudConnectPollDto::from(CloudConnectPollView {
            status: CloudConnectStatus::Completing,
            expires_at_ms: 1,
        });
        assert_eq!(
            serde_json::to_string(&poll).unwrap(),
            r#"{"status":"completing","expiresAtMs":1}"#
        );
        assert_eq!(
            serde_json::from_str::<CloudConnectBeginRequest>("{}")
                .unwrap()
                .account_id,
            None
        );
        assert!(
            serde_json::from_str::<CloudConnectBeginRequest>(r#"{"driveAccountId":"x"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<CloudAccountDisconnectRequest>(
                r#"{"accountId":"x","expectedGeneration":3,"fileId":"c"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn projections_keep_handle_only_identity_and_internal_uuids() {
        let page = CloudBrowsePage {
            folder_handle: Some("6f9a1d20-3c5b-4e77-9a11-2d4c8b0e7f51".into()),
            entries: vec![CloudBrowseEntryView {
                handle: None,
                display_name: "已回收站.pdf".into(),
                is_folder: false,
                pdf_supported: false,
                size_bytes: None,
            }],
            next_cursor: None,
        };
        assert_eq!(
            serde_json::to_string(&CloudBrowsePageDto::from(page)).unwrap(),
            r#"{"folderHandle":"6f9a1d20-3c5b-4e77-9a11-2d4c8b0e7f51","entries":[{"handle":null,"displayName":"已回收站.pdf","isFolder":false,"pdfSupported":false,"sizeBytes":null}],"nextCursor":null}"#
        );
        let object = CloudObjectDto::from(CloudObjectView {
            object_id: "0b6b1f3a-4c1e-4a2b-8f0e-2b7c9d5a1e30".into(),
            media_item_id: "m".into(),
            resource_id: "r".into(),
            display_name: "书.pdf".into(),
            size_bytes: 2048,
        });
        assert_eq!(
            serde_json::to_string(&object).unwrap(),
            r#"{"objectId":"0b6b1f3a-4c1e-4a2b-8f0e-2b7c9d5a1e30","mediaItemId":"m","resourceId":"r","displayName":"书.pdf","sizeBytes":2048}"#
        );
    }
}
