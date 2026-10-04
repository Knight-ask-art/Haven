//! 浏览句柄租约注册表的 TTL / 类型 / 容量 / 一次性光标回归。
//!
//! 全部使用显式 `UtcMillis` 与内存注册表：不触网、不睡眠、不依赖系统时钟推进。
//! 真实 Drive ID 只作为租约字段出现，注册表本身不校验它们。

use super::*;

fn folder_lease() -> FolderLease {
    FolderLease {
        account_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        folder_id: "Fld-9".to_owned(),
        parent_id: None,
        display_name: "文献".to_owned(),
        registered: None,
    }
}

fn file_lease() -> FileLease {
    FileLease {
        account_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        folder_id: "Fld-9".to_owned(),
        file_id: "Pdf-7".to_owned(),
        registered: None,
    }
}

fn cursor_lease() -> CursorLease {
    CursorLease {
        account_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        folder_id: "Fld-9".to_owned(),
        folder_handle: uuid::Uuid::new_v4().to_string(),
        registered: None,
        page_token: "~!!~opaque-page-token".to_owned(),
        owned_handles: Vec::new(),
    }
}

#[test]
fn leases_expire_exactly_at_their_explicit_deadline() {
    let registry = BrowseHandleRegistry::new();
    let handle = registry
        .insert_folder(folder_lease(), UtcMillis(10_000))
        .unwrap();
    let deadline = 10_000 + CLOUD_BROWSE_HANDLE_TTL_MS;
    assert!(registry.folder(&handle, UtcMillis(deadline - 1)).is_ok());
    assert_eq!(
        registry
            .folder(&handle, UtcMillis(deadline))
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_EXPIRED"
    );
    // TTL 不续期：命中过的句柄仍按最初截止时刻过期。
    assert_eq!(
        registry
            .folder(&handle, UtcMillis(deadline + 1))
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_EXPIRED"
    );
}

