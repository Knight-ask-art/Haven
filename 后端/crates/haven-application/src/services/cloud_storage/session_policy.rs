//! PDF cloud admission shared by Resource, Session and the desktop protocol.
use super::state::CloudObjectSnapshot;
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{MediaType, ResourceType, StorageProviderType, StorageStatus};
use haven_domain::ids::MediaItemId;

/// 云盘只读 PDF 绑定的唯一准入判据（Session 准备 / Resource 能力投影 / 协议重授权共用）。
///
/// 三方身份必须逐字段一致：Resource 行、StorageLocation 行与 `cloud_object_bindings`
/// 快照。Provider file ID 绝不参与比较，也不进入 locator —— locator 的 `object_id`
/// 只能是内部对象 UUID，且 `path_hint` 必须为空（它会被本地路径解析消费）。
pub fn cloud_session_binding_matches(
    media_item_id: Option<MediaItemId>,
    media_type: MediaType,
    resource: &Resource,
    storage: &StorageLocation,
    snapshot: &CloudObjectSnapshot,
) -> bool {
    let object = &snapshot.object;
    // 目录 / 账户事实：必须是同一个仍处于 Connected 的 Google Drive 位置，
    // 且位置行本身不携带凭据（本切片凭据只在账户行）。
    if snapshot.location_status != StorageStatus::Connected
        || storage.status != StorageStatus::Connected
        || snapshot.account_generation <= 0
        || snapshot.account_id != snapshot.folder.account_id
        || snapshot.folder.location_id != object.location_id
        || storage.id != object.location_id
        || storage.provider_type != StorageProviderType::GoogleDrive
        || storage.root_ref != snapshot.folder.root_ref
        || storage.credential_ref.is_some()
    {
        return false;
    }
    // 资源身份：行、条目、位置、类型与格式都必须与快照一致。
    if resource.id != object.resource_id
        || resource.media_item_id != object.media_item_id
        || media_item_id.is_some_and(|id| id != object.media_item_id)
        || resource.storage_location_id != Some(object.location_id)
        || resource.source_id.is_some()
        || resource.size != Some(object.size_bytes)
        || resource.resource_type != ResourceType::PublicationFile
        || !is_pdf_mime(resource.mime_type.as_deref())
    {
        return false;
    }
    // 本切片只把「PDF 文档」投影成云盘在线会话。
    if media_type != MediaType::Document {
        return false;
    }
    matches!(
        &resource.locator,
        ResourceLocator::StorageObject { provider_id, object_id, path_hint: None }
            if *provider_id == object.location_id && object_id == &object.id
    )
}

fn is_pdf_mime(mime: Option<&str>) -> bool {
    mime.and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/pdf"))
}
