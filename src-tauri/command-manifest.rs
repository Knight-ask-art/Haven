// 命令名单唯一事实源（R-MAIN-06/阻塞一复审修复）。
//
// 被三处 include（单一真源，防漂移）：
// - build.rs：AppManifest::commands（生成 allow-*/deny-* ACL）；
// - src/lib.rs（include!）：`target_handler_entries!()` 直接驱动 tauri::generate_handler
//   注册（**不再是手写第二份命令列表**）；
// - 发布前的构建检查：registered 集合直接从 TARGET_COMMAND_NAMES 派生
//   （**不再是手写第三份**），并校验真实生成 ACL 产物。
//
// 宏同时派生：
// - 命令名静态数组（build.rs 需要 &'static [&str]）；
// - 权限 id 数组；
// - `target_handler_entries!()`：展开成 `generate_handler` 所需的函数引用 token 流
//   （每个命令函数路径，如 `commands::library::library_list`）。
//   该宏只在 lib.rs 注册点展开；build.rs 只展开/使用其余派生项。

macro_rules! define_commands {
    ($(($name:literal, $perm:literal, $handler:path)),*) => {
        pub const TARGET_COMMANDS: &[(&str, &str)] = &[$(($name, $perm)),*];
        pub const TARGET_COMMAND_NAMES: &[&str] = &[$($name),*];
        pub const TARGET_PERMISSIONS: &[&str] = &[$($perm),*];
        /// 展开为完整的 `tauri::generate_handler![...]` 调用；handler 函数引用来自本宏
        /// 的同一份声明（单真源驱动注册，杜绝手写第二份命令列表）。
        #[allow(unused_macros)]
        macro_rules! registered_handlers {
            () => { tauri::generate_handler![ $($handler,)* ] };
        }
    };
}

