//! 外观应用服务（Appearance Stage 1B）。
//!
//! 这个服务把外观能力拆成两条真实事实流：
//! - `settings` 的 `appearance` section 保存主题选择、自定义调色板、壁纸选择和字体资产 ID；
//! - `AppearanceRepository` 保存资产元数据与首页布局，布局使用独立 revision 做 CAS。
//!
//! 文件导入只接受来自受控 Native 文件选择器的路径。路径停留在 Application/Infrastructure
//! 内部，永不进入 Domain、Wire 或前端状态；Domain 与 UI 只处理不透明资产 ID。
//!
//! ## 失败时的孤儿状态
//!
//! 资产由两部分组成：登记行（`appearance_assets`）与字节（受控存储里的文件）。
//! 两部分无法在同一个事务里更新，所以每条失败路径都必须明确**哪一半可能残留**，
//! 并且只允许残留「可解释」的那一半：
//! - 导入：字节先落盘，登记行后写入。登记行写失败时立刻删除字节；即使清理也失败，
//!   残留的也只是一个没有任何登记行引用的文件（ID 是新生成的 UUID，不可能被复用）。
//!   反过来（先写登记行）会留下「列表里有、字节没有」的不可用资产，那才是无法解释的状态。
//! - 删除：先删登记行（资产存在的唯一事实源），再删字节。登记行删掉而字节残留，
//!   同样只是垃圾文件；删除结果会如实报告 `file_removed=false`，让残留可被观测。

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::appearance::{
    APPEARANCE_ASSET_IN_USE, AppearanceAssetId, AppearanceAssetKind, AppearanceAssetMetadata,
    AppearanceAssetReference, AssetValidationState, HomeLayout, OverviewLayout,
    normalize_asset_display_name,
};
use haven_domain::contracts::AppearanceRepository;

/// 受控文件导入实现。具体实现位于 Infrastructure，可使用应用管理的资源目录。
pub trait AppearanceAssetStorage: Send + Sync {
    /// 校验并把文件复制到应用管理目录，返回实际字节数。
    fn import_file(
        &self,
        source: &Path,
        id: AppearanceAssetId,
        kind: AppearanceAssetKind,
    ) -> Result<u64, AppError>;
    /// 删除应用管理目录中的对应字节；元数据删除失败时由调用方决定是否保留。
    fn remove_file(&self, id: AppearanceAssetId) -> Result<(), AppError>;

    /// 打开已登记资产的受控字节流。
    ///
    /// 这个句柄只在 Tauri 的 `haven-resource` 协议内部流转，不会进入 Wire、React
    /// 状态或任何用户可见 DTO。默认实现让只关心导入/删除的测试替身无需伪造读取能力。
    fn open_file(&self, _id: AppearanceAssetId) -> Result<AppearanceAssetFile, AppError> {
        Err(AppError::new(
            "APPEARANCE_ASSET_UNAVAILABLE",
            ErrorKind::NotFound,
            "外观资产字节不可用",
            false,
        ))
    }
}

/// 外观资产的内部读取句柄。路径只用于协议层选择 MIME，不得序列化或返回给 WebView。
pub struct AppearanceAssetFile {
    pub file: File,
    pub path: PathBuf,
}

/// 经 Application 层重新授权后的外观资产读取事实。
pub struct AppearanceAssetRead {
    pub kind: AppearanceAssetKind,
    pub file: File,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppearanceAssetImportResult {
    pub metadata: AppearanceAssetMetadata,
}

/// 资产删除结果。两部分事实分开报告，避免「登记行删了、字节没删」被当成整体失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppearanceAssetDeleteResult {
    /// 登记行是否被删除。这是「资产是否存在」的判定，也是读路径的唯一依据。
    pub deleted: bool,
    /// 字节是否同时清理成功。`deleted && !file_removed` 表示留下一个无登记的垃圾文件。
    pub file_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeLayoutResult {
    pub layout: HomeLayout,
    pub revision: Option<String>,
    pub changed: bool,
}

/// 总览布局的读写结果。
///
/// 与 [`HomeLayoutResult`] 分开一个类型（而不是复用一个泛型）：两者的 `layout` 是两套
/// 不同的领域值，合成一个类型只会让「保存的是哪一份布局」在调用点变得含糊。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OverviewLayoutResult {
    pub layout: OverviewLayout,
    pub revision: Option<String>,
    pub changed: bool,
}

#[derive(Clone)]
pub struct AppearanceService {
    repository: Arc<dyn AppearanceRepository>,
    storage: Arc<dyn AppearanceAssetStorage>,
}

impl AppearanceService {
    pub fn new(
        repository: Arc<dyn AppearanceRepository>,
        storage: Arc<dyn AppearanceAssetStorage>,
    ) -> Self {
        Self {
            repository,
            storage,
        }
    }

    pub async fn list_assets(
        &self,
        kind: Option<AppearanceAssetKind>,
    ) -> Result<Vec<AppearanceAssetMetadata>, AppError> {
        self.repository.list_assets(kind).await
    }

