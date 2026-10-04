//! 外观资产（Appearance）真实组合集成测试。
//!
//! 服务层的失败补偿（`services/appearance.rs`）与存储层的校验/原子替换
//! （`appearance_assets.rs`）各自都有单元测试，但真正要钉住的是两者的接缝：
//! 字节落盘时用的那个不透明 ID，必须正好是删除时交给存储的那个 ID。这条性质在任何
//! 一侧的测试替身里都不成立——替身按传入的 ID 记账，两侧可以各自「正确」地漂移。
//!
//! 因此这里的用例全部走真实 SQLite（045/046 迁移）与真实文件系统。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use haven_application::services::appearance::AppearanceService;
use haven_application::services::settings::SettingsService;
use haven_domain::appearance::{
    APPEARANCE_ASSET_IN_USE, AppearanceAssetId, AppearanceAssetKind, HOME_LAYOUT_SCHEMA_VERSION,
    HomeLayout, HomeModuleId, HomeModulePlacement, HomeModuleSize, OVERVIEW_LAYOUT_SCHEMA_VERSION,
    OverviewLayout, OverviewModuleId, OverviewModulePlacement, OverviewModuleSize,
    WallpaperSelection,
};
use haven_domain::settings::{
    AppearancePatch, SettingsPatch, SettingsSection, SettingsValue, Theme,
};
use haven_infrastructure::Db;
use haven_infrastructure::appearance_assets::LocalAppearanceAssetStorage;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};

/// 稳定错误码：命令层原样映射为 `ErrorDto.code`，前端按它分支，所以钉字面量。
const SIGNATURE_MISMATCH: &str = "APPEARANCE_ASSET_SIGNATURE_MISMATCH";

/// 最小可识别的 TrueType 头（只喂签名嗅探，不要求是真能解析的字体）。
const TTF_BYTES: &[u8] = b"\x00\x01\x00\x00\x00\x0f\x00\x80";
/// 最小可识别的 PNG 头。
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\r";

struct Fixture {
    /// 受控存储根目录活在这个临时目录里，必须活到用例结束。
    _dir: tempfile::TempDir,
    root: PathBuf,
    service: AppearanceService,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let root = dir.path().join("AppearanceAssets");
    let repo = Arc::new(SqliteRepositories::new(db));
    let storage = Arc::new(LocalAppearanceAssetStorage::new(root.clone()));
    let service = AppearanceService::new(repo, storage);
    Fixture {
        _dir: dir,
        root,
        service,
    }
}

/// 受控目录里的全部文件名（排序后）：用来断言「字节」这一半事实。
fn stored_names(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}

/// 导入 → 删除的闭环：登记行与字节始终成对，且字节落在以同一个不透明 ID 命名的文件里
/// （存储按 ID 删除，文件名与登记行 ID 不一致就会留下永远删不掉的垃圾）。
#[tokio::test]
async fn appearance_import_then_delete_keeps_metadata_and_bytes_in_sync() {
    let fixture = fixture();
    let sources = tempfile::tempdir().unwrap();
    let source = sources.path().join("思源宋体 变体.ttf");
    fs::write(&source, TTF_BYTES).unwrap();

    let service = &fixture.service;
    let kind = AppearanceAssetKind::Font;
    let display = Some("  思源宋体  ".to_owned());
    let imported = service.import_asset(&source, kind, display).await.unwrap();
    let id = imported.metadata.id;

    let size = TTF_BYTES.len() as u64;
    assert_eq!(imported.metadata.byte_size, size);
    let name = imported.metadata.display_name.as_deref();
    assert_eq!(name, Some("思源宋体"), "展示名在服务层就已归一化");

    // 登记行与字节同时存在。源文件名（空格、中文、错扩展名）不参与落盘名。
    let listed = service.list_assets(None).await.unwrap();
    assert_eq!(listed, vec![imported.metadata.clone()]);
    let stored = format!("{id}.ttf");
    assert_eq!(stored_names(&fixture.root), vec![stored.clone()]);
    let path = fixture.root.join(&stored);
    assert_eq!(fs::read(&path).unwrap(), TTF_BYTES);

    let deleted = service.delete_asset(id).await.unwrap();
    assert!(deleted.deleted, "登记行被删除");
    assert!(deleted.file_removed, "字节按同一个 ID 被清理");
    let remaining = service.list_assets(None).await.unwrap();
    assert!(remaining.is_empty());
    assert!(stored_names(&fixture.root).is_empty());
}

