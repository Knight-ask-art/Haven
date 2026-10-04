//! InterfaceFontService 集成测试（BE-INTERFACE-FONT-001）。
//!
//! 真实 SQLite + 真实 Infrastructure 实现（字体识别、仓储、UoW），只替换「本机字体目录」
//! 这一个外部事实源。覆盖：
//! - 导入边界：扩展名 / 签名 / 大小 / 摘要去重 / 数量上限 / 族名修复 / 文件名扁平化；
//! - 持久化与重启：重新打开同一文件后 opaque id 与字节仍可读；
//! - 删除：仍被外观设置引用的字体在同一事务内被拒绝；未知/已删除/非规范 id 被拒绝；
//! - 设置写入边界：引用不存在的资产（stale id）与不安全族名都被拒绝且不写库。

use std::path::PathBuf;
use std::sync::Arc;

use haven_application::services::interface_fonts::{
    InterfaceFontService, MAX_INTERFACE_FONT_ASSETS,
};
use haven_application::services::settings::SettingsService;
use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{InterfaceFontAssetRepository, SystemFontCatalog, SystemFontFamily};
use haven_domain::settings::{
    AppearancePatch, InterfaceFontMode, SettingsPatch, SettingsSection, SettingsValue,
    is_safe_interface_font_family,
};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{
    SqliteInterfaceFontAssetRepository, SqliteInterfaceFontUoW, SqliteRepositories,
    SqliteSettingsUoW,
};
use haven_infrastructure::system_fonts::{
    LocalFontFileInspector, MAX_FONT_FILE_BYTES, parse_font_families,
};

const UNKNOWN_ASSET_ID: &str = "11111111-2222-4333-8444-555555555555";

/// 只读字体目录替身：本测试不依赖运行机器的真实字体。
struct NoSystemFonts;

impl SystemFontCatalog for NoSystemFonts {
    fn list_families(&self) -> Result<Vec<SystemFontFamily>, AppError> {
        Ok(Vec::new())
    }
}

fn service(db: &Arc<Db>) -> InterfaceFontService {
    service_with(db, Arc::new(SqliteRepositories::new(db.clone())))
}

fn service_with(db: &Arc<Db>, repos: Arc<SqliteRepositories>) -> InterfaceFontService {
    InterfaceFontService::new(
        Arc::new(SqliteInterfaceFontUoW::new(db.clone())),
        Arc::new(NoSystemFonts),
        Arc::new(LocalFontFileInspector),
        repos,
    )
}

fn settings_service(db: &Arc<Db>) -> SettingsService {
    SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())))
}

/// `asset_id`：`None` = 不改动；`Some("")` = 清除引用；其它 = 设为该 id。
fn appearance_patch(
    mode: Option<InterfaceFontMode>,
    family: Option<&str>,
    asset_id: Option<&str>,
) -> SettingsPatch {
    SettingsPatch::Appearance(AppearancePatch {
        theme: None,
        density: None,
        sidebar: None,
        reduce_motion: None,
        interface_font_mode: mode,
        interface_font_family: family.map(str::to_owned),
        interface_font_asset_id: asset_id.map(str::to_owned),
        ..AppearancePatch::default()
    })
}

/// 最小可用 SFNT（只有 `name` 表），足以让识别器读出族名并算出内容摘要。
fn build_font(family: &str) -> Vec<u8> {
    let units: Vec<u8> = family
        .encode_utf16()
        .flat_map(|unit| unit.to_be_bytes())
        .collect();
    let mut name_table = Vec::new();
    name_table.extend_from_slice(&0u16.to_be_bytes()); // format
    name_table.extend_from_slice(&1u16.to_be_bytes()); // count
    name_table.extend_from_slice(&18u16.to_be_bytes()); // stringOffset = 6 + 12
    name_table.extend_from_slice(&3u16.to_be_bytes()); // platformID（Windows）
    name_table.extend_from_slice(&1u16.to_be_bytes()); // encodingID
    name_table.extend_from_slice(&0x0409u16.to_be_bytes()); // languageID（en-US）
    name_table.extend_from_slice(&1u16.to_be_bytes()); // nameID（family）
    name_table.extend_from_slice(&(units.len() as u16).to_be_bytes());
    name_table.extend_from_slice(&0u16.to_be_bytes()); // storage offset
    name_table.extend_from_slice(&units);

    let mut font = Vec::new();
    font.extend_from_slice(b"\x00\x01\x00\x00");
    font.extend_from_slice(&1u16.to_be_bytes()); // numTables
    font.extend_from_slice(&0u16.to_be_bytes());
    font.extend_from_slice(&0u16.to_be_bytes());
    font.extend_from_slice(&0u16.to_be_bytes());
    font.extend_from_slice(b"name");
    font.extend_from_slice(&0u32.to_be_bytes());
    font.extend_from_slice(&28u32.to_be_bytes()); // table offset
    font.extend_from_slice(&(name_table.len() as u32).to_be_bytes());
    font.extend_from_slice(&name_table);
    font
}

