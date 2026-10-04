//! InterfaceFontService：界面自定义字体的枚举、导入、选择与删除（BE-INTERFACE-FONT-001）。
//!
//! 事实边界：
//! - **Rust + SQLite 是唯一事实源**。枚举结果、导入元数据与字节都在这里产生，
//!   前端只拿到 opaque id 与族名，永远拿不到路径。
//! - 本机字体枚举只产出**族名**（见 [`SystemFontCatalog`]），导入字体只产出
//!   **opaque id**；WebView 通过 `haven-resource://font/<id>` 取字节。
//! - **导入校验在边界一次做完**：扩展名 + 文件签名 + 大小上限（识别器）+ 族名
//!   安全化（Domain）+ 数量上限 + 内容摘要去重（本服务，同一事务）。
//! - **「不得删除正在使用的字体」在同一个事务内判定**：先读 `settings.appearance`
//!   的 `interfaceFontAssetId`，命中即拒绝；否则删除。两者在同一 `BEGIN IMMEDIATE`
//!   事务内，不存在「检查通过后设置又被改成这个字体」的窗口。
//! - 与之对称的另一半在 `SettingsTxPorts::validate_references`：设置写入时
//!   在**同一事务**里确认被引用的资产存在，因此已删除的 stale id 无法被写进设置。

use std::sync::{Arc, Mutex};

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{
    FontFileInspector, InterfaceFontAsset, InterfaceFontAssetBytes, InterfaceFontAssetInsert,
    InterfaceFontAssetRepository, SystemFontCatalog, SystemFontFamily,
};
use haven_domain::settings::sanitize_interface_font_family;
use uuid::Uuid;

/// 已导入字体数量上限。
///
/// 上限存在的意义是给「导入的字节全部进 SQLite」这件事一个明确边界：
/// 单文件 32 MiB × 32 ≈ 1 GiB 是硬上限，而不是「随便导入直到磁盘满」。
pub const MAX_INTERFACE_FONT_ASSETS: u32 = 32;

/// 族名全部不可用时的兜底展示名（永远安全，绝不为空）。
const FALLBACK_FAMILY_NAME: &str = "导入字体";

/// 事务内可用的界面字体原语。
///
/// 「读当前选中资产 + 删除」必须在同一事务，所以这两件事一起出现在这里，
/// 而不是拆到异步 Repository 上。
pub trait InterfaceFontTxPorts {
    /// 当前 `appearance.interfaceFontAssetId`（未选中 / 未保存 / 非外观分区 → None）。
    fn active_asset_id(&self) -> Result<Option<String>, AppError>;
    /// 资产是否存在（stale id 判定）。
    fn asset_exists(&self, id: &str) -> Result<bool, AppError>;
    /// 按内容摘要查重。
    fn find_by_digest(&self, sha256: &str) -> Result<Option<InterfaceFontAsset>, AppError>;
    /// 当前资产数量。
    fn count_assets(&self) -> Result<u32, AppError>;
    /// 写入新资产（元数据 + 字节）。
    fn insert_asset(&self, asset: &InterfaceFontAssetInsert) -> Result<(), AppError>;
    /// 删除资产；不存在返回 `false`。
    fn delete_asset(&self, id: &str) -> Result<bool, AppError>;
}

