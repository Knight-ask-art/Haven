//! 外观 Commands（契约 §12；Appearance Stage 1B 的 Tauri 接线）。
//!
//! Command 边界（ADR-002 / IPC-TAURI-001A）：
//! - 公开签名只出现**不透明资产 ID**、typed DTO 与布局事实；没有任何参数能承载
//!   路径或 URL。文件来源只能由后端 Native 选择器（rfd）决定，选中的路径只在本模块
//!   内部传给 `AppearanceService`，永不进入 Wire、前端状态或日志。
//! - 布局写入走 wire → 领域的严格 `TryFrom`（越界坐标/重叠/重复/排序号冲突都在这里
//!   被拒绝），并发控制是服务层的 `expectedRevision` CAS。
//! - 同步 SQLite 与文件系统只在 `run_blocking` 提供的 blocking worker 上执行。
//! - 错误统一映射为 `ErrorDto`；非法 ID 用既有 `INVALID_ID`，取消用稳定
//!   `OPERATION_CANCELLED`。

use std::path::PathBuf;

use tauri::State;

use haven_application::services::appearance::{HomeLayoutResult, OverviewLayoutResult};
use haven_application::wire::{
    AppearanceAssetDeleteRequest, AppearanceAssetDeleteResultDto, AppearanceAssetDto,
    AppearanceAssetImportRequest, AppearanceAssetsDto, AppearanceAssetsListRequest, ErrorDto,
    HomeLayoutMutationResultDto, HomeLayoutResetRequest, HomeLayoutSaveRequest,
    HomeLayoutSnapshotDto, OverviewLayoutMutationResultDto, OverviewLayoutResetRequest,
    OverviewLayoutSaveRequest, OverviewLayoutSnapshotDto,
};
use haven_domain::appearance::{
    appearance_display_name_from_file_stem, AppearanceAssetId, AppearanceAssetKind, HomeLayout,
    OverviewLayout,
};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

/// 资产列表（`kind` 为 `null` 表示不按种类过滤）。
pub async fn run_appearance_assets_list(
    state: &AppState,
    request: AppearanceAssetsListRequest,
) -> Result<AppearanceAssetsDto, ErrorDto> {
    let kind = request.kind.map(AppearanceAssetKind::from);
    let listed = state
        .appearance
        .list_assets(kind)
        .await
        .map_err(|error| to_error_dto(&error))?;
    let assets: Vec<AppearanceAssetDto> = listed.into_iter().map(Into::into).collect();
    Ok(AppearanceAssetsDto {
        schema_version: 1,
        assets,
    })
}

/// 导入核心：`source` 只来自 Native 文件选择器（`displayName` / `kind` 来自 typed
/// request，前端提交不了文件位置）。校验、落盘与登记的顺序由服务层负责。
pub async fn run_appearance_asset_import(
    state: &AppState,
    request: AppearanceAssetImportRequest,
    source: PathBuf,
) -> Result<AppearanceAssetDto, ErrorDto> {
    let imported = state
        .appearance
        .import_asset(&source, request.kind.into(), request.display_name)
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(imported.metadata.into())
}

/// Native 文件选择 + 导入（用户取消 → 稳定 `OPERATION_CANCELLED`）。
///
/// 选择器按请求的 `kind` 过滤扩展名（[`AppearanceAssetKind::picker_extensions`]）：
/// 这只是把明显不可能的候选挡在对话框外，让用户不必先选中一个必然被拒的文件再读错误
/// 信息。它**不是**校验，也不构成任何安全边界——真正的判定仍然是导入路径上的字节签名
/// 检测，伪造扩展名的文件照样在那里被拒。
///
/// 展示名：请求带了的用它，没带（或只有一个空白串）就取所选文件的**文件名词干**并归一化
/// 到安全形态（[`appearance_display_name_from_file_stem`]）。不取用它，界面上每个资产
/// 都只能叫「资产 0196f0d2」；而把文件名原样塞进去，又等于让一个外部输入直接变成库里的
/// 展示名（可能超长、含控制字符或会伪造视觉顺序的格式字符）。归一化之后什么都不剩时
/// 如实回落到「没有展示名」。
async fn pick_asset_and_import(
    state: &AppState,
    request: AppearanceAssetImportRequest,
) -> Result<AppearanceAssetDto, ErrorDto> {
    let kind = AppearanceAssetKind::from(request.kind);
    let requested_display_name = request.display_name.filter(|name| !name.trim().is_empty());
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let extensions = kind.picker_extensions().to_vec();
        rfd::FileDialog::new()
            .add_filter(kind.label(), &extensions)
            .pick_file()
    })
    .await
    .map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "文件选择器执行失败".into(),
        retryable: false,
    })?;
    let Some(source) = picked else {
        return Err(ErrorDto {
            code: "OPERATION_CANCELLED".into(),
            user_message: "已取消选择文件".into(),
            retryable: false,
        });
    };
    let display_name = requested_display_name.or_else(|| {
        source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(appearance_display_name_from_file_stem)
    });
    run_appearance_asset_import(
        state,
        AppearanceAssetImportRequest {
            kind: request.kind,
            display_name,
        },
        source,
    )
    .await
}

