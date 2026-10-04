//! Safe directory identity projection and removal; browsing/importing is owned by CloudBrowseService.
use super::credential_session::is_canonical_uuid;
use super::service::CloudStorageService;
use super::state::CloudFolderRemoveOutcome;
use super::views::CloudFolderView;
use haven_common::{AppError, validation};
use haven_domain::ids::StorageLocationId;

impl CloudStorageService {
    pub async fn folder_binding(
        &self,
        location_id: &str,
    ) -> Result<Option<CloudFolderView>, AppError> {
        let location_id = parse_location_id(location_id)?;
        Ok(self
            .repo
            .get_folder(location_id)
            .await?
            .map(|binding| CloudFolderView {
                location_id: binding.location_id.to_string(),
                account_id: binding.account_id,
                created_at_ms: binding.created_at.0,
            }))
    }

    /// Purge application indexes only; remote files and shared account credentials remain.
    pub async fn remove_folder(&self, location_id: &str) -> Result<bool, AppError> {
        let location_id = parse_location_id(location_id)?;
        Ok(matches!(
            self.repo.remove_folder(location_id).await?,
            CloudFolderRemoveOutcome::Removed
        ))
    }
}

fn parse_location_id(raw: &str) -> Result<StorageLocationId, AppError> {
    if !is_canonical_uuid(raw) {
        return Err(validation("云盘位置 ID 不合法"));
    }
    raw.parse().map_err(|_| validation("云盘位置 ID 不合法"))
}