/// 界面字体 Unit of Work：闭包在**单一 `BEGIN IMMEDIATE` 事务**内执行。
pub trait InterfaceFontUoW: Send + Sync {
    fn run(
        &self,
        f: &dyn Fn(&dyn InterfaceFontTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError>;
}

/// 一次导入的结果。
///
/// `deduplicated = true` 表示同一份文件此前已导入：复用既有资产，
/// 不新增行、不重复占用配额、不改动资产 id（因此已引用它的设置不受影响）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceFontImportOutcome {
    pub asset: InterfaceFontAsset,
    pub deduplicated: bool,
}

/// 本机字体目录 + 导入资产的应用服务。
#[derive(Clone)]
pub struct InterfaceFontService {
    uow: Arc<dyn InterfaceFontUoW>,
    catalog: Arc<dyn SystemFontCatalog>,
    inspector: Arc<dyn FontFileInspector>,
    assets: Arc<dyn InterfaceFontAssetRepository>,
}

impl InterfaceFontService {
    pub fn new(
        uow: Arc<dyn InterfaceFontUoW>,
        catalog: Arc<dyn SystemFontCatalog>,
        inspector: Arc<dyn FontFileInspector>,
        assets: Arc<dyn InterfaceFontAssetRepository>,
    ) -> Self {
        Self {
            uow,
            catalog,
            inspector,
            assets,
        }
    }

    /// 本机已安装字体族（只含族名，不含路径）。
    pub async fn list_system_families(&self) -> Result<Vec<SystemFontFamily>, AppError> {
        self.catalog.list_families()
    }

    /// 已导入字体资产的元数据，新的在前。
    pub async fn list_assets(&self) -> Result<Vec<InterfaceFontAsset>, AppError> {
        self.assets.list().await
    }

    /// 按 opaque id 读取字节（仅供受控资源协议使用）。
    pub async fn load_bytes(&self, id: &str) -> Result<Option<InterfaceFontAssetBytes>, AppError> {
        if !is_canonical_asset_id(id) {
            // 非规范 id 一律按「不存在」处理：既不打库，也不回显输入。
            return Ok(None);
        }
        self.assets.load_bytes(id).await
    }

    /// 导入一份字体文件。
    ///
    /// 边界校验全部在这里，且校验失败时不产生任何持久化副作用：
    /// 1. 识别（扩展名闭合集合 + 文件签名 + 32 MiB 上限 + 内容摘要）；
    /// 2. 族名安全化（第三方字体文件里的族名不可直接信任）；
    /// 3. 同一事务内：摘要去重 → 数量上限 → 写入。
    pub async fn import(
        &self,
        file_name: &str,
        bytes: &[u8],
        created_at: UtcMillis,
    ) -> Result<InterfaceFontImportOutcome, AppError> {
        let inspected = self.inspector.inspect(file_name, bytes)?;
        let insert = InterfaceFontAssetInsert {
            id: Uuid::new_v4().to_string(),
            family_name: sanitize_interface_font_family(
                &inspected.family_name,
                FALLBACK_FAMILY_NAME,
            ),
            file_name: sanitize_file_name(file_name),
            extension: inspected.extension.clone(),
            mime_type: inspected.mime_type.clone(),
            sha256: inspected.sha256.clone(),
            bytes: bytes.to_vec(),
            created_at,
        };

        let cell = Arc::new(Mutex::new(
            None::<Result<InterfaceFontImportOutcome, AppError>>,
        ));
        self.uow.run(&|tx| {
            if let Some(existing) = tx.find_by_digest(&inspected.sha256)? {
                *cell.lock().unwrap() = Some(Ok(InterfaceFontImportOutcome {
                    asset: existing,
                    deduplicated: true,
                }));
                return Ok(());
            }
            if tx.count_assets()? >= MAX_INTERFACE_FONT_ASSETS {
                *cell.lock().unwrap() = Some(Err(AppError::new(
                    "FONT_ASSET_LIMIT_REACHED",
                    ErrorKind::Validation,
                    "已导入字体数量达到上限，请先删除不再使用的字体",
                    false,
                )));
                return Ok(());
            }
            tx.insert_asset(&insert)?;
            *cell.lock().unwrap() = Some(Ok(InterfaceFontImportOutcome {
                asset: projection_of(&insert),
                deduplicated: false,
            }));
            Ok(())
        })?;
        cell.lock().unwrap().take().expect("闭包必然写入结果")
    }