/// 被存储层拒绝的导入不得留下任何一半：既没有登记行，也没有字节（连受控目录都不该被
/// 建出来）。同一份 PNG 换成匹配的种类才允许落盘。
#[tokio::test]
async fn appearance_rejected_import_leaves_neither_row_nor_bytes() {
    let fixture = fixture();
    let sources = tempfile::tempdir().unwrap();
    let disguised = sources.path().join("伪装字体.png");
    fs::write(&disguised, PNG_BYTES).unwrap();

    let service = &fixture.service;
    let kind = AppearanceAssetKind::Font;
    let result = service.import_asset(&disguised, kind, None).await;
    let error = result.unwrap_err();
    let code = error.code().as_str();
    assert_eq!(code, SIGNATURE_MISMATCH);
    let listed = service.list_assets(None).await.unwrap();
    assert!(listed.is_empty());
    assert!(!fixture.root.exists(), "签名不过时一个字节都不该落盘");

    let kind = AppearanceAssetKind::StaticWallpaper;
    let result = service.import_asset(&disguised, kind, None).await;
    let imported = result.unwrap();
    let stored = format!("{}.png", imported.metadata.id);
    assert_eq!(stored_names(&fixture.root), vec![stored]);
    assert_eq!(imported.metadata.kind, kind);
}

/// 删除未知 ID 是幂等空操作，且不得误伤真实存在的其它资产的字节。
#[tokio::test]
async fn appearance_delete_of_unknown_id_leaves_an_existing_asset_untouched() {
    let fixture = fixture();
    let sources = tempfile::tempdir().unwrap();
    let source = sources.path().join("a.ttf");
    fs::write(&source, TTF_BYTES).unwrap();

    let service = &fixture.service;
    let kind = AppearanceAssetKind::Font;
    let result = service.import_asset(&source, kind, None).await;
    let imported = result.unwrap();
    let kept = format!("{}.ttf", imported.metadata.id);

    for _ in 0..2 {
        let unknown = AppearanceAssetId::new();
        let deleted = service.delete_asset(unknown).await.unwrap();
        assert!(!deleted.deleted);
        assert!(!deleted.file_removed);
        assert_eq!(stored_names(&fixture.root), vec![kept.clone()]);
        let listed = service.list_assets(None).await.unwrap();
        assert_eq!(listed.len(), 1);
    }
}

/// 受控根目录跟随数据库所在的数据目录（与 `ArtworkCache::default_root` 同一套事实），
/// 且没有任何导入时不留下空目录。
#[test]
fn appearance_storage_default_root_follows_the_database_directory() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("haven.sqlite")).unwrap();
    let root = LocalAppearanceAssetStorage::default_root(&db);
    assert_eq!(root, dir.path().join("AppearanceAssets"));
    assert!(!root.exists(), "没有导入就不该留下空目录");
}

