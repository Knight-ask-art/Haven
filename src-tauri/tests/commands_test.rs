//! Command 核心逻辑测试（IPC-TAURI-001A 验收）。
//!
//! 正常列表 / 空列表 / Favorite 状态变化 / 重复幂等 / WORK_NOT_FOUND / INVALID_ID /
//! 事件只在实际变化时产生。
//! 测试直接调用纯函数（run_library_list / run_favorite_set），不依赖 Tauri runtime
//! （本机 0xc0000139 为 tauri mock harness 环境问题，核心逻辑覆盖见 application crate）。

use std::sync::Arc;

use haven_application::wire::{
    FavoriteSetRequest, LibraryListRequest, LibraryListSort, QueryCategory,
};
use haven_domain::entities::{Edition, MediaIndex, MediaItem, Work};
use haven_domain::enums::{MediaItemStatus, MediaType, WorkStatus, WorkType};
use haven_domain::ids::{EditionId, MediaItemId, WorkId};
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::Db;
use haven_tauri_lib::commands::{favorite::run_favorite_set, library::run_library_list};
use haven_tauri_lib::state::AppState;

fn app_state() -> AppState {
    AppState::new(Arc::new(Db::open_in_memory().unwrap()))
}

fn list_request() -> LibraryListRequest {
    LibraryListRequest {
        category: QueryCategory::All,
        media_types: None,
        query: None,
        sort: LibraryListSort::RecentlyAdded,
        cursor: None,
        limit: 50,
    }
}

/// 通过共享 repos 种子一条 work → edition → media_item 链。
async fn seed_chain(repos: &SqliteRepositories) -> WorkId {
    use haven_domain::contracts::{EditionRepository, MediaItemRepository, WorkRepository};
    let work_id = WorkId::new();
    let edition_id = EditionId::new();
    let media_item_id = MediaItemId::new();
    let now = haven_common::UtcMillis::now();
    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "三体".into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    repos
        .edition
        .save(&Edition {
            id: edition_id,
            work_id,
            title: "三体".into(),
            subtitle: None,
            edition_type: MediaType::Book,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    repos
        .media_item
        .save(&MediaItem {
            id: media_item_id,
            edition_id,
            parent_id: None,
            media_type: MediaType::Book,
            title: "三体".into(),
            index: MediaIndex::Chapter {
                volume: None,
                chapter: 1.0,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    work_id
}

#[tokio::test]
async fn library_list_returns_seeded_work_card() {
    let state = app_state();
    seed_chain(&state.repos).await;

    let page = run_library_list(&state, list_request())
        .await
        .expect("正常列表");
    assert_eq!(page.items.len(), 1, "应返回 1 张 WorkCard");
    assert_eq!(page.items[0].title, "三体");
    assert_eq!(page.total, Some(1));
}

#[tokio::test]
async fn library_list_empty_database_returns_empty_page() {
    let state = app_state();
    let page = run_library_list(&state, list_request())
        .await
        .expect("空列表");
    assert!(page.items.is_empty());
    assert_eq!(page.total, Some(0));
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn favorite_set_changes_state_and_emits_event_once() {
    let state = app_state();
    let work_id = seed_chain(&state.repos).await;

    // 首次收藏：changed=true → event 存在 + revision 非空
    let first = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: work_id.to_string(),
            favorite: true,
        },
    )
    .await
    .expect("收藏成功");
    assert!(first.result.favorite);
    assert!(first.result.revision.is_some(), "状态变化必须带 revision");
    let event = first.event.expect("状态变化必须产生事件");
    assert_eq!(event.work_id, work_id.to_string());
    assert!(event.favorite);
    assert!(!event.revision.is_empty(), "事件 revision 恒非空");
    assert_eq!(
        event.revision,
        first.result.revision.as_deref().unwrap(),
        "事件与 Mutation Result 同一状态版本"
    );

    // 幂等重复设置：无事件、revision 相同
    let repeated = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: work_id.to_string(),
            favorite: true,
        },
    )
    .await
    .expect("重复收藏成功");
    assert!(
        repeated.event.is_none(),
        "幂等重复设置不得产生第二个 favorite.changed"
    );
    assert_eq!(
        repeated.result.revision, first.result.revision,
        "幂等返回当前 revision"
    );

    // 取消：changed=true → 新 revision + 事件
    let off = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: work_id.to_string(),
            favorite: false,
        },
    )
    .await
    .expect("取消收藏成功");
    assert!(!off.result.favorite);
    assert!(off.event.is_some());
    assert_ne!(
        off.result.revision, first.result.revision,
        "状态变化生成新 revision"
    );

    // 重复取消：无事件 + 与上次相同 revision（版本保留）
    let off_again = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: work_id.to_string(),
            favorite: false,
        },
    )
    .await
    .expect("重复取消幂等");
    assert!(off_again.event.is_none(), "幂等路径不发 Event");
    assert_eq!(off_again.result.revision, off.result.revision, "版本保留");

    // 首次 false（从未收藏的新 Work）：无事件 + revision=None（R-FAV-002）
    let never_touched = seed_chain(&state.repos).await;
    let first_false = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: never_touched.to_string(),
            favorite: false,
        },
    )
    .await
    .expect("首次 false 幂等");
    assert!(!first_false.result.favorite);
    assert!(
        first_false.result.revision.is_none(),
        "无版本历史 → revision=null"
    );
    assert!(first_false.event.is_none(), "幂等路径不发 Event");
}

#[tokio::test]
async fn favorite_set_unknown_work_returns_work_not_found() {
    let state = app_state();
    let err = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: WorkId::new().to_string(),
            favorite: true,
        },
    )
    .await
    .expect_err("未知 Work 必须报错");
    assert_eq!(err.code, "WORK_NOT_FOUND");
    assert!(!err.retryable);
}

#[tokio::test]
async fn favorite_set_malformed_id_returns_invalid_id() {
    let state = app_state();
    let err = run_favorite_set(
        &state,
        FavoriteSetRequest {
            work_id: "not-a-uuid".into(),
            favorite: true,
        },
    )
    .await
    .expect_err("非法 ID 必须报错");
    assert_eq!(err.code, "INVALID_ID");
    assert!(!err.retryable);
}

#[tokio::test]
async fn library_list_works_card_shape_is_projection_not_row() {
    let state = app_state();
    seed_chain(&state.repos).await;
    let page = run_library_list(&state, list_request()).await.unwrap();
    let json = serde_json::to_string(&page.items[0]).unwrap();
    assert!(!json.contains("canonicalTitle"), "不得暴露 DB 字段名");
    assert!(json.contains("\"workId\""), "投影使用 wire 字段名");
}