    /// 删除一份导入字体。
    ///
    /// 只要 `appearance.interfaceFontAssetId` 还引用着这个资产就拒绝删除——判定与删除
    /// 在同一事务内，因此不存在「检查通过后设置又被切到这个字体」的窗口。
    ///
    /// 判定**与 `interfaceFontMode` 无关**：模式切到预设并不会清除引用，若允许删除，
    /// 设置行就会永久指向一个不存在的资产；要删除必须先显式改选/清除引用。
    /// 不存在的 id（含已被删除的 stale id）返回 `FONT_ASSET_NOT_FOUND`。
    pub async fn delete(&self, id: &str) -> Result<(), AppError> {
        if !is_canonical_asset_id(id) {
            return Err(asset_not_found());
        }
        let cell = Arc::new(Mutex::new(None::<Result<(), AppError>>));
        self.uow.run(&|tx| {
            if tx.active_asset_id()?.as_deref() == Some(id) {
                *cell.lock().unwrap() = Some(Err(AppError::new(
                    "FONT_ASSET_IN_USE",
                    ErrorKind::Conflict,
                    "该字体仍被界面字体设置引用，请先改选其他字体再删除",
                    false,
                )));
                return Ok(());
            }
            if !tx.delete_asset(id)? {
                *cell.lock().unwrap() = Some(Err(asset_not_found()));
                return Ok(());
            }
            *cell.lock().unwrap() = Some(Ok(()));
            Ok(())
        })?;
        cell.lock().unwrap().take().expect("闭包必然写入结果")
    }
}

/// 文件名只用于展示：去掉目录成分与控制字符，并截断到数据库列上限（260）。
///
/// 这里**不是**安全措施的全部——真正的安全来自「永不使用 file_name 拼路径」；
/// 该函数保证它落库时是干净的单段文本。
fn sanitize_file_name(file_name: &str) -> String {
    let base = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_name)
        .trim();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "font".to_owned();
    }
    cleaned.chars().take(260).collect()
}

fn projection_of(insert: &InterfaceFontAssetInsert) -> InterfaceFontAsset {
    InterfaceFontAsset {
        id: insert.id.clone(),
        family_name: insert.family_name.clone(),
        file_name: insert.file_name.clone(),
        extension: insert.extension.clone(),
        mime_type: insert.mime_type.clone(),
        byte_size: insert.bytes.len() as u64,
        sha256: insert.sha256.clone(),
        created_at: insert.created_at,
    }
}

fn asset_not_found() -> AppError {
    AppError::new(
        "FONT_ASSET_NOT_FOUND",
        ErrorKind::NotFound,
        "字体不存在或已被删除",
        false,
    )
}

/// 只接受规范 UUID 字符串（与资源协议同一判定），避免任意输入打到数据库。
fn is_canonical_asset_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_asset_ids_only() {
        let id = Uuid::new_v4().to_string();
        assert!(is_canonical_asset_id(&id));
        assert!(!is_canonical_asset_id(""));
        assert!(!is_canonical_asset_id("not-a-uuid"));
        assert!(!is_canonical_asset_id("../etc/passwd"));
        // 大写与非规范写法都拒绝（与资源协议一致：必须能原样 round-trip）。
        assert!(!is_canonical_asset_id(&id.to_uppercase()));
        assert!(!is_canonical_asset_id(&format!("{{{id}}}")));
    }

    #[test]
    fn file_names_are_flattened_for_display() {
        assert_eq!(sanitize_file_name("C:\\Fonts\\My Font.ttf"), "My Font.ttf");
        assert_eq!(sanitize_file_name("/usr/share/fonts/a.otf"), "a.otf");
        assert_eq!(sanitize_file_name("..\\..\\evil.woff2"), "evil.woff2");
        assert_eq!(sanitize_file_name(""), "font");
        assert_eq!(sanitize_file_name("   "), "font");
        assert_eq!(sanitize_file_name("a\nb.ttf"), "ab.ttf");
        assert_eq!(sanitize_file_name(&"x".repeat(400)).chars().count(), 260);
    }
}