/// 删除资产（只有不透明 ID；未知 ID 是幂等空操作，不是错误）。
pub async fn run_appearance_asset_delete(
    state: &AppState,
    request: AppearanceAssetDeleteRequest,
) -> Result<AppearanceAssetDeleteResultDto, ErrorDto> {
    // 只接受规范小写连字符 UUID：同一个资产只有一种文本身份。
    let Some(asset_id) = AppearanceAssetId::parse(&request.asset_id) else {
        return Err(invalid_id());
    };
    let deleted = state
        .appearance
        .delete_asset(asset_id)
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(AppearanceAssetDeleteResultDto {
        deleted: deleted.deleted,
        file_removed: deleted.file_removed,
    })
}

/// 读取首页布局；`revision` 为 `null` 表示从未保存过自定义（返回领域默认布局）。
pub async fn run_home_layout_get(state: &AppState) -> Result<HomeLayoutSnapshotDto, ErrorDto> {
    let result = state
        .appearance
        .home_layout_get()
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(HomeLayoutSnapshotDto {
        layout: result.layout.into(),
        revision: result.revision,
    })
}

/// 保存首页布局（`expectedRevision` CAS；对不上 → `REVISION_CONFLICT` 且零写入）。
pub async fn run_home_layout_save(
    state: &AppState,
    request: HomeLayoutSaveRequest,
) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
    // wire → 领域的严格转换：越界坐标、重叠、重复模块与重复排序号都在这里被拒绝。
    let layout = HomeLayout::try_from(request.layout).map_err(|error| to_error_dto(&error))?;
    let result = state
        .appearance
        .home_layout_save(request.expected_revision.as_deref(), layout)
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(mutation_dto(result))
}

/// 重置首页布局（回到领域默认；从未保存过时是幂等空操作）。
pub async fn run_home_layout_reset(
    state: &AppState,
    request: HomeLayoutResetRequest,
) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
    let result = state
        .appearance
        .home_layout_reset(request.expected_revision.as_deref())
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(mutation_dto(result))
}

fn mutation_dto(result: HomeLayoutResult) -> HomeLayoutMutationResultDto {
    HomeLayoutMutationResultDto {
        layout: result.layout.into(),
        revision: result.revision,
        changed: result.changed,
    }
}

/// 读取总览布局；`revision` 为 `null` 表示从未保存过自定义（返回领域默认布局）。
///
/// 总览布局与首页布局是两条独立的命令通道：前端不会因为「首页读到了」就以为总览也读到了。
pub async fn run_overview_layout_get(
    state: &AppState,
) -> Result<OverviewLayoutSnapshotDto, ErrorDto> {
    let result = state
        .appearance
        .overview_layout_get()
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(OverviewLayoutSnapshotDto {
        layout: result.layout.into(),
        revision: result.revision,
    })
}

/// 保存总览布局（`expectedRevision` CAS；对不上 → `REVISION_CONFLICT` 且零写入）。
pub async fn run_overview_layout_save(
    state: &AppState,
    request: OverviewLayoutSaveRequest,
) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
    // wire → 领域的严格转换：越界坐标（三列网格）、重叠、重复模块与重复排序号都在这里被拒绝。
    let layout = OverviewLayout::try_from(request.layout).map_err(|error| to_error_dto(&error))?;
    let result = state
        .appearance
        .overview_layout_save(request.expected_revision.as_deref(), layout)
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(overview_mutation_dto(result))
}