/// 总览布局在**真实 SQLite 文件**上的持久化闭环：写入 → 关库 → 重新打开同一个文件 →
/// 读回同一份排列与同一个 revision。
///
/// 这条用例是「重启后仍然生效」的唯一真实证据：内存库上的单元测试无论怎么写都证明不了
/// 这一点（进程结束数据就没了）。它同时经过 048 迁移的两张表（模块行 + meta 行）与
/// 领域校验，因此「落库形态能读回领域」也被一并钉住。
#[tokio::test]
async fn overview_layout_survives_reopening_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("haven.sqlite");

    // 会话一：保存一份自定义排列（两张图对调档位，证明读回的不是默认布局）。
    let saved = {
        let db = Arc::new(Db::open(&path).unwrap());
        let service = AppearanceService::new(
            Arc::new(SqliteRepositories::new(db)),
            Arc::new(LocalAppearanceAssetStorage::new(
                dir.path().join("AppearanceAssets"),
            )),
        );
        // 首次读取：从未保存过 → 领域默认布局 + revision = null。
        let initial = service.overview_layout_get().await.unwrap();
        assert_eq!(initial.revision, None);
        assert_eq!(initial.layout, OverviewLayout::default());

        let custom = OverviewLayout::new(
            OVERVIEW_LAYOUT_SCHEMA_VERSION,
            vec![
                OverviewModulePlacement::new(
                    OverviewModuleId::ReadingHeatmap,
                    OverviewModuleSize::Large,
                    0,
                    0,
                    0,
                )
                .unwrap(),
                OverviewModulePlacement::new(
                    OverviewModuleId::ReadingMinutes,
                    OverviewModuleSize::Small,
                    1,
                    0,
                    1,
                )
                .unwrap(),
                OverviewModulePlacement::new(
                    OverviewModuleId::TypeShare,
                    OverviewModuleSize::Medium,
                    1,
                    1,
                    2,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let saved = service.overview_layout_save(None, custom).await.unwrap();
        assert!(saved.changed);
        // 同一份布局重复保存是幂等的：changed=false 且 revision 不变。
        let repeated = service
            .overview_layout_save(saved.revision.as_deref(), saved.layout.clone())
            .await
            .unwrap();
        assert!(!repeated.changed);
        assert_eq!(repeated.revision, saved.revision);
        saved
    };

    // 会话二：重新打开同一个数据库文件，事实必须原样还在。
    let db = Arc::new(Db::open(&path).unwrap());
    let service = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db)),
        Arc::new(LocalAppearanceAssetStorage::new(
            dir.path().join("AppearanceAssets"),
        )),
    );
    let reopened = service.overview_layout_get().await.unwrap();
    assert_eq!(
        reopened.revision, saved.revision,
        "重开后 revision 必须不变"
    );
    assert_eq!(reopened.layout, saved.layout, "重开后排列必须原样读回");
    assert_ne!(
        reopened.layout,
        OverviewLayout::default(),
        "读回的必须是保存过的自定义排列，而不是默认布局"
    );

    // 重开后的 CAS 仍然认这份 revision：带上它就能改，带上过期的就零写入。
    let reset = service
        .overview_layout_reset(reopened.revision.as_deref())
        .await
        .unwrap();
    assert!(reset.changed);
    assert_eq!(reset.revision, None);
    assert_eq!(reset.layout, OverviewLayout::default());
    let after_reset = service.overview_layout_get().await.unwrap();
    assert_eq!(after_reset.revision, None);
    assert!(!after_reset.layout.modules.is_empty());
}

/// 隐藏全部模块（显式空布局）与「从未保存过」在真实存储上是两种不同事实：
/// 前者落库后读回 `Some(empty)`，后者读回 `None`（由 meta 行区分，见 048）。
#[tokio::test]
async fn overview_empty_layout_is_distinguishable_from_never_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("haven.sqlite");
    let db = Arc::new(Db::open(&path).unwrap());
    let service = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db)),
        Arc::new(LocalAppearanceAssetStorage::new(
            dir.path().join("AppearanceAssets"),
        )),
    );

    let before = service.overview_layout_get().await.unwrap();
    assert_eq!(before.revision, None);
    assert_eq!(before.layout.modules.len(), 5, "未保存过时是领域默认布局");

    let empty = OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, vec![]).unwrap();
    let saved = service.overview_layout_save(None, empty).await.unwrap();
    assert!(saved.changed);

    let after = service.overview_layout_get().await.unwrap();
    assert!(after.layout.modules.is_empty(), "显式空布局必须原样保留");
    assert!(
        after.revision.is_some(),
        "空布局是保存过的状态，不是「从未保存」"
    );
}

/// 一份占用格子重叠的总览布局必须在**写库之前**被服务层拒绝，且库里不留下任何保存状态。
///
/// 这份状态只能绕过校验构造器造出来：`OverviewLayout::new` 会拒绝同一个形状（下面先钉住
/// 这一点）。但字段是 pub（存储读取侧与 wire 反序列化都构造它），所以服务层不能假设
/// 「拿到的布局一定合法」——这条用例走真实 SQLite（048）验证的正是那个假设不成立时的行为。
#[tokio::test]
async fn overview_overlapping_layout_is_rejected_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("haven.sqlite")).unwrap());
    let service = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db)),
        Arc::new(LocalAppearanceAssetStorage::new(
            dir.path().join("AppearanceAssets"),
        )),
    );

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
        OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, overlapping.modules.clone()).is_err(),
        "同一个形状必须被领域的校验构造器拒绝，否则这条用例测的不是「绕过校验器」"
    );

    let error = service
        .overview_layout_save(None, overlapping)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "APPEARANCE_INVALID_OVERVIEW_LAYOUT");

    // 从未保存过：被拒绝的写入不得留下 revision。
    let snapshot = service.overview_layout_get().await.unwrap();
    assert_eq!(snapshot.revision, None);
    assert_eq!(snapshot.layout, OverviewLayout::default());
}