define_commands!(
    (
        "cache_clear",
        "allow-cache-clear",
        commands::cache::cache_clear
    ),
    (
        "app_info_get",
        "allow-app-info-get",
        commands::app_info::app_info_get
    ),
    (
        "error_report_preview_get",
        "allow-error-report-preview-get",
        commands::error_report::error_report_preview_get
    ),
    (
        "error_report_confirm",
        "allow-error-report-confirm",
        commands::error_report::error_report_confirm
    ),
    (
        "error_report_export",
        "allow-error-report-export",
        commands::error_report::error_report_export
    ),
    (
        "error_report_open_issue",
        "allow-error-report-open-issue",
        commands::error_report::error_report_open_issue
    ),
    (
        "open_data_directory",
        "allow-open-data-directory",
        commands::app_info::open_data_directory
    ),
    (
        "open_logs_directory",
        "allow-open-logs-directory",
        commands::app_info::open_logs_directory
    ),
    (
        "open_cache_directory",
        "allow-open-cache-directory",
        commands::app_info::open_cache_directory
    ),
    ("library_list", "allow-library-list", commands::library::library_list),
    ("progress_save", "allow-progress-save", commands::progress::progress_save),
    ("favorite_set", "allow-favorite-set", commands::favorite::favorite_set),
    ("home_get", "allow-home-get", commands::home::home_get),
    (
        "storage_location_list",
        "allow-storage-location-list",
        commands::storage_location::storage_location_list
    ),
    (
        "storage_location_pick_local_directory",
        "allow-storage-location-pick-local-directory",
        commands::storage_location::storage_location_pick_local_directory
    ),
    (
        "storage_location_rebind_local_directory",
        "allow-storage-location-rebind-local-directory",
        commands::storage_location::storage_location_rebind_local_directory
    ),
    (
        "storage_location_disconnect",
        "allow-storage-location-disconnect",
        commands::storage_location::storage_location_disconnect
    ),
    (
        "storage_location_remove",
        "allow-storage-location-remove",
        commands::storage_location::storage_location_remove
    ),
    ("settings_get", "allow-settings-get", commands::settings::settings_get),
    (
        "settings_update",
        "allow-settings-update",
        commands::settings::settings_update
    ),
    (
        "settings_export",
        "allow-settings-export",
        commands::settings::settings_export
    ),
    (
        "settings_import",
        "allow-settings-import",
        commands::settings::settings_import
    ),
    (
        "preference_get",
        "allow-preference-get",
        commands::resource_preferences::preference_get
    ),
    (
        "preference_update",
        "allow-preference-update",
        commands::resource_preferences::preference_update
    ),
    (
        "library_scan_start",
        "allow-library-scan-start",
        commands::scan::library_scan_start
    ),
    ("scan_cancel", "allow-scan-cancel", commands::scan::scan_cancel),
    ("work_get", "allow-work-get", commands::work::work_get),
    (
        "resource_list_by_media_item",
        "allow-resource-list-by-media-item",
        commands::resource::resource_list_by_media_item
    ),
    (
        "edition_list_by_work",
        "allow-edition-list-by-work",
        commands::work::edition_list_by_work
    ),
    (
        "edition_get",
        "allow-edition-get",
        commands::work::edition_get
    ),
    (
        "session_open",
        "allow-session-open",
        commands::session::session_open
    ),
    (
        "session_close",
        "allow-session-close",
        commands::session::session_close
    ),
    (
        "comic_page_manifest_get",
        "allow-comic-page-manifest-get",
        commands::comic::comic_page_manifest_get
    ),
    (
        "reader_toc_get",
        "allow-reader-toc-get",
        commands::reader::reader_toc_get
    ),
    (
        "reader_search",
        "allow-reader-search",
        commands::reader::reader_search
    ),
    (
        "reader_search_start",
        "allow-reader-search-start",
        commands::reader::reader_search_start
    ),
    (
        "reader_search_cancel",
        "allow-reader-search-cancel",
        commands::reader::reader_search_cancel
    ),
    (
        "progress_recent",
        "allow-progress-recent",
        commands::progress::progress_recent
    ),
    (
        "progress_reset",
        "allow-progress-reset",
        commands::progress::progress_reset
    ),
    (
        "history_list",
        "allow-history-list",
        commands::history::history_list
    ),
    (
        "history_clear",
        "allow-history-clear",
        commands::history::history_clear
    ),
    (
        "search_history_list",
        "allow-search-history-list",
        commands::search_history::search_history_list
    ),
    (
        "search_history_record",
        "allow-search-history-record",
        commands::search_history::search_history_record
    ),
    (
        "search_history_remove",
        "allow-search-history-remove",
        commands::search_history::search_history_remove
    ),
    (
        "search_history_clear",
        "allow-search-history-clear",
        commands::search_history::search_history_clear
    ),
    (
        "marker_create",
        "allow-marker-create",
        commands::marker::marker_create
    ),
    (
        "marker_list",
        "allow-marker-list",
        commands::marker::marker_list
    ),
    (
        "marker_list_all",
        "allow-marker-list-all",
        commands::marker::marker_list_all
    ),
    (
        "marker_delete",
        "allow-marker-delete",
        commands::marker::marker_delete
    ),
    ("download_create", "allow-download-create", commands::download::download_create),
    ("download_list", "allow-download-list", commands::download::download_list),
    ("download_pause", "allow-download-pause", commands::download::download_pause),
    ("download_resume", "allow-download-resume", commands::download::download_resume),
    ("download_cancel", "allow-download-cancel", commands::download::download_cancel),
    ("download_retry", "allow-download-retry", commands::download::download_retry),
    (
        "download_remove_record",
        "allow-download-remove-record",
        commands::download::download_remove_record
    ),
    (
        "download_delete_offline",
        "allow-download-delete-offline",
        commands::download::download_delete_offline
    ),
    (
        "download_reveal_offline",
        "allow-download-reveal-offline",
        commands::download::download_reveal_offline
    ),
    (
        "download_subscribe",
        "allow-download-subscribe",
        commands::download::download_subscribe
    ),
    (
        "download_unsubscribe",
        "allow-download-unsubscribe",
        commands::download::download_unsubscribe
    ),
    // v0.2 契约冻结批次（契约 §36.2/§36.3/§36.5；CONTRACT-V02-*）。
    (
        "source_registry_list",
        "allow-source-registry-list",
        commands::source_registry::source_registry_list
    ),
    (
        "source_registry_set",
        "allow-source-registry-set",
        commands::source_registry::source_registry_set
    ),
    (
        "search_source_start",
        "allow-search-source-start",
        commands::search_source::search_source_start
    ),
    (
        "search_source_cancel",
        "allow-search-source-cancel",
        commands::search_source::search_source_cancel
    ),
    // v0.2 V2-F 批次（契约 §36.8；CONTRACT-V02-ENRICHMENT-001）。
    (
        "enrichment_status",
        "allow-enrichment-status",
        commands::enrichment::enrichment_status
    ),
    (
        "credential_status",
        "allow-credential-status",
        commands::credential::credential_status
    ),
    (
        "credential_set",
        "allow-credential-set",
        commands::credential::credential_set
    ),
    (
        "credential_delete",
        "allow-credential-delete",
        commands::credential::credential_delete
    ),
    // V2-B 实战批次（来源运行时；契约 §36.2/§36.3/§36.4 演进）。
    (
        "source_registry_set_endpoint",
        "allow-source-registry-set-endpoint",
        commands::source_runtime::source_registry_set_endpoint
    ),
    // V2-H 收尾批次（自定义 OPDS 书源；契约 §36.2 演进）。
    ("source_add", "allow-source-add", commands::source_custom::source_add),
    (
        "source_update",
        "allow-source-update",
        commands::source_custom::source_update
    ),
    (
        "source_remove",
        "allow-source-remove",
        commands::source_custom::source_remove
    ),
    (
        "source_set_credential",
        "allow-source-set-credential",
        commands::source_custom::source_set_credential
    ),
    (
        "source_work_import",
        "allow-source-work-import",
        commands::source_runtime::source_work_import
    ),
    (
        "stream_open",
        "allow-stream-open",
        commands::source_runtime::stream_open
    ),
    (
        "stream_close",
        "allow-stream-close",
        commands::source_runtime::stream_close
    ),
    (
        "trending_boards_get",
        "allow-trending-boards-get",
        commands::trending::trending_boards_get
    ),
    (
        "trending_boards_refresh",
        "allow-trending-boards-refresh",
        commands::trending::trending_boards_refresh
    ),
    ("cast_discover", "allow-cast-discover", commands::cast::cast_discover),
    ("cast_play", "allow-cast-play", commands::cast::cast_play),
    ("cast_status", "allow-cast-status", commands::cast::cast_status),
    ("cast_stop", "allow-cast-stop", commands::cast::cast_stop),
    (
        "video_screenshot_begin",
        "allow-video-screenshot-begin",
        commands::video_screenshot::video_screenshot_begin
    ),
    (
        "video_screenshot_chunk",
        "allow-video-screenshot-chunk",
        commands::video_screenshot::video_screenshot_chunk
    ),
    (
        "video_screenshot_commit",
        "allow-video-screenshot-commit",
        commands::video_screenshot::video_screenshot_commit
    ),
    (
        "video_screenshot_cancel",
        "allow-video-screenshot-cancel",
        commands::video_screenshot::video_screenshot_cancel
    )
);