/// 重置总览布局（回到领域默认；从未保存过时是幂等空操作）。
pub async fn run_overview_layout_reset(
    state: &AppState,
    request: OverviewLayoutResetRequest,
) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
    let result = state
        .appearance
        .overview_layout_reset(request.expected_revision.as_deref())
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(overview_mutation_dto(result))
}

fn overview_mutation_dto(result: OverviewLayoutResult) -> OverviewLayoutMutationResultDto {
    OverviewLayoutMutationResultDto {
        layout: result.layout.into(),
        revision: result.revision,
        changed: result.changed,
    }
}

// ---- Tauri Command 薄包装（公开签名不含任何路径参数）----

#[tauri::command]
pub async fn appearance_assets_list(
    state: State<'_, AppState>,
    request: AppearanceAssetsListRequest,
) -> Result<AppearanceAssetsDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_appearance_assets_list(&state, request).await }).await
}

#[tauri::command]
pub async fn appearance_asset_import(
    state: State<'_, AppState>,
    request: AppearanceAssetImportRequest,
) -> Result<AppearanceAssetDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { pick_asset_and_import(&state, request).await }).await
}

#[tauri::command]
pub async fn appearance_asset_delete(
    state: State<'_, AppState>,
    request: AppearanceAssetDeleteRequest,
) -> Result<AppearanceAssetDeleteResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_appearance_asset_delete(&state, request).await }).await
}

#[tauri::command]
pub async fn home_layout_get(
    state: State<'_, AppState>,
) -> Result<HomeLayoutSnapshotDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_home_layout_get(&state).await }).await
}

#[tauri::command]
pub async fn home_layout_save(
    state: State<'_, AppState>,
    request: HomeLayoutSaveRequest,
) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_home_layout_save(&state, request).await }).await
}

#[tauri::command]
pub async fn home_layout_reset(
    state: State<'_, AppState>,
    request: HomeLayoutResetRequest,
) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_home_layout_reset(&state, request).await }).await
}

#[tauri::command]
pub async fn overview_layout_get(
    state: State<'_, AppState>,
) -> Result<OverviewLayoutSnapshotDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_overview_layout_get(&state).await }).await
}

#[tauri::command]
pub async fn overview_layout_save(
    state: State<'_, AppState>,
    request: OverviewLayoutSaveRequest,
) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_overview_layout_save(&state, request).await }).await
}