// ---------- 首页布局：真实 SQLite 文件上的重开闭环（046 的两张表） ----------

/// 首页布局在**真实 SQLite 文件**上的持久化闭环：写入 → 关库 → 重新打开同一个文件 →
/// 读回同一份排列与同一个 revision。
///
/// 与总览布局那条同形，但它走的是一组**不同**的表（`appearance_home_*` 与
/// `appearance_overview_*` 各自独立）：只测其中一条，另一条的表结构或读取顺序改坏了
/// 也不会有任何测试发现。
#[tokio::test]
async fn home_layout_survives_reopening_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("haven.sqlite");
    let assets = dir.path().join("AppearanceAssets");

    let saved = {
        let db = Arc::new(Db::open(&path).unwrap());
        let service = AppearanceService::new(
            Arc::new(SqliteRepositories::new(db)),
            Arc::new(LocalAppearanceAssetStorage::new(assets.clone())),
        );
        let initial = service.home_layout_get().await.unwrap();
        assert_eq!(initial.revision, None);
        assert_eq!(initial.layout, HomeLayout::default());

        // 一份打包算法产不出来的合法排列（行与行之间有空档）：读回来的必须正是它，
        // 而不是任何「按模块清单重算」的结果。
        let custom = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                HomeModulePlacement::new(
                    HomeModuleId::ShelfFavorites,
                    HomeModuleSize::Small,
                    2,
                    2,
                    0,
                )
                .unwrap(),
                HomeModulePlacement::new(
                    HomeModuleId::RecentlyAdded,
                    HomeModuleSize::Medium,
                    0,
                    0,
                    1,
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let saved = service.home_layout_save(None, custom).await.unwrap();
        assert!(saved.changed);
        let repeated = service
            .home_layout_save(saved.revision.as_deref(), saved.layout.clone())
            .await
            .unwrap();
        assert!(!repeated.changed, "同一份首页布局重复保存必须幂等");
        assert_eq!(repeated.revision, saved.revision);
        saved
    };

    let db = Arc::new(Db::open(&path).unwrap());
    let service = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db)),
        Arc::new(LocalAppearanceAssetStorage::new(assets)),
    );
    let reopened = service.home_layout_get().await.unwrap();
    assert_eq!(
        reopened.revision, saved.revision,
        "重开后 revision 必须不变"
    );
    assert_eq!(reopened.layout, saved.layout, "重开后排列必须原样读回");
    assert_ne!(
        reopened.layout,
        HomeLayout::default(),
        "读回的必须是保存过的自定义排列，而不是默认布局"
    );

    // 重开后的 CAS 仍然认这份 revision；过期的 revision 依然零写入。
    let error = service
        .home_layout_save(None, HomeLayout::default())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
    let reset = service
        .home_layout_reset(reopened.revision.as_deref())
        .await
        .unwrap();
    assert!(reset.changed);
    assert_eq!(reset.revision, None);
    assert_eq!(reset.layout, HomeLayout::default());
}

// ---------- 外观设置与资产：真实 SQLite 文件上的重开闭环（045 + settings 行） ----------