    /// 打开资产前重新读取登记行，并要求字节处于 `validated` 状态且长度仍与登记事实一致。
    ///
    /// 资源协议每次请求都会调用这个入口，因此删除资产、登记行漂移或文件被替换后，旧的
    /// URL 不会继续获得字节。协议只接收返回的已打开文件句柄，永远不会看到本地路径。
    ///
    /// 失败分两类，语义不同、可重试性也不同：
    /// - **读不出来**（打开时或取长度时的 IO 故障：文件被占用、权限抖动、设备暂时不可用）
    ///   → [`asset_storage_failed`]，可重试。把这一类报成「资产不存在」会让页面上那个
    ///   「重试预览」变成一个注定失败的动作；
    /// - **资产不存在 / 登记行与字节不一致**（登记缺失、未校验、长度与登记事实对不上）
    ///   → [`asset_unavailable`]，不可重试。这是既成事实，重试多少次都是同一个结果。
    pub async fn open_asset(&self, id: AppearanceAssetId) -> Result<AppearanceAssetRead, AppError> {
        let metadata = self
            .repository
            .get_asset(id)
            .await?
            .ok_or_else(asset_unavailable)?;
        if metadata.state != AssetValidationState::Validated {
            return Err(asset_unavailable());
        }
        let opened = self.storage.open_file(id)?;
        let actual_size = opened
            .file
            .metadata()
            .map_err(|_| asset_storage_failed())?
            .len();
        if actual_size != metadata.byte_size {
            return Err(asset_unavailable());
        }
        Ok(AppearanceAssetRead {
            kind: metadata.kind,
            file: opened.file,
            path: opened.path,
        })
    }

    /// 导入资产：校验并复制字节，然后登记元数据。
    ///
    /// 顺序是刻意的：展示名在**任何字节落盘之前**先校验（非法展示名不该白写一次磁盘），
    /// 字节先写、登记行后写，任何一步失败都把已写的字节回滚掉，不留孤儿。
    pub async fn import_asset(
        &self,
        source: &Path,
        kind: AppearanceAssetKind,
        display_name: Option<String>,
    ) -> Result<AppearanceAssetImportResult, AppError> {
        let display_name = normalize_asset_display_name(display_name)?;
        let id = AppearanceAssetId::new();
        let byte_size = self.storage.import_file(source, id, kind)?;

        let metadata = match AppearanceAssetMetadata::new(
            id,
            kind,
            AssetValidationState::Validated,
            byte_size,
            display_name,
        ) {
            Ok(metadata) => metadata,
            Err(error) => {
                // 已写入的字节不能留在一个永远不会被登记的名字上。
                let _ = self.storage.remove_file(id);
                return Err(error);
            }
        };

        if let Err(error) = self.repository.create_asset(&metadata).await {
            // 元数据没有落库时不能留下孤儿文件。清理是尽力而为：清理本身失败也不能
            // 掩盖原始数据库错误（那是调用方真正要处理的），残留文件没有任何登记行
            // 引用，是一条可解释的垃圾记录而不是半可用资产。
            let _ = self.storage.remove_file(id);
            return Err(error);
        }
        Ok(AppearanceAssetImportResult { metadata })
    }

    /// 删除资产：先确认没有任何持久化外观设置引用它，再删登记行（唯一事实源），
    /// 最后删字节。
    ///
    /// 反过来做的话，「字节删了、登记行没删」会留下一个列表里可见但永远渲染不出来的
    /// 资产——那是读路径无法解释的状态。当前顺序最坏只留下一个无登记引用的文件，
    /// 且结果里的 `file_removed` 会如实报告它。资产不存在时不触碰任何字节（幂等）。
    ///
    /// 占用判定在 Repository 的删除事务内完成（见 [`AppearanceRepository::delete_asset`]），
    /// 因此这里拿到 `references` 非空时，登记行与字节都还没有被动过；服务层据此返回
    /// 稳定的 `APPEARANCE_ASSET_IN_USE`，而不是替用户悄悄改掉外观设置。
    pub async fn delete_asset(
        &self,
        id: AppearanceAssetId,
    ) -> Result<AppearanceAssetDeleteResult, AppError> {
        let outcome = self.repository.delete_asset(id).await?;
        if !outcome.references.is_empty() {
            return Err(asset_in_use(&outcome.references));
        }
        if !outcome.deleted {
            return Ok(AppearanceAssetDeleteResult {
                deleted: false,
                file_removed: false,
            });
        }
        // 字节清理失败不回滚登记行，也不把整体判成失败：登记行已经删除，读路径上
        // 资产确实不存在了，报失败只会让 UI 保留一个已经过期的列表项。
        Ok(AppearanceAssetDeleteResult {
            deleted: true,
            file_removed: self.storage.remove_file(id).is_ok(),
        })
    }

    pub async fn home_layout_get(&self) -> Result<HomeLayoutResult, AppError> {
        Ok(match self.repository.get_home_layout().await? {
            Some(snapshot) => HomeLayoutResult {
                layout: snapshot.layout,
                revision: Some(snapshot.revision),
                changed: false,
            },
            None => HomeLayoutResult {
                layout: HomeLayout::default(),
                revision: None,
                changed: false,
            },
        })
    }

    /// 保存首页布局（revision CAS）。
    ///
    /// 事务语义分两层：
    /// - 这里先读当前快照做**幂等比较**与友好错误，真正的一致性由
    ///   `cas_save_home_layout` 在单一事务里再校验一次 revision 保证（读与写之间
    ///   被其他窗口插入时，CAS 会失败并返回 `REVISION_CONFLICT`，零写入）；
    /// - 写入前统一规范化（模块按 `order` 升序），因此「同一份布局、不同数组顺序」
    ///   的重复提交是幂等的，不会每次刷出一个新 revision。
    pub async fn home_layout_save(
        &self,
        expected_revision: Option<&str>,
        layout: HomeLayout,
    ) -> Result<HomeLayoutResult, AppError> {
        layout.validate()?;
        let layout = layout.canonicalized();
        let current = self.repository.get_home_layout().await?;
        let current_revision = current.as_ref().map(|snapshot| snapshot.revision.as_str());
        ensure_revision_matches(current_revision, expected_revision)?;

        if current
            .as_ref()
            .is_some_and(|snapshot| snapshot.layout == layout)
        {
            return Ok(HomeLayoutResult {
                layout,
                revision: current.map(|snapshot| snapshot.revision),
                changed: false,
            });
        }

        let revision = next_revision();
        if !self
            .repository
            .cas_save_home_layout(expected_revision, &layout, &revision, UtcMillis::now())
            .await?
        {
            return Err(revision_conflict());
        }
        Ok(HomeLayoutResult {
            layout,
            revision: Some(revision),
            changed: true,
        })
    }