#[test]
fn handles_are_typed_and_unknown_or_malformed_handles_fail_closed() {
    let registry = BrowseHandleRegistry::new();
    let now = UtcMillis(0);
    let folder = registry.insert_folder(folder_lease(), now).unwrap();
    let file = registry.insert_file(file_lease(), now).unwrap();
    let cursor = registry.insert_cursor(cursor_lease(), now).unwrap();

    assert!(registry.folder(&folder, now).is_ok());
    assert!(registry.file(&file, now).is_ok());
    // 目录 / 文件句柄可重复使用。
    assert!(registry.folder(&folder, now).is_ok());

    // 类型不符：文件句柄不是目录句柄，光标句柄也不是目录句柄。
    assert_eq!(
        registry.file(&folder, now).unwrap_err().code().as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    assert_eq!(
        registry.folder(&cursor, now).unwrap_err().code().as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    // 非 UUID（含真实 Provider ID 字面量）与空串一律失败关闭。
    for opaque in ["not-a-handle", "Pdf-7", ""] {
        assert_eq!(
            registry.folder(opaque, now).unwrap_err().code().as_str(),
            "CLOUD_BROWSE_HANDLE_INVALID"
        );
    }
    let unknown = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        registry.file(&unknown, now).unwrap_err().code().as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    assert_eq!(
        registry
            .take_cursor(&unknown, now)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    // 放在最后：`take_cursor` 的类型校验发生在取出之后，会消费传入句柄。
    assert_eq!(
        registry
            .take_cursor(&folder, now)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
}

#[test]
fn cursors_are_single_use_and_stop_existing_once_taken() {
    let registry = BrowseHandleRegistry::new();
    let handle = registry
        .insert_cursor(cursor_lease(), UtcMillis(1_000))
        .unwrap();
    let taken = registry.take_cursor(&handle, UtcMillis(1_001)).unwrap();
    assert_eq!(taken.page_token, "~!!~opaque-page-token");
    assert_eq!(taken.folder_id, "Fld-9");
    // 同一句柄第二次消费必须失败：原始分页 token 不会第二次交出。
    assert_eq!(
        registry
            .take_cursor(&handle, UtcMillis(1_002))
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );

    let expiring = registry
        .insert_cursor(cursor_lease(), UtcMillis(2_000))
        .unwrap();
    let deadline = 2_000 + CLOUD_BROWSE_HANDLE_TTL_MS;
    assert_eq!(
        registry
            .take_cursor(&expiring, UtcMillis(deadline))
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_EXPIRED"
    );
    // 过期取出即销毁：再消费是彻底的无效句柄，而不是「又过期一次」。
    assert_eq!(
        registry
            .take_cursor(&expiring, UtcMillis(deadline + 1))
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
}

#[test]
fn capacity_is_enforced_after_pruning_expired_leases() {
    let registry = BrowseHandleRegistry::new();
    let now = UtcMillis(0);
    let mut handles = Vec::with_capacity(CLOUD_BROWSE_MAX_LEASES);
    for _ in 0..CLOUD_BROWSE_MAX_LEASES {
        handles.push(registry.insert_file(file_lease(), now).unwrap());
    }
    assert_eq!(
        registry
            .insert_file(file_lease(), now)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_BUSY"
    );
    // 满员不淘汰活动租约：最早与最新的句柄都仍然可用。
    assert!(registry.file(&handles[0], now).is_ok());
    assert!(registry.file(handles.last().unwrap(), now).is_ok());

    // 先清理过期再计数：TTL 之后立刻有空间。
    let later = UtcMillis(CLOUD_BROWSE_HANDLE_TTL_MS + 1);
    assert!(registry.insert_file(file_lease(), later).is_ok());
    assert_eq!(
        registry
            .file(&handles[0], later)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
}

#[test]
fn consuming_a_cursor_retires_only_the_preceding_page_entry_handles() {
    let registry = BrowseHandleRegistry::new();
    let now = UtcMillis(0);
    let parent = registry.insert_folder(folder_lease(), now).unwrap();
    let file = registry.insert_file(file_lease(), now).unwrap();
    let child = registry.insert_folder(folder_lease(), now).unwrap();
    let cursor = registry
        .insert_cursor(
            CursorLease {
                folder_handle: parent.clone(),
                owned_handles: vec![file.clone(), child.clone()],
                ..cursor_lease()
            },
            now,
        )
        .unwrap();

    let lease = registry.take_cursor(&cursor, now).unwrap();
    assert_eq!(lease.page_token, "~!!~opaque-page-token");
    assert_eq!(lease.folder_handle, parent);
    assert_eq!(lease.owned_handles, vec![file.clone(), child.clone()]);

    // 上一页条目句柄被退休：再选择一律失败关闭。
    assert_eq!(
        registry.file(&file, now).unwrap_err().code().as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    assert_eq!(
        registry.folder(&child, now).unwrap_err().code().as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
    // 父目录句柄不在退休列表内，必须保留。
    assert!(registry.folder(&parent, now).is_ok());
    // 光标本身仍是一次性：重复消费失败关闭。
    assert_eq!(
        registry
            .take_cursor(&cursor, now)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_BROWSE_HANDLE_INVALID"
    );
}

#[test]
fn many_pages_stay_bounded_and_keep_the_live_page_selections() {
    let registry = BrowseHandleRegistry::new();
    let now = UtcMillis(0);
    let parent = registry.insert_folder(folder_lease(), now).unwrap();
    // 每页 100 个条目句柄；若不退休上一页，40 页会远超 2048 上限并被 CLOUD_BROWSE_BUSY 拒绝。
    let mut pending: Option<(String, Vec<String>)> = None;
    for _ in 0..40 {
        let mut minted = Vec::with_capacity(100);
        for _ in 0..100 {
            minted.push(registry.insert_file(file_lease(), now).unwrap());
        }
        let cursor = registry
            .insert_cursor(
                CursorLease {
                    folder_handle: parent.clone(),
                    owned_handles: minted.clone(),
                    ..cursor_lease()
                },
                now,
            )
            .unwrap();
        if let Some((previous_cursor, previous_handles)) = pending.take() {
            let lease = registry.take_cursor(&previous_cursor, now).unwrap();
            assert_eq!(lease.owned_handles, previous_handles);
            for handle in &previous_handles {
                assert_eq!(
                    registry.file(handle, now).unwrap_err().code().as_str(),
                    "CLOUD_BROWSE_HANDLE_INVALID",
                    "上一页条目句柄必须已失效"
                );
            }
        }
        pending = Some((cursor, minted));
    }
    let (_, current_handles) = pending.expect("保留最新一页光标");
    for handle in &current_handles {
        assert!(registry.file(handle, now).is_ok(), "当前页选择必须仍然可用");
    }
    assert!(registry.folder(&parent, now).is_ok(), "父目录句柄必须保留");
}