/// 外观**设置**（字体资产引用 + 主题）与资产**登记行**必须一起活过重启。
///
/// 这两半住在不同的表里（`settings.data_json` 与 `appearance_assets`），而它们之间没有
/// 外键：只有把两侧都真的重开一遍，才能证明「设置里那个 assetId 在重开后仍然解析得到
/// 一份真实字节」。只测一侧，另一侧的代表性漂移（比如资产 ID 换了一种文本形态）不会被
/// 任何内存库用例发现。
#[tokio::test]
async fn appearance_settings_and_assets_survive_reopening_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("haven.sqlite");
    let assets = dir.path().join("AppearanceAssets");
    let sources = tempfile::tempdir().unwrap();
    let font_source = sources.path().join("思源宋体 变体.ttf");
    fs::write(&font_source, TTF_BYTES).unwrap();

    let (font_id, saved_revision) = {
        let db = Arc::new(Db::open(&path).unwrap());
        let appearance = AppearanceService::new(
            Arc::new(SqliteRepositories::new(db.clone())),
            Arc::new(LocalAppearanceAssetStorage::new(assets.clone())),
        );
        let settings = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())));

        let imported = appearance
            .import_asset(&font_source, AppearanceAssetKind::Font, None)
            .await
            .unwrap();
        let updated = settings
            .update(
                SettingsSection::Appearance,
                None,
                SettingsPatch::Appearance(AppearancePatch {
                    theme: Some(Theme::Dark),
                    custom_font_asset_id: Some(Some(imported.metadata.id)),
                    ..AppearancePatch::default()
                }),
            )
            .await
            .unwrap();
        assert!(updated.changed);
        (imported.metadata.id, updated.revision)
    };

    let db = Arc::new(Db::open(&path).unwrap());
    let appearance = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db.clone())),
        Arc::new(LocalAppearanceAssetStorage::new(assets)),
    );
    let settings = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())));

    let snapshot = settings.get(SettingsSection::Appearance).await.unwrap();
    assert_eq!(
        snapshot.revision, saved_revision,
        "重开后 revision 必须不变"
    );
    let SettingsValue::Appearance(value) = snapshot.value else {
        panic!("appearance 分区必须读回 appearance 设置");
    };
    assert_eq!(value.theme, Theme::Dark);
    assert_eq!(
        value.custom_font_asset_id,
        Some(font_id),
        "重开后字体资产引用必须原样还在"
    );

    // 登记行还在，字节也仍然按同一个 ID 解析得到，且长度与登记事实一致。
    let listed = appearance.list_assets(None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, font_id);
    assert_eq!(listed[0].byte_size, TTF_BYTES.len() as u64);
    let opened = appearance.open_asset(font_id).await.unwrap();
    assert_eq!(opened.kind, AppearanceAssetKind::Font);
}

/// 打开资产时「登记行与字节对不上」是**既成事实**，不是一次可重试的读取故障。
///
/// 错误码与可重试性必须分开报：报成可重试会让页面上那个「重试预览」变成一个注定失败的
/// 动作；报成不可重试才如实说明「这份资产现在就是不可用」。字节被换掉是这条性质唯一
/// 可复现的构造方式（真实文件系统、真实登记行）。
#[tokio::test]
async fn opening_an_asset_whose_bytes_drifted_from_the_registration_is_refused() {
    let fixture = fixture();
    let sources = tempfile::tempdir().unwrap();
    let source = sources.path().join("思源宋体.ttf");
    fs::write(&source, TTF_BYTES).unwrap();

    let service = &fixture.service;
    let imported = service
        .import_asset(&source, AppearanceAssetKind::Font, None)
        .await
        .unwrap();
    let id = imported.metadata.id;
    assert!(service.open_asset(id).await.is_ok(), "登记与字节一致时可读");

    // 受控目录里的字节被换成一个更短的版本：登记行没变，事实对不上了。
    let stored = fixture.root.join(format!("{id}.ttf"));
    fs::write(&stored, &TTF_BYTES[..4]).unwrap();

    let error = service
        .open_asset(id)
        .await
        .err()
        .expect("字节与登记信息不一致时必须拒绝打开");
    assert_eq!(error.code().as_str(), "APPEARANCE_ASSET_UNAVAILABLE");
    assert!(
        !error.retryable(),
        "登记行与字节对不上是既成事实，重试多少次都是同一个结果"
    );

    // 未知 ID 同样是「不可用」而不是「暂时读不到」。
    let unknown = service
        .open_asset(AppearanceAssetId::new())
        .await
        .err()
        .expect("不存在的资产 ID 必须被拒绝");
    assert_eq!(unknown.code().as_str(), "APPEARANCE_ASSET_UNAVAILABLE");
    assert!(!unknown.retryable());
}