/// 与 `build_font` 同形但内容不同（摘要不同）的一份文件。
fn build_unique_font(index: u32) -> Vec<u8> {
    build_font(&format!("Import Fixture {index}"))
}

#[tokio::test]
async fn import_persists_metadata_and_bytes_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("haven.db");
    let bytes = build_font("Example Sans");

    let asset_id = {
        let db = Arc::new(Db::open(&path).unwrap());
        let outcome = service(&db)
            .import("ExampleSans-Regular.ttf", &bytes, UtcMillis::now())
            .await
            .unwrap();
        assert!(!outcome.deduplicated);
        assert_eq!(outcome.asset.family_name, "Example Sans");
        assert_eq!(outcome.asset.file_name, "ExampleSans-Regular.ttf");
        assert_eq!(outcome.asset.extension, "ttf");
        assert_eq!(outcome.asset.mime_type, "font/ttf");
        assert_eq!(outcome.asset.byte_size as usize, bytes.len());
        assert_eq!(outcome.asset.sha256.len(), 64);
        outcome.asset.id
    };

    // 「重启」：同一文件重新打开连接，opaque id 必须仍然可用。
    let reopened = Arc::new(Db::open(&path).unwrap());
    let reopened_service = service(&reopened);
    let listed = reopened_service.list_assets().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, asset_id);
    assert_eq!(listed[0].family_name, "Example Sans");

    let loaded = reopened_service
        .load_bytes(&asset_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.mime_type, "font/ttf");
    assert_eq!(loaded.bytes, bytes);
}

#[tokio::test]
async fn custom_system_font_selection_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("haven.db");
    let revision = {
        let db = Arc::new(Db::open(&path).unwrap());
        let snapshot = settings_service(&db)
            .update(
                SettingsSection::Appearance,
                None,
                appearance_patch(
                    Some(InterfaceFontMode::Custom),
                    Some("Source Han Serif SC"),
                    None,
                ),
            )
            .await
            .unwrap();
        snapshot.revision.expect("保存后应生成 revision")
    };

    // 重新打开真实 SQLite 文件，确认模式与字体族名共同恢复。
    let reopened = Arc::new(Db::open(&path).unwrap());
    let restored = settings_service(&reopened)
        .get(SettingsSection::Appearance)
        .await
        .unwrap();
    assert_eq!(restored.revision.as_deref(), Some(revision.as_str()));
    match restored.value {
        SettingsValue::Appearance(value) => {
            assert_eq!(value.interface_font_mode, InterfaceFontMode::Custom);
            assert_eq!(
                value.interface_font_family.as_deref(),
                Some("Source Han Serif SC")
            );
            assert!(value.interface_font_asset_id.is_none());
        }
        other => panic!("expected appearance settings, got {other:?}"),
    }
}