    /// 重置首页布局：删除已保存的自定义，回到领域默认布局。
    ///
    /// 「从未保存」与「已保存的空布局」在存储层由 meta 行区分（见 `HomeLayoutSnapshot`），
    /// 所以重置只有在确实存在保存状态时才算变化；从未保存过时是幂等空操作。
    pub async fn home_layout_reset(
        &self,
        expected_revision: Option<&str>,
    ) -> Result<HomeLayoutResult, AppError> {
        let current = self.repository.get_home_layout().await?;
        let current_revision = current.as_ref().map(|snapshot| snapshot.revision.as_str());
        ensure_revision_matches(current_revision, expected_revision)?;
        if current.is_none() {
            return Ok(HomeLayoutResult {
                layout: HomeLayout::default(),
                revision: None,
                changed: false,
            });
        }
        if !self
            .repository
            .cas_reset_home_layout(expected_revision)
            .await?
        {
            return Err(revision_conflict());
        }
        Ok(HomeLayoutResult {
            layout: HomeLayout::default(),
            revision: None,
            changed: true,
        })
    }

    /// 读取总览布局；从未自定义过时返回领域默认布局 + `revision = null`。
    pub async fn overview_layout_get(&self) -> Result<OverviewLayoutResult, AppError> {
        Ok(match self.repository.get_overview_layout().await? {
            Some(snapshot) => OverviewLayoutResult {
                layout: snapshot.layout,
                revision: Some(snapshot.revision),
                changed: false,
            },
            None => OverviewLayoutResult {
                layout: OverviewLayout::default(),
                revision: None,
                changed: false,
            },
        })
    }

    /// 保存总览布局（revision CAS）。语义与 [`Self::home_layout_save`] 逐条相同，只是作用在
    /// 另一份布局与另一张表上：先规范化再比对，同一份布局的重复提交是幂等的
    /// （`changed = false` 且 revision 不变），真正的并发一致性由 `cas_save_overview_layout`
    /// 在单一事务里保证。
    pub async fn overview_layout_save(
        &self,
        expected_revision: Option<&str>,
        layout: OverviewLayout,
    ) -> Result<OverviewLayoutResult, AppError> {
        layout.validate()?;
        let layout = layout.canonicalized();
        let current = self.repository.get_overview_layout().await?;
        let current_revision = current.as_ref().map(|snapshot| snapshot.revision.as_str());
        ensure_revision_matches(current_revision, expected_revision)?;

        if current
            .as_ref()
            .is_some_and(|snapshot| snapshot.layout == layout)
        {
            return Ok(OverviewLayoutResult {
                layout,
                revision: current.map(|snapshot| snapshot.revision),
                changed: false,
            });
        }

        let revision = next_overview_revision();
        if !self
            .repository
            .cas_save_overview_layout(expected_revision, &layout, &revision, UtcMillis::now())
            .await?
        {
            return Err(revision_conflict());
        }
        Ok(OverviewLayoutResult {
            layout,
            revision: Some(revision),
            changed: true,
        })
    }

    /// 重置总览布局：删除已保存的自定义，回到领域默认布局。从未保存过时是幂等空操作。
    pub async fn overview_layout_reset(
        &self,
        expected_revision: Option<&str>,
    ) -> Result<OverviewLayoutResult, AppError> {
        let current = self.repository.get_overview_layout().await?;
        let current_revision = current.as_ref().map(|snapshot| snapshot.revision.as_str());
        ensure_revision_matches(current_revision, expected_revision)?;
        if current.is_none() {
            return Ok(OverviewLayoutResult {
                layout: OverviewLayout::default(),
                revision: None,
                changed: false,
            });
        }
        if !self
            .repository
            .cas_reset_overview_layout(expected_revision)
            .await?
        {
            return Err(revision_conflict());
        }
        Ok(OverviewLayoutResult {
            layout: OverviewLayout::default(),
            revision: None,
            changed: true,
        })
    }
}

fn ensure_revision_matches(current: Option<&str>, expected: Option<&str>) -> Result<(), AppError> {
    if current == expected {
        Ok(())
    } else {
        Err(revision_conflict())
    }
}

fn revision_conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "外观设置已被其他窗口更新，请重新加载后再保存",
        false,
    )
}

fn asset_unavailable() -> AppError {
    AppError::new(
        "APPEARANCE_ASSET_UNAVAILABLE",
        ErrorKind::NotFound,
        "外观资产不存在或当前不可用",
        false,
    )
}

/// 读取资产字节时的**瞬时**故障：这一次读失败，重试有机会成功。
///
/// 与 [`asset_unavailable`] 分开的理由是可重试性，不是错误文案：后者描述的是既成事实
/// （资产不存在、或登记行与字节对不上），把瞬时 IO 故障也报成它，界面上那个「重试预览」
/// 就变成了一个注定失败的动作。错误码与受控存储的写路径共用同一个
/// `APPEARANCE_ASSET_STORAGE_FAILED`（同一种故障，读写两侧不该有两个名字）。
fn asset_storage_failed() -> AppError {
    AppError::new(
        "APPEARANCE_ASSET_STORAGE_FAILED",
        ErrorKind::Storage,
        "外观资产读取失败",
        true,
    )
}