/// 删除守卫读的是 `settings.appearance` 这行 JSON 里**真实写下**的两条引用路径。
///
/// 这条用例把两半事实接在一起：外观设置由真正的 `SettingsService` 写进 settings 表，
/// 占用判定由真正的 `SqliteAppearanceRepository` 用 `json_extract` 读回来。任何一半的
/// 字段名漂移（领域声明的路径、序列化形态、SQL 里的路径）都会在这里变成「删除被放行」，
/// 也就是一条指着已删除资产的外观设置。
#[tokio::test]
async fn deleting_an_asset_referenced_by_the_saved_appearance_row_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("haven.sqlite")).unwrap());
    let assets = dir.path().join("AppearanceAssets");
    let service = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db.clone())),
        Arc::new(LocalAppearanceAssetStorage::new(assets.clone())),
    );
    let settings = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())));

    let sources = tempfile::tempdir().unwrap();
    let font_source = sources.path().join("字体.ttf");
    let wallpaper_source = sources.path().join("壁纸.png");
    fs::write(&font_source, TTF_BYTES).unwrap();
    fs::write(&wallpaper_source, PNG_BYTES).unwrap();
    let font = service
        .import_asset(&font_source, AppearanceAssetKind::Font, None)
        .await
        .unwrap();
    let wallpaper = service
        .import_asset(
            &wallpaper_source,
            AppearanceAssetKind::StaticWallpaper,
            None,
        )
        .await
        .unwrap();

    // 两处引用同时写进外观分区：字体资产 + 静态壁纸。
    let saved = settings
        .update(
            SettingsSection::Appearance,
            None,
            SettingsPatch::Appearance(AppearancePatch {
                wallpaper: Some(Some(WallpaperSelection::Static(wallpaper.metadata.id))),
                custom_font_asset_id: Some(Some(font.metadata.id)),
                ..AppearancePatch::default()
            }),
        )
        .await
        .unwrap();
    assert!(saved.changed);

    let font_error = service.delete_asset(font.metadata.id).await.unwrap_err();
    assert_eq!(font_error.code().as_str(), APPEARANCE_ASSET_IN_USE);
    assert!(font_error.user_message().contains("界面字体"));
    let wallpaper_error = service
        .delete_asset(wallpaper.metadata.id)
        .await
        .unwrap_err();
    assert_eq!(wallpaper_error.code().as_str(), APPEARANCE_ASSET_IN_USE);
    assert!(wallpaper_error.user_message().contains("首页壁纸"));

    // 被挡住 = 零写入：设置行、登记行与字节三样都原封不动。
    let after = settings.get(SettingsSection::Appearance).await.unwrap();
    assert_eq!(after.revision, saved.revision);
    assert_eq!(after.value, saved.value);
    assert_eq!(
        service.list_assets(None).await.unwrap().len(),
        2,
        "被挡住的删除不得动到任何一条登记行"
    );
    assert_eq!(stored_names(&assets).len(), 2, "被挡住的删除不得删字节");

    // 解除两处引用（显式 null = 清除，与另外两个可空字段同形）之后，同一次删除就会成功。
    let cleared = settings
        .update(
            SettingsSection::Appearance,
            saved.revision.as_deref(),
            SettingsPatch::Appearance(AppearancePatch {
                wallpaper: Some(None),
                custom_font_asset_id: Some(None),
                ..AppearancePatch::default()
            }),
        )
        .await
        .unwrap();
    let SettingsValue::Appearance(value) = cleared.value else {
        panic!("appearance 分区必须读回 appearance 设置");
    };
    assert_eq!(
        value.wallpaper,
        WallpaperSelection::None,
        "显式 null 必须清除壁纸"
    );
    assert_eq!(
        value.custom_font_asset_id, None,
        "显式 null 必须清除字体引用"
    );

    assert!(
        service
            .delete_asset(font.metadata.id)
            .await
            .unwrap()
            .deleted
    );
    assert!(
        service
            .delete_asset(wallpaper.metadata.id)
            .await
            .unwrap()
            .deleted
    );
    assert!(stored_names(&assets).is_empty(), "引用解除后字节也要清掉");
}