#[tokio::test]
async fn import_deduplicates_identical_bytes() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let bytes = build_font("Example Sans");

    let first = svc.import("A.ttf", &bytes, UtcMillis::now()).await.unwrap();
    let second = svc
        .import("Renamed Copy.ttf", &bytes, UtcMillis::now())
        .await
        .unwrap();

    assert!(!first.deduplicated);
    assert!(second.deduplicated, "同一份字节必须复用既有资产");
    assert_eq!(second.asset.id, first.asset.id);
    assert_eq!(svc.list_assets().await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_rejects_unsupported_wrong_and_oversized_files() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let font = build_font("Example Sans");

    // 不在闭合扩展名集合内。
    let error = svc
        .import("font.ttc", &font, UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_FORMAT_UNSUPPORTED");
    let error = svc
        .import("no-extension", &font, UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_FORMAT_UNSUPPORTED");

    // 扩展名与内容签名不匹配（TTF 改名为 woff2）。
    let error = svc
        .import("font.woff2", &font, UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_FILE_INVALID");

    // 空文件。
    let error = svc
        .import("font.ttf", &[], UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_FILE_INVALID");

    // 超限文件（32 MiB 上限）。
    let oversized = vec![0u8; MAX_FONT_FILE_BYTES + 1];
    let error = svc
        .import("big.ttf", &oversized, UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_FILE_INVALID");

    // 所有失败路径都不得留下任何持久化痕迹。
    assert!(svc.list_assets().await.unwrap().is_empty());
}

#[tokio::test]
async fn import_enforces_asset_limit() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    for index in 0..MAX_INTERFACE_FONT_ASSETS {
        svc.import(
            &format!("Fixture{index}.ttf"),
            &build_unique_font(index),
            UtcMillis::now(),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        svc.list_assets().await.unwrap().len(),
        MAX_INTERFACE_FONT_ASSETS as usize
    );

    let error = svc
        .import(
            "OneTooMany.ttf",
            &build_unique_font(MAX_INTERFACE_FONT_ASSETS),
            UtcMillis::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_LIMIT_REACHED");
    assert_eq!(
        svc.list_assets().await.unwrap().len(),
        MAX_INTERFACE_FONT_ASSETS as usize
    );
}

#[tokio::test]
async fn unsafe_family_names_from_font_files_are_repaired() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let bytes = build_font("Bad\";color:red{}");

    // 识别到的原始族名确实带着危险字符（否则这个用例没有意义）。
    assert_eq!(
        parse_font_families(&bytes)[0].0,
        "Bad\";color:red{}",
        "识别器应原样读出字体里的族名"
    );

    let outcome = svc
        .import("weird.ttf", &bytes, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(outcome.asset.family_name, "Badcolor:red");
    assert!(is_safe_interface_font_family(&outcome.asset.family_name));
}

#[tokio::test]
async fn file_names_are_flattened_and_never_stored_as_paths() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let outcome = svc
        .import(
            "C:\\Users\\someone\\Fonts\\ExampleSans.ttf",
            &build_font("Example Sans"),
            UtcMillis::now(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.asset.file_name, "ExampleSans.ttf");
    assert!(!outcome.asset.file_name.contains('\\'));
    assert!(!outcome.asset.file_name.contains('/'));
}

#[tokio::test]
async fn delete_is_rejected_while_the_asset_is_referenced_by_appearance_settings() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let settings = settings_service(&db);
    let asset = svc
        .import(
            "ExampleSans.ttf",
            &build_font("Example Sans"),
            UtcMillis::now(),
        )
        .await
        .unwrap()
        .asset;

    // 选中它（设置写入边界会先校验资产真实存在）。
    let selected = settings
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(
                Some(InterfaceFontMode::Custom),
                None,
                Some(asset.id.as_str()),
            ),
        )
        .await
        .unwrap();

    let error = svc.delete(&asset.id).await.unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_IN_USE");
    assert_eq!(
        svc.list_assets().await.unwrap().len(),
        1,
        "被拒绝的删除不得留下副作用"
    );

    // 引用与模式无关：即使切到预设，只要设置里还留着这个 id 就不允许删除
    // （否则设置会永久引用一个已删除的资产）。
    let preset = settings
        .update(
            SettingsSection::Appearance,
            selected.revision.as_deref(),
            appearance_patch(Some(InterfaceFontMode::Serif), None, None),
        )
        .await
        .unwrap();
    let error = svc.delete(&asset.id).await.unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_IN_USE");

    // 显式清除引用后即可删除。
    settings
        .update(
            SettingsSection::Appearance,
            preset.revision.as_deref(),
            appearance_patch(None, None, Some("")),
        )
        .await
        .unwrap();
    svc.delete(&asset.id).await.unwrap();
    assert!(svc.list_assets().await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_rejects_unknown_deleted_and_non_canonical_ids() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let error = svc.delete(UNKNOWN_ASSET_ID).await.unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_NOT_FOUND");

    // 非规范 id 直接拒绝，连数据库都不打。
    let uppercase = UNKNOWN_ASSET_ID.to_uppercase();
    for raw in ["", "not-a-uuid", "../etc/passwd", uppercase.as_str()] {
        let error = svc.delete(raw).await.unwrap_err();
        assert_eq!(error.code().as_str(), "FONT_ASSET_NOT_FOUND", "id={raw}");
    }

    // 已删除的 id 再删一次必须被拒绝（stale id）。
    let asset = svc
        .import(
            "ExampleSans.ttf",
            &build_font("Example Sans"),
            UtcMillis::now(),
        )
        .await
        .unwrap()
        .asset;
    svc.delete(&asset.id).await.unwrap();
    let error = svc.delete(&asset.id).await.unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_NOT_FOUND");
}

#[tokio::test]
async fn settings_reject_missing_stale_and_unsafe_font_selections() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let settings = settings_service(&db);

    // 1) 引用一个从未存在过的资产 → 在同一事务内被拒绝，设置行不被写入。
    let error = settings
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(
                Some(InterfaceFontMode::Custom),
                None,
                Some(UNKNOWN_ASSET_ID),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_NOT_FOUND");
    let snapshot = settings.get(SettingsSection::Appearance).await.unwrap();
    assert!(snapshot.revision.is_none(), "被拒绝的更新不得写入任何版本");

    // 2) 不安全族名同样在保存边界被拒绝。
    let error = settings
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(
                Some(InterfaceFontMode::Custom),
                Some("Arial\"; color: red"),
                None,
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    let snapshot = settings.get(SettingsSection::Appearance).await.unwrap();
    assert!(snapshot.revision.is_none());

    // 3) 真实存在的资产可以正常引用。
    let asset = svc
        .import(
            "ExampleSans.ttf",
            &build_font("Example Sans"),
            UtcMillis::now(),
        )
        .await
        .unwrap()
        .asset;
    let result = settings
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(
                Some(InterfaceFontMode::Custom),
                None,
                Some(asset.id.as_str()),
            ),
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert!(result.revision.is_some());

    // 4) 资产一旦被删除，它的 id 再也无法被写进设置（stale id 被拒绝）。
    //    先清除引用才能删除（见上一个用例），因此这里是「删除后再引用」。
    let cleared = settings
        .update(
            SettingsSection::Appearance,
            result.revision.as_deref(),
            appearance_patch(None, None, Some("")),
        )
        .await
        .unwrap();
    svc.delete(&asset.id).await.unwrap();

    let error = settings
        .update(
            SettingsSection::Appearance,
            cleared.revision.as_deref(),
            appearance_patch(
                Some(InterfaceFontMode::Custom),
                None,
                Some(asset.id.as_str()),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "FONT_ASSET_NOT_FOUND");
}

#[tokio::test]
async fn load_bytes_never_resolves_non_canonical_ids() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let asset = svc
        .import(
            "ExampleSans.ttf",
            &build_font("Example Sans"),
            UtcMillis::now(),
        )
        .await
        .unwrap()
        .asset;

    let loaded = svc.load_bytes(&asset.id).await.unwrap().unwrap();
    assert_eq!(loaded.id, asset.id);

    let uppercase = asset.id.to_uppercase();
    for raw in ["", "not-a-uuid", "../etc/passwd", uppercase.as_str()] {
        assert!(
            svc.load_bytes(raw).await.unwrap().is_none(),
            "非规范 id 必须按不存在处理：{raw}"
        );
    }
}

#[tokio::test]
async fn repository_projects_metadata_without_bytes_or_paths() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let bytes = build_font("Example Sans");
    let asset = service_with(&db, repos.clone())
        .import("ExampleSans.ttf", &bytes, UtcMillis::now())
        .await
        .unwrap()
        .asset;

    let listed = repos.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].file_name, "ExampleSans.ttf");
    assert_eq!(listed[0].byte_size as usize, bytes.len());
    assert_eq!(repos.count().await.unwrap(), 1);

    let fetched = repos.get(&asset.id).await.unwrap().unwrap();
    assert_eq!(fetched.family_name, "Example Sans");
    assert_eq!(fetched.sha256.len(), 64);
}

#[tokio::test]
async fn repository_load_bytes_returns_none_for_unknown_id() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repo = SqliteInterfaceFontAssetRepository::new(db.clone());
    assert!(repo.load_bytes(UNKNOWN_ASSET_ID).await.unwrap().is_none());
    assert_eq!(repo.count().await.unwrap(), 0);
    assert!(repo.list().await.unwrap().is_empty());
    assert!(repo.get(UNKNOWN_ASSET_ID).await.unwrap().is_none());
    assert!(!repo.delete(UNKNOWN_ASSET_ID).await.unwrap());
    assert!(
        repo.find_by_digest(&"0".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
}