#[tauri::command]
pub async fn overview_layout_reset(
    state: State<'_, AppState>,
    request: OverviewLayoutResetRequest,
) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_overview_layout_reset(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use haven_application::wire::{AppearanceAssetKindDto, HomeLayoutDto, OverviewLayoutDto};
    use haven_infrastructure::Db;

    fn app_state() -> AppState {
        AppState::new(Arc::new(Db::open_in_memory().unwrap()))
    }

    /// 领域默认布局的 wire 形态（合法布局的现成样本）。
    fn default_layout_dto() -> HomeLayoutDto {
        HomeLayoutDto::from(HomeLayout::default())
    }

    async fn save_layout(
        state: &AppState,
        expected_revision: Option<String>,
        layout: HomeLayoutDto,
    ) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
        let request = HomeLayoutSaveRequest {
            expected_revision,
            layout,
        };
        run_home_layout_save(state, request).await
    }

    async fn reset_layout(
        state: &AppState,
        expected_revision: Option<String>,
    ) -> Result<HomeLayoutMutationResultDto, ErrorDto> {
        let request = HomeLayoutResetRequest { expected_revision };
        run_home_layout_reset(state, request).await
    }

    async fn delete_asset(
        state: &AppState,
        asset_id: &str,
    ) -> Result<AppearanceAssetDeleteResultDto, ErrorDto> {
        let request = AppearanceAssetDeleteRequest {
            asset_id: asset_id.to_owned(),
        };
        run_appearance_asset_delete(state, request).await
    }

    /// 空库的资产列表：`schemaVersion` 恒为 1，按任何种类过滤后仍然为空（不伪造资产）。
    #[tokio::test]
    async fn asset_list_is_empty_for_every_kind_filter() {
        let state = app_state();
        for kind in [
            None,
            Some(AppearanceAssetKindDto::Font),
            Some(AppearanceAssetKindDto::StaticWallpaper),
            Some(AppearanceAssetKindDto::DynamicWallpaper),
        ] {
            let request = AppearanceAssetsListRequest { kind };
            let listed = run_appearance_assets_list(&state, request).await.unwrap();
            assert_eq!(listed.schema_version, 1);
            assert!(listed.assets.is_empty(), "空库不得列出资产: {kind:?}");
        }
    }

    /// 非规范 UUID（形态不符 / 大写 / 无连字符 / URN）一律 `INVALID_ID`，且不触碰存储。
    #[tokio::test]
    async fn delete_with_non_canonical_asset_id_is_invalid_id() {
        let state = app_state();
        let canonical = AppearanceAssetId::new().to_string();
        let invalids = [
            "not-a-uuid".to_owned(),
            String::new(),
            canonical.to_uppercase(),
            canonical.replace('-', ""),
            "urn:uuid:1".to_owned(),
        ];
        for raw in &invalids {
            let error = delete_asset(&state, raw).await.unwrap_err();
            assert_eq!(error.code, "INVALID_ID", "非法 ID 必须稳定映射: {raw}");
            assert!(!error.retryable);
        }
    }

    /// 未知（但合法）资产 ID 的删除是幂等空操作：不是错误，也不谎报删掉了字节。
    #[tokio::test]
    async fn delete_of_unknown_asset_reports_nothing_removed() {
        let state = app_state();
        let asset_id = AppearanceAssetId::new().to_string();
        let deleted = delete_asset(&state, &asset_id).await.unwrap();
        assert!(!deleted.deleted);
        assert!(!deleted.file_removed);
    }

    /// 从未保存过自定义布局：读取返回领域默认布局 + `revision = null`。
    #[tokio::test]
    async fn layout_get_falls_back_to_the_domain_default() {
        let state = app_state();
        let snapshot = run_home_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, None);
        assert_eq!(snapshot.layout, default_layout_dto());
    }

    /// 非法布局（占用格子重叠）必须在写库前被拒绝，且不留下任何保存状态。
    #[tokio::test]
    async fn layout_save_rejects_an_invalid_layout_without_writing() {
        let state = app_state();
        // 把第二个模块压到第一个模块占用的格子上：占格重叠必须被拒绝。
        let mut layout = default_layout_dto();
        layout.modules[1].row = layout.modules[0].row;
        layout.modules[1].column = layout.modules[0].column;

        let error = save_layout(&state, None, layout).await.unwrap_err();
        assert_eq!(error.code, "APPEARANCE_INVALID_HOME_LAYOUT");
        assert!(!error.retryable);

        let snapshot = run_home_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, None, "非法布局不得产生 revision");
    }

    /// `expectedRevision` CAS：陈旧 revision 的保存与重置都必须 `REVISION_CONFLICT`，
    /// 并保持已保存的布局原样（零写入）。
    #[tokio::test]
    async fn layout_save_and_reset_conflict_on_a_stale_revision() {
        let state = app_state();
        let layout = default_layout_dto();
        let saved = save_layout(&state, None, layout).await.unwrap();
        assert!(saved.changed);
        let revision = saved.revision.clone();
        assert!(revision.is_some(), "首次保存必须产生 revision");

        // 陈旧 revision（这里回传 `null`）的保存不得写入。
        let mut empty = default_layout_dto();
        empty.modules.clear();
        let error = save_layout(&state, None, empty).await.unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");

        let error = reset_layout(&state, None).await.unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");

        let snapshot = run_home_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, revision);
        assert_eq!(snapshot.layout, saved.layout);

        let reset = reset_layout(&state, snapshot.revision).await.unwrap();
        assert!(reset.changed);
        assert_eq!(reset.revision, None);
        assert_eq!(reset.layout, default_layout_dto());
    }

    // ---------- 总览布局（048）----------

    /// 领域默认总览布局的 wire 形态（合法布局的现成样本）。
    fn default_overview_layout_dto() -> OverviewLayoutDto {
        OverviewLayoutDto::from(OverviewLayout::default())
    }

    async fn save_overview_layout(
        state: &AppState,
        expected_revision: Option<String>,
        layout: OverviewLayoutDto,
    ) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
        let request = OverviewLayoutSaveRequest {
            expected_revision,
            layout,
        };
        run_overview_layout_save(state, request).await
    }

    async fn reset_overview_layout(
        state: &AppState,
        expected_revision: Option<String>,
    ) -> Result<OverviewLayoutMutationResultDto, ErrorDto> {
        let request = OverviewLayoutResetRequest { expected_revision };
        run_overview_layout_reset(state, request).await
    }

    /// 从未保存过自定义总览：读取返回领域默认布局（五个真实内容块）+ `revision = null`。
    #[tokio::test]
    async fn overview_layout_get_falls_back_to_the_domain_default() {
        let state = app_state();
        let snapshot = run_overview_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, None);
        assert_eq!(snapshot.layout, default_overview_layout_dto());
        assert_eq!(snapshot.layout.modules.len(), 5);
    }

    /// 非法总览布局（占用格子重叠）必须在写库前被拒绝，且不留下任何保存状态。
    #[tokio::test]
    async fn overview_layout_save_rejects_an_invalid_layout_without_writing() {
        let state = app_state();
        // large 占满整行，small 落在它覆盖的最后一列：起始格不同，占格却重叠。
        let mut layout = default_overview_layout_dto();
        layout.modules[3].row = layout.modules[2].row;
        layout.modules[3].column = layout.modules[2].column;

        let error = save_overview_layout(&state, None, layout)
            .await
            .unwrap_err();
        assert_eq!(error.code, "APPEARANCE_INVALID_OVERVIEW_LAYOUT");
        assert!(!error.retryable);

        let snapshot = run_overview_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, None, "非法布局不得产生 revision");
    }

    /// `expectedRevision` CAS：陈旧 revision 的保存与重置都必须 `REVISION_CONFLICT`，
    /// 并保持已保存的布局原样（零写入）。
    #[tokio::test]
    async fn overview_layout_save_and_reset_conflict_on_a_stale_revision() {
        let state = app_state();
        let saved = save_overview_layout(&state, None, default_overview_layout_dto())
            .await
            .unwrap();
        assert!(saved.changed);
        let revision = saved.revision.clone();
        assert!(revision.is_some(), "首次保存必须产生 revision");

        let mut empty = default_overview_layout_dto();
        empty.modules.clear();
        let error = save_overview_layout(&state, None, empty).await.unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");

        let error = reset_overview_layout(&state, None).await.unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");

        let snapshot = run_overview_layout_get(&state).await.unwrap();
        assert_eq!(snapshot.revision, revision);
        assert_eq!(snapshot.layout, saved.layout);

        let reset = reset_overview_layout(&state, snapshot.revision)
            .await
            .unwrap();
        assert!(reset.changed);
        assert_eq!(reset.revision, None);
        assert_eq!(reset.layout, default_overview_layout_dto());
    }

    /// 同一份布局重复保存是幂等的（`changed = false`），且总览与首页两条通道互不影响：
    /// 保存总览不会给首页布局造出状态，反之亦然。
    #[tokio::test]
    async fn overview_layout_save_is_idempotent_and_independent_from_home_layout() {
        let state = app_state();
        let first = save_overview_layout(&state, None, default_overview_layout_dto())
            .await
            .unwrap();
        assert!(first.changed);
        let second = save_overview_layout(
            &state,
            first.revision.clone(),
            default_overview_layout_dto(),
        )
        .await
        .unwrap();
        assert!(!second.changed, "同一份总览布局重复保存不得判为变化");
        assert_eq!(second.revision, first.revision);

        let home = run_home_layout_get(&state).await.unwrap();
        assert_eq!(home.revision, None, "保存总览布局不得产生首页布局状态");
        assert_eq!(home.layout, default_layout_dto());

        // 首页的 revision 不是总览的 CAS token。
        let error = save_overview_layout(&state, None, default_overview_layout_dto())
            .await
            .unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");
    }
}