/// 资产仍被持久化的外观设置引用：删除被拒绝，且零写入。
///
/// 这是 `Conflict` 而不是 `Validation`：状态本身合法，只是当前不允许——用户先改掉
/// 引用它的那项外观设置，同一次删除就会成功。
fn asset_in_use(references: &[AppearanceAssetReference]) -> AppError {
    let labels: Vec<&str> = references.iter().map(|item| item.label()).collect();
    AppError::new(
        APPEARANCE_ASSET_IN_USE,
        ErrorKind::Conflict,
        format!("该资产仍被{}使用，请先更换后再删除", labels.join("、")),
        false,
    )
}

/// 新的 CAS revision：毫秒时间戳（可读，便于诊断）+ 随机 UUID（保证唯一）。
///
/// 随机成分不是装饰：CAS 的正确性建立在「每次写入都产生一个**不同**的 revision」上。
/// 只靠时间戳的话，粗粒度时钟或时钟回拨会让两次写入拿到同一个 token，此时一个持有
/// 旧 revision 的窗口会被误判为最新，CAS 就退化成「永远通过」。长度 64 落在 046/048 的
/// `length(revision) BETWEEN 8 AND 160` 之内。
fn next_revision() -> String {
    revision_with_scope("appearance")
}

/// 总览布局的 revision。
///
/// 前缀刻意与首页布局不同：两张表的 revision 各自独立，日志里出现 `overview-…` 时
/// 不必再去查它是哪一份布局的 token。
fn next_overview_revision() -> String {
    revision_with_scope("overview")
}