/// 双连接并发：一边删资产、一边把同一个资产写进外观设置。
///
/// 两条路径各自在**自己的** SQLite 写事务里判定（删除侧 `BEGIN IMMEDIATE` + 占用判定，
/// 设置侧同事务内的资产行校验），所以结果只可能是这两种：
/// - 设置先提交 → 删除读到引用，被 `APPEARANCE_ASSET_IN_USE` 挡住；
/// - 删除先提交 → 设置写不进去，被 `APPEARANCE_ASSET_UNAVAILABLE` 挡住。
///
/// 这里断言的是**不变量**而不是某一种交错：无论 SQLite 把两条事务排成什么顺序，结束后
/// 都不允许出现「外观设置里留着一条指向已删除资产（或反过来，删掉了还在被引用的资产）」。
/// 旧实现把占用判定与删除分成两次独立提交，靠的是「本进程只有一条连接」这个偶然事实，
/// 换一条连接就能把引用插进两次提交之间——这条用例正是那个窗口的回归。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_delete_and_reference_write_never_leave_a_dangling_reference() {
    use std::sync::Barrier;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("haven.sqlite");
    let assets = dir.path().join("AppearanceAssets");
    let sources = tempfile::tempdir().unwrap();
    let source = sources.path().join("并发字体.ttf");
    fs::write(&source, TTF_BYTES).unwrap();

    // 连接 A：建库 + 迁移 + 导入一个字体资产。
    let db_a = Arc::new(Db::open(&path).unwrap());
    let appearance_a = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db_a.clone())),
        Arc::new(LocalAppearanceAssetStorage::new(assets.clone())),
    );
    let imported = appearance_a
        .import_asset(&source, AppearanceAssetKind::Font, None)
        .await
        .unwrap();
    let font_id = imported.metadata.id;

    // 连接 B：独立打开同一个文件（真实的第二个连接，不是同一个 `Db` 的两次借用）。
    let db_b = Arc::new(Db::open(&path).unwrap());
    let settings_b = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db_b.clone())));

    let barrier = Arc::new(Barrier::new(2));
    let handle = tokio::runtime::Handle::current();

    let barrier_delete = barrier.clone();
    let handle_delete = handle.clone();
    let appearance_delete = appearance_a.clone();
    let delete_thread = std::thread::spawn(move || {
        barrier_delete.wait();
        let deleted = appearance_delete.delete_asset(font_id);
        handle_delete.block_on(deleted)
    });

    let barrier_write = barrier;
    let handle_write = handle.clone();
    let write_thread = std::thread::spawn(move || {
        barrier_write.wait();
        handle_write.block_on(async move {
            settings_b
                .update(
                    SettingsSection::Appearance,
                    None,
                    SettingsPatch::Appearance(AppearancePatch {
                        custom_font_asset_id: Some(Some(font_id)),
                        ..AppearancePatch::default()
                    }),
                )
                .await
        })
    });

    let deleted = delete_thread.join().expect("删除线程不应 panic");
    let written = write_thread.join().expect("设置线程不应 panic");

    // 失败的一侧必须是稳定错误码，不得把 BUSY 之类的 SQLite 细节泄漏成 DATABASE_ERROR
    // （两条事务都很短，busy_timeout 足够让后到者排队而不是失败）。
    if let Err(error) = &deleted {
        assert_eq!(error.code().as_str(), APPEARANCE_ASSET_IN_USE);
    }
    if let Err(error) = &written {
        assert_eq!(error.code().as_str(), "APPEARANCE_ASSET_UNAVAILABLE");
    }

    // 第三条连接读最终状态：不变量在这里被钉住。
    let db_check = Arc::new(Db::open(&path).unwrap());
    let appearance_check = AppearanceService::new(
        Arc::new(SqliteRepositories::new(db_check.clone())),
        Arc::new(LocalAppearanceAssetStorage::new(assets)),
    );
    let settings_check = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db_check.clone())));
    let snapshot = settings_check
        .get(SettingsSection::Appearance)
        .await
        .unwrap();
    let SettingsValue::Appearance(value) = snapshot.value else {
        panic!("appearance 分区必须读回 appearance 设置");
    };
    let registered = appearance_check
        .list_assets(Some(AppearanceAssetKind::Font))
        .await
        .unwrap();
    let still_registered = registered.iter().any(|asset| asset.id == font_id);
    assert!(
        !(value.custom_font_asset_id == Some(font_id) && !still_registered),
        "并发结束后不得留下指向已删除资产的外观设置"
    );

    // 且不可能两边都原地不动：一方成功才会让另一方被挡住。
    let asset_deleted = deleted.is_ok_and(|outcome| outcome.deleted);
    assert!(asset_deleted || written.is_ok(), "删除与写入不可能都没发生");
}