/// 两份布局共用的 revision 形状（唯一事实源，避免两条生成路径各自漂移）。
fn revision_with_scope(scope: &str) -> String {
    format!(
        "{scope}-{:016x}-{}",
        UtcMillis::now().0,
        uuid::Uuid::new_v4()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use haven_domain::appearance::{
        AppearanceAssetDeleteOutcome, HOME_LAYOUT_SCHEMA_VERSION, HomeModuleId,
        HomeModulePlacement, HomeModuleSize, MAX_ASSET_DISPLAY_NAME_CHARS,
        OVERVIEW_LAYOUT_SCHEMA_VERSION, OverviewModuleId, OverviewModulePlacement,
        OverviewModuleSize,
    };
    use haven_domain::contracts::{HomeLayoutSnapshot, OverviewLayoutSnapshot};
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryRepo {
        assets: Mutex<Vec<AppearanceAssetMetadata>>,
        layout: Mutex<Option<HomeLayoutSnapshot>>,
        /// 总览布局是**另一份**状态：与 `layout` 共用一份会让「两份布局互不影响」
        /// 这条不变量在服务层测试里失去观察点。
        overview: Mutex<Option<OverviewLayoutSnapshot>>,
        /// `true` 时 `create_asset` 直接失败（模拟登记行写不进去）。
        create_asset_failure: Mutex<bool>,
        /// 模拟 `settings.appearance` 持有的资产引用（真实实现里这两个字段住在
        /// settings 行的 JSON 内，见 SqliteAppearanceRepository::delete_asset）。
        references: Mutex<Vec<(AppearanceAssetId, AppearanceAssetReference)>>,
    }

    #[async_trait]
    impl AppearanceRepository for MemoryRepo {
        async fn get_asset(
            &self,
            id: AppearanceAssetId,
        ) -> Result<Option<AppearanceAssetMetadata>, AppError> {
            Ok(self
                .assets
                .lock()
                .unwrap()
                .iter()
                .find(|asset| asset.id == id)
                .cloned())
        }

        async fn list_assets(
            &self,
            kind: Option<AppearanceAssetKind>,
        ) -> Result<Vec<AppearanceAssetMetadata>, AppError> {
            Ok(self
                .assets
                .lock()
                .unwrap()
                .iter()
                .filter(|asset| kind.is_none_or(|expected| asset.kind == expected))
                .cloned()
                .collect())
        }

        async fn create_asset(&self, asset: &AppearanceAssetMetadata) -> Result<(), AppError> {
            if *self.create_asset_failure.lock().unwrap() {
                return Err(AppError::new(
                    "DATABASE_ERROR",
                    ErrorKind::Database,
                    "保存外观资产失败",
                    true,
                ));
            }
            self.assets.lock().unwrap().push(asset.clone());
            Ok(())
        }

        async fn delete_asset(
            &self,
            id: AppearanceAssetId,
        ) -> Result<AppearanceAssetDeleteOutcome, AppError> {
            let references: Vec<AppearanceAssetReference> = self
                .references
                .lock()
                .unwrap()
                .iter()
                .filter(|(asset_id, _)| *asset_id == id)
                .map(|(_, reference)| *reference)
                .collect();
            if !references.is_empty() {
                return Ok(AppearanceAssetDeleteOutcome::blocked(references));
            }
            let mut assets = self.assets.lock().unwrap();
            let len = assets.len();
            assets.retain(|asset| asset.id != id);
            Ok(if assets.len() != len {
                AppearanceAssetDeleteOutcome::removed()
            } else {
                AppearanceAssetDeleteOutcome::missing()
            })
        }

        async fn get_home_layout(&self) -> Result<Option<HomeLayoutSnapshot>, AppError> {
            Ok(self.layout.lock().unwrap().clone())
        }

        async fn cas_save_home_layout(
            &self,
            expected_revision: Option<&str>,
            layout: &HomeLayout,
            revision: &str,
            _updated_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut current = self.layout.lock().unwrap();
            if current.as_ref().map(|snapshot| snapshot.revision.as_str()) != expected_revision {
                return Ok(false);
            }
            *current = Some(HomeLayoutSnapshot {
                layout: layout.clone(),
                revision: revision.to_owned(),
            });
            Ok(true)
        }

        async fn cas_reset_home_layout(
            &self,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let mut current = self.layout.lock().unwrap();
            if current.as_ref().map(|snapshot| snapshot.revision.as_str()) != expected_revision {
                return Ok(false);
            }
            *current = None;
            Ok(true)
        }

        async fn get_overview_layout(&self) -> Result<Option<OverviewLayoutSnapshot>, AppError> {
            Ok(self.overview.lock().unwrap().clone())
        }

        async fn cas_save_overview_layout(
            &self,
            expected_revision: Option<&str>,
            layout: &OverviewLayout,
            revision: &str,
            _updated_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut current = self.overview.lock().unwrap();
            if current.as_ref().map(|snapshot| snapshot.revision.as_str()) != expected_revision {
                return Ok(false);
            }
            *current = Some(OverviewLayoutSnapshot {
                layout: layout.clone(),
                revision: revision.to_owned(),
            });
            Ok(true)
        }

        async fn cas_reset_overview_layout(
            &self,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let mut current = self.overview.lock().unwrap();
            if current.as_ref().map(|snapshot| snapshot.revision.as_str()) != expected_revision {
                return Ok(false);
            }
            *current = None;
            Ok(true)
        }
    }

    /// 受控存储测试替身：可注入导入/清理失败，并记录真实落盘的字节。
    #[derive(Default)]
    struct MemoryStorage {
        /// `Some(错误码)` 时 `import_file` 直接失败。
        import_failure: Mutex<Option<&'static str>>,
        /// `Some(错误码)` 时 `remove_file` 直接失败（模拟文件被占用/权限不足）。
        remove_failure: Mutex<Option<&'static str>>,
        /// 已写入的字节（id + 字节数）。
        files: Mutex<Vec<(AppearanceAssetId, u64)>>,
        /// `import_file` 被调用的次数：用来断言非法展示名**没有**触碰文件系统。
        import_calls: Mutex<usize>,
    }

    impl MemoryStorage {
        fn with_remove_failure(code: &'static str) -> Self {
            Self {
                remove_failure: Mutex::new(Some(code)),
                ..Self::default()
            }
        }

        fn file_count(&self) -> usize {
            self.files.lock().unwrap().len()
        }
    }

    impl AppearanceAssetStorage for MemoryStorage {
        fn import_file(
            &self,
            _source: &Path,
            id: AppearanceAssetId,
            _kind: AppearanceAssetKind,
        ) -> Result<u64, AppError> {
            *self.import_calls.lock().unwrap() += 1;
            if let Some(code) = *self.import_failure.lock().unwrap() {
                return Err(AppError::new(
                    code,
                    ErrorKind::Validation,
                    "导入失败",
                    false,
                ));
            }
            self.files.lock().unwrap().push((id, 4));
            Ok(4)
        }

        fn remove_file(&self, id: AppearanceAssetId) -> Result<(), AppError> {
            if let Some(code) = *self.remove_failure.lock().unwrap() {
                return Err(AppError::new(code, ErrorKind::Storage, "清理失败", true));
            }
            self.files
                .lock()
                .unwrap()
                .retain(|(existing, _)| *existing != id);
            Ok(())
        }
    }

    fn service(repo: Arc<MemoryRepo>) -> AppearanceService {
        AppearanceService::new(repo, Arc::new(MemoryStorage::default()))
    }

    fn service_with_storage(
        repo: Arc<MemoryRepo>,
        storage: Arc<MemoryStorage>,
    ) -> AppearanceService {
        AppearanceService::new(repo, storage)
    }

    /// 合法放置的构造快捷方式（越界坐标/跨度会在这里 panic，测试用例本身就要求它们合法）。
    fn placement(
        module: HomeModuleId,
        size: HomeModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> HomeModulePlacement {
        HomeModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的放置必须合法")
    }

    #[tokio::test]
    async fn missing_layout_falls_back_and_empty_layout_is_real_saved_state() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());
        let initial = service.home_layout_get().await.unwrap();
        assert_eq!(initial.revision, None);
        assert!(!initial.layout.modules.is_empty());

        let saved = service
            .home_layout_save(
                None,
                HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![]).unwrap(),
            )
            .await
            .unwrap();
        assert!(saved.changed);
        assert!(saved.revision.is_some());
        let reread = service.home_layout_get().await.unwrap();
        assert!(reread.layout.modules.is_empty());
        assert_eq!(reread.revision, saved.revision);
    }

    #[tokio::test]
    async fn layout_save_is_idempotent_and_conflicts_on_stale_revision() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let layout = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Small, 0, 0, 0)
                    .unwrap(),
            ],
        )
        .unwrap();
        let first = service
            .home_layout_save(None, layout.clone())
            .await
            .unwrap();
        let second = service
            .home_layout_save(first.revision.as_deref(), layout)
            .await
            .unwrap();
        assert!(!second.changed);
        assert_eq!(second.revision, first.revision);
        let error = service
            .home_layout_save(None, HomeLayout::default())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
    }

    #[tokio::test]
    async fn reset_removes_saved_layout_and_returns_default() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let saved = service
            .home_layout_save(
                None,
                HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![]).unwrap(),
            )
            .await
            .unwrap();
        let reset = service
            .home_layout_reset(saved.revision.as_deref())
            .await
            .unwrap();
        assert!(reset.changed);
        assert_eq!(reset.revision, None);
        assert_eq!(reset.layout, HomeLayout::default());
    }

    /// 同一份布局的不同数组顺序必须判为「未变化」：写入前统一规范化，读取侧本来
    /// 就按 `sort_order` 返回，否则每次保存都会刷出一个新 revision。
    #[tokio::test]
    async fn layout_save_is_idempotent_regardless_of_module_array_order() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let first_module = placement(HomeModuleId::Continue, HomeModuleSize::Small, 0, 0, 0);
        let second_module = placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Small, 1, 0, 1);
        let third_module = placement(HomeModuleId::ShelfFavorites, HomeModuleSize::Small, 2, 0, 2);

        let out_of_order = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                third_module.clone(),
                first_module.clone(),
                second_module.clone(),
            ],
        )
        .unwrap();
        let saved = service.home_layout_save(None, out_of_order).await.unwrap();
        assert!(saved.changed);

        let order: Vec<HomeModuleId> = saved
            .layout
            .ordered_modules()
            .into_iter()
            .map(|item| item.module)
            .collect();
        assert_eq!(order, HomeModuleId::ALL.to_vec());

        let reordered = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![first_module, second_module, third_module],
        )
        .unwrap();
        let second = service
            .home_layout_save(saved.revision.as_deref(), reordered)
            .await
            .unwrap();
        assert!(!second.changed);
        assert_eq!(second.revision, saved.revision);
    }

    /// CAS 失败必须是**零写入**：已保存的布局与 revision 都保持原样。
    #[tokio::test]
    async fn stale_layout_save_writes_nothing() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let kept = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![placement(
                HomeModuleId::Continue,
                HomeModuleSize::Small,
                0,
                0,
                0,
            )],
        )
        .unwrap();
        let first = service.home_layout_save(None, kept.clone()).await.unwrap();

        let error = service
            .home_layout_save(None, HomeLayout::default())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");

        let current = service.home_layout_get().await.unwrap();
        assert_eq!(current.revision, first.revision);
        assert_eq!(current.layout, kept);
    }

    /// 从未保存过时重置是幂等空操作：不得凭空造出一个 revision。
    #[tokio::test]
    async fn reset_without_saved_layout_is_a_no_op() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());
        let reset = service.home_layout_reset(None).await.unwrap();
        assert!(!reset.changed);
        assert_eq!(reset.revision, None);
        assert_eq!(reset.layout, HomeLayout::default());
        assert!(repo.get_home_layout().await.unwrap().is_none());
    }

    // ---------- 总览布局 ----------

    fn overview_placement(
        module: OverviewModuleId,
        size: OverviewModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> OverviewModulePlacement {
        OverviewModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的总览放置必须合法")
    }

    /// 从未自定义过总览时读取返回领域默认布局 + `revision = null`；显式保存空布局
    /// （用户隐藏了全部模块）是**另一种**真实状态，与「从未保存」由 revision 区分。
    #[tokio::test]
    async fn missing_overview_layout_falls_back_and_empty_layout_is_real_saved_state() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());
        let initial = service.overview_layout_get().await.unwrap();
        assert_eq!(initial.revision, None);
        assert_eq!(initial.layout, OverviewLayout::default());
        assert_eq!(initial.layout.modules.len(), 5, "默认总览是五个真实内容块");

        let saved = service
            .overview_layout_save(
                None,
                OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, vec![]).unwrap(),
            )
            .await
            .unwrap();
        assert!(saved.changed);
        assert!(saved.revision.is_some());
        let reread = service.overview_layout_get().await.unwrap();
        assert!(reread.layout.modules.is_empty());
        assert_eq!(reread.revision, saved.revision);
    }

    /// 幂等：同一份布局（含乱序数组）重复提交都是 `changed = false` 且 revision 不变；
    /// 陈旧 revision 是 `REVISION_CONFLICT` 且零写入。
    #[tokio::test]
    async fn overview_layout_save_is_idempotent_and_conflicts_on_stale_revision() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let layout = OverviewLayout::new(
            OVERVIEW_LAYOUT_SCHEMA_VERSION,
            vec![overview_placement(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Large,
                0,
                0,
                0,
            )],
        )
        .unwrap();

        let first = service
            .overview_layout_save(None, layout.clone())
            .await
            .unwrap();
        assert!(first.changed);

        let second = service
            .overview_layout_save(first.revision.as_deref(), layout)
            .await
            .unwrap();
        assert!(!second.changed, "同一份布局重复保存不得判为变化");
        assert_eq!(second.revision, first.revision);

        let error = service
            .overview_layout_save(None, OverviewLayout::default())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");

        // 零写入：磁盘上仍是上一步保存的那份布局。
        let current = service.overview_layout_get().await.unwrap();
        assert_eq!(current.revision, first.revision);
        assert_eq!(current.layout.modules.len(), 1);
    }

    /// 乱序提交规范化后与读回形态相等：`changed` 不因为数组顺序抖动。
    #[tokio::test]
    async fn overview_layout_save_is_idempotent_regardless_of_module_array_order() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let preferences = overview_placement(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Large,
            0,
            0,
            0,
        );
        let metrics = overview_placement(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Large,
            1,
            0,
            1,
        );

        let out_of_order = OverviewLayout::new(
            OVERVIEW_LAYOUT_SCHEMA_VERSION,
            vec![metrics.clone(), preferences.clone()],
        )
        .unwrap();
        let saved = service
            .overview_layout_save(None, out_of_order)
            .await
            .unwrap();
        assert!(saved.changed);
        assert_eq!(
            saved
                .layout
                .ordered_modules()
                .into_iter()
                .map(|item| item.module)
                .collect::<Vec<_>>(),
            vec![OverviewModuleId::Preferences, OverviewModuleId::Metrics]
        );

        let reordered =
            OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, vec![preferences, metrics])
                .unwrap();
        let second = service
            .overview_layout_save(saved.revision.as_deref(), reordered)
            .await
            .unwrap();
        assert!(!second.changed);
        assert_eq!(second.revision, saved.revision);
    }

    /// 服务层的第二道闸：`overview_layout_save` 自己也会 `validate()` 一次，重叠布局
    /// 必须在**触碰存储之前**被拒绝，且不产生任何 revision。
    ///
    /// 这份状态只能绕过校验构造器造出来：`OverviewLayout::new` 会拒绝同一个形状（下面
    /// 先钉住这一点）。但字段是 pub（存储读取侧与 wire 反序列化都构造它），所以服务层
    /// 不能假设「拿到的布局一定合法」——这条用例验证的正是那个假设不成立时的行为。
    #[tokio::test]
    async fn overview_layout_save_rejects_an_overlapping_layout_without_writing() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());
        // large 占满整行 0，small 落在它覆盖的最后一列：起始格不同，占格却重叠。
        let overlapping = OverviewLayout {
            schema_version: OVERVIEW_LAYOUT_SCHEMA_VERSION,
            modules: vec![
                OverviewModulePlacement {
                    module: OverviewModuleId::Metrics,
                    size: OverviewModuleSize::Large,
                    row: 0,
                    column: 0,
                    order: 0,
                },
                OverviewModulePlacement {
                    module: OverviewModuleId::Preferences,
                    size: OverviewModuleSize::Small,
                    row: 0,
                    column: 2,
                    order: 1,
                },
            ],
        };
        assert!(
            OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, overlapping.modules.clone())
                .is_err(),
            "同一个形状必须被领域的校验构造器拒绝，否则这条用例测的不是「绕过校验器」"
        );

        let error = service
            .overview_layout_save(None, overlapping)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "APPEARANCE_INVALID_OVERVIEW_LAYOUT");
        assert!(!error.retryable(), "布局非法不是可重试的故障");
        assert!(
            repo.get_overview_layout().await.unwrap().is_none(),
            "被拒绝的保存不得留下任何 revision"
        );
    }

    /// 总览与首页布局是两份独立事实：任一侧的保存/重置都不影响另一侧，两侧 revision
    /// 也不可互换。
    #[tokio::test]
    async fn overview_and_home_layouts_are_independent_facts() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());

        let home = service
            .home_layout_save(None, HomeLayout::default())
            .await
            .unwrap();
        assert!(home.changed);
        assert!(
            service
                .overview_layout_get()
                .await
                .unwrap()
                .revision
                .is_none(),
            "保存首页布局不得产生总览布局状态"
        );

        // 首页的 revision 不是总览的 CAS token。
        let error = service
            .overview_layout_save(home.revision.as_deref(), OverviewLayout::default())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");

        let overview = service
            .overview_layout_save(None, OverviewLayout::default())
            .await
            .unwrap();
        assert!(overview.changed);
        assert_ne!(
            overview.revision, home.revision,
            "两份布局的 revision 必须各自独立"
        );
        assert!(
            overview
                .revision
                .as_deref()
                .is_some_and(|revision| revision.starts_with("overview-")),
            "总览 revision 必须带自己的前缀，便于诊断"
        );

        // 重置总览不动首页布局。
        let reset = service
            .overview_layout_reset(overview.revision.as_deref())
            .await
            .unwrap();
        assert!(reset.changed);
        assert_eq!(reset.layout, OverviewLayout::default());
        assert_eq!(
            service.home_layout_get().await.unwrap().revision,
            home.revision,
            "重置总览不得动到首页布局"
        );
    }

    /// 从未保存过时重置总览是幂等空操作。
    #[tokio::test]
    async fn overview_reset_without_saved_layout_is_a_no_op() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo.clone());
        let reset = service.overview_layout_reset(None).await.unwrap();
        assert!(!reset.changed);
        assert_eq!(reset.revision, None);
        assert_eq!(reset.layout, OverviewLayout::default());
        assert!(repo.get_overview_layout().await.unwrap().is_none());
    }

    /// 保存过之后的重复保存（含隐藏全部模块）是幂等的，再次提交 `changed = false`。
    #[tokio::test]
    async fn overview_empty_layout_save_then_repeat_is_not_a_change() {
        let repo = Arc::new(MemoryRepo::default());
        let service = service(repo);
        let empty = OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, vec![]).unwrap();
        let first = service
            .overview_layout_save(None, empty.clone())
            .await
            .unwrap();
        assert!(first.changed);
        let second = service
            .overview_layout_save(first.revision.as_deref(), empty)
            .await
            .unwrap();
        assert!(!second.changed);
        assert_eq!(second.revision, first.revision);
    }

    /// 非法展示名必须在**任何字节落盘之前**被拒绝：否则导入会先把字节复制进受控
    /// 存储再回滚，白写一次磁盘。
    #[tokio::test]
    async fn invalid_display_name_is_rejected_before_any_byte_is_written() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());
        let too_long = "a".repeat(MAX_ASSET_DISPLAY_NAME_CHARS + 1);

        for invalid in [too_long, "名\u{7f}字".to_owned(), "名\u{200b}字".to_owned()] {
            let error = service
                .import_asset(
                    Path::new("ignored"),
                    AppearanceAssetKind::Font,
                    Some(invalid),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "APPEARANCE_INVALID_ASSET");
        }
        assert_eq!(*storage.import_calls.lock().unwrap(), 0);
        assert_eq!(storage.file_count(), 0);
        assert!(repo.list_assets(None).await.unwrap().is_empty());
    }

    /// 字节写入失败（签名/体积校验不过）时不得登记任何元数据。
    #[tokio::test]
    async fn import_failure_registers_nothing() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        *storage.import_failure.lock().unwrap() = Some("APPEARANCE_ASSET_SIGNATURE_MISMATCH");
        let service = service_with_storage(repo.clone(), storage.clone());

        let error = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "APPEARANCE_ASSET_SIGNATURE_MISMATCH");
        assert!(repo.list_assets(None).await.unwrap().is_empty());
        assert_eq!(storage.file_count(), 0);
    }

    /// 登记行写失败时必须回滚已写入的字节，不能留下没有登记的文件。
    #[tokio::test]
    async fn metadata_write_failure_rolls_back_the_written_bytes() {
        let repo = Arc::new(MemoryRepo::default());
        *repo.create_asset_failure.lock().unwrap() = true;
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());

        let error = service
            .import_asset(
                Path::new("ignored"),
                AppearanceAssetKind::StaticWallpaper,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        assert_eq!(storage.file_count(), 0);
        assert!(repo.list_assets(None).await.unwrap().is_empty());
    }

    /// 导入成功：登记行是 validated，字节确实落盘一次。
    #[tokio::test]
    async fn successful_import_registers_validated_metadata() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());

        let imported = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap();
        assert_eq!(imported.metadata.state, AssetValidationState::Validated);
        assert_eq!(imported.metadata.byte_size, 4);
        assert_eq!(storage.file_count(), 1);
        let listed = repo
            .list_assets(Some(AppearanceAssetKind::Font))
            .await
            .unwrap();
        assert_eq!(listed, vec![imported.metadata]);
    }

    /// 删除顺序：先删登记行（唯一事实源），再删字节。
    #[tokio::test]
    async fn delete_removes_the_registration_before_the_bytes() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());
        let imported = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap();

        let deleted = service.delete_asset(imported.metadata.id).await.unwrap();
        assert!(deleted.deleted);
        assert!(deleted.file_removed);
        assert!(repo.list_assets(None).await.unwrap().is_empty());
        assert_eq!(storage.file_count(), 0);
    }

    /// 字节清理失败时：登记行仍然删除（读路径上资产确实不存在），结果如实报告
    /// `file_removed=false`。残留的是没有登记引用的垃圾文件，而不是「列表里有、
    /// 字节没有」的不可用资产。再删一次是幂等的，且不再触碰字节。
    #[tokio::test]
    async fn delete_reports_partial_cleanup_without_leaving_a_broken_asset() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::with_remove_failure(
            "APPEARANCE_CLEANUP_FAILED",
        ));
        let service = service_with_storage(repo.clone(), storage.clone());
        let imported = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap();

        let deleted = service.delete_asset(imported.metadata.id).await.unwrap();
        assert!(deleted.deleted);
        assert!(!deleted.file_removed);
        assert!(
            repo.get_asset(imported.metadata.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(storage.file_count(), 1);

        let again = service.delete_asset(imported.metadata.id).await.unwrap();
        assert!(!again.deleted);
        assert!(!again.file_removed);
        assert_eq!(storage.file_count(), 1);
    }

    /// 删除不存在的资产是幂等空操作，且不得触碰任何字节。
    #[tokio::test]
    async fn delete_of_unknown_asset_touches_no_bytes() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());
        let orphan = AppearanceAssetId::new();
        storage.files.lock().unwrap().push((orphan, 7));

        let deleted = service.delete_asset(orphan).await.unwrap();
        assert!(!deleted.deleted);
        assert!(!deleted.file_removed);
        assert_eq!(storage.file_count(), 1);
    }

    /// 仍被外观设置引用的资产：删除被拒绝，且登记行与字节都原封不动。
    ///
    /// 这是「UI 之外」的那道闸：即使界面没拦住，应用层也不会让一份外观设置指向
    /// 已被删除的资产。
    #[tokio::test]
    async fn delete_of_a_referenced_asset_is_refused_without_touching_anything() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());
        let imported = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap();
        repo.references
            .lock()
            .unwrap()
            .push((imported.metadata.id, AppearanceAssetReference::CustomFont));

        let error = service
            .delete_asset(imported.metadata.id)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_IN_USE);
        assert!(!error.retryable());
        assert_eq!(
            repo.get_asset(imported.metadata.id).await.unwrap(),
            Some(imported.metadata.clone()),
            "被引用的资产不得删除登记行"
        );
        assert_eq!(storage.file_count(), 1, "被引用的资产不得删除字节");

        // 引用解除后，同一次删除就会成功。
        repo.references.lock().unwrap().clear();
        let deleted = service.delete_asset(imported.metadata.id).await.unwrap();
        assert!(deleted.deleted);
        assert_eq!(storage.file_count(), 0);
    }

    /// 同时被字体与壁纸引用时，错误信息必须把两个位置都列出来（用户才知道要改什么）。
    #[tokio::test]
    async fn delete_error_names_every_referencing_placement() {
        let repo = Arc::new(MemoryRepo::default());
        let storage = Arc::new(MemoryStorage::default());
        let service = service_with_storage(repo.clone(), storage.clone());
        let imported = service
            .import_asset(Path::new("ignored"), AppearanceAssetKind::Font, None)
            .await
            .unwrap();
        repo.references.lock().unwrap().extend([
            (imported.metadata.id, AppearanceAssetReference::CustomFont),
            (imported.metadata.id, AppearanceAssetReference::Wallpaper),
        ]);

        let error = service
            .delete_asset(imported.metadata.id)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_IN_USE);
        assert!(error.user_message().contains("界面字体"));
        assert!(error.user_message().contains("首页壁纸"));
    }
}
