//! OAuth 尝试登记表 / 提交门闸的纯内存状态机回归。
//!
//! 全部使用显式到期时刻与内存表：不触网、不睡眠、不依赖系统时钟推进；只断言既有
//! `pub(crate)` 接口的对外行为（终态一次写入、领取恰好一次、取消与提交互斥）。

use std::time::Duration;

use super::*;

/// 远未到期的登记：让条目停在 live 状态，不需要任何计时。
fn future_expiry() -> UtcMillis {
    UtcMillis(UtcMillis::now().0 + 600_000)
}

fn begin_fresh(registry: &OAuthAttemptRegistry) -> String {
    let attempt_id = uuid::Uuid::new_v4().to_string();
    registry
        .begin(&attempt_id, future_expiry(), OAuthAttemptAnchor::Absent)
        .expect("登记尝试");
    attempt_id
}

#[tokio::test]
async fn dropping_an_armed_completion_lease_fails_the_attempt_and_releases_the_gate() {
    let registry = OAuthAttemptRegistry::new();
    let attempt_id = begin_fresh(&registry);
    registry.claim(&attempt_id).expect("领取完成");
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Completing
    );

    let gate = registry
        .lock_commit_gate(&attempt_id)
        .await
        .expect("取得提交门闸");
    {
        let mut lease = registry.completion_lease(&attempt_id);
        lease.hold_gate(gate);
        // 不 release_gate 直接 drop：租约必须显式收口为 Failed。
    }
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Failed
    );

    // 门闸随租约一起释放：再次取得必须立即可得（限时守卫只防挂死，不是睡眠）。
    let regate = tokio::time::timeout(
        Duration::from_secs(5),
        registry.lock_commit_gate(&attempt_id),
    )
    .await
    .expect("门闸必须已随租约释放")
    .expect("重新取得提交门闸");
    drop(regate);
}

#[tokio::test]
async fn a_disarmed_lease_returns_the_claim_to_pending_for_a_retry() {
    let registry = OAuthAttemptRegistry::new();
    let attempt_id = begin_fresh(&registry);
    registry.claim(&attempt_id).unwrap();
    {
        let mut lease = registry.completion_lease(&attempt_id);
        lease.disarm();
        registry.release_claim(&attempt_id);
    }
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Pending
    );

    // 退回 Pending 后可重新领取（幂等重试），而不是卡在 Completing。
    registry.claim(&attempt_id).unwrap();
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Completing
    );
}

#[tokio::test]
async fn cancel_before_commit_wins_and_a_late_poll_cannot_rewrite_it() {
    let registry = OAuthAttemptRegistry::new();
    let attempt_id = begin_fresh(&registry);
    registry.claim(&attempt_id).unwrap();

    assert_eq!(
        registry.cancel(&attempt_id).await.unwrap(),
        OAuthAttemptState::Cancelled
    );
    // 取消之后提交门闸拒绝提交，且不改写终态。
    assert_eq!(
        registry
            .ensure_committable(&attempt_id)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_OAUTH_CANCELLED"
    );
    // 迟到的 Provider 轮询只能看到终态。
    assert_eq!(
        registry
            .observe_poll(&attempt_id, OAuthStatus::Authorized)
            .unwrap(),
        (OAuthAttemptState::Cancelled, false)
    );
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Cancelled
    );
    // 已取消的尝试不能被再次领取。
    assert_eq!(
        registry.claim(&attempt_id).unwrap_err().code().as_str(),
        "CLOUD_OAUTH_CANCELLED"
    );
}

#[tokio::test]
async fn a_committed_attempt_is_terminal_for_cancel_failure_and_late_polls() {
    let registry = OAuthAttemptRegistry::new();
    let attempt_id = begin_fresh(&registry);
    registry.claim(&attempt_id).unwrap();
    registry.ensure_committable(&attempt_id).unwrap();
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Committing
    );
    registry.mark_terminal(&attempt_id, OAuthAttemptState::Completed);

    // 取消如实报告已完成，不把提交改写掉。
    assert_eq!(
        registry.cancel(&attempt_id).await.unwrap(),
        OAuthAttemptState::Completed
    );
    assert_eq!(
        registry
            .observe_poll(&attempt_id, OAuthStatus::Failed)
            .unwrap(),
        (OAuthAttemptState::Completed, false)
    );
    // 终态只写一次：迟到的失败不能覆盖 Completed。
    registry.mark_terminal(&attempt_id, OAuthAttemptState::Failed);
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Completed
    );
}

#[tokio::test]
async fn an_already_due_attempt_is_expired_and_never_claimable() {
    let registry = OAuthAttemptRegistry::new();
    let attempt_id = uuid::Uuid::new_v4().to_string();
    // 显式到期时刻（0）：到点即终态，无需任何睡眠。
    registry
        .begin(&attempt_id, UtcMillis(0), OAuthAttemptAnchor::Absent)
        .unwrap();
    assert_eq!(
        registry.state(&attempt_id).unwrap(),
        OAuthAttemptState::Expired
    );
    assert_eq!(
        registry.claim(&attempt_id).unwrap_err().code().as_str(),
        "CLOUD_OAUTH_ATTEMPT_EXPIRED"
    );
    assert_eq!(
        registry
            .ensure_live(&attempt_id)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_OAUTH_ATTEMPT_EXPIRED"
    );
    // 取消如实报告 Expired，不误报 Cancelled。
    assert_eq!(
        registry.cancel(&attempt_id).await.unwrap(),
        OAuthAttemptState::Expired
    );
    assert_eq!(
        registry
            .observe_poll(&attempt_id, OAuthStatus::Pending)
            .unwrap(),
        (OAuthAttemptState::Expired, false)
    );
}

#[tokio::test]
async fn live_capacity_conflicts_and_malformed_ids_are_rejected_in_memory() {
    let registry = OAuthAttemptRegistry::new();
    let mut ids = Vec::new();
    for _ in 0..MAX_TRACKED_OAUTH_ATTEMPTS {
        ids.push(begin_fresh(&registry));
    }
    assert_eq!(
        registry
            .begin(
                &uuid::Uuid::new_v4().to_string(),
                future_expiry(),
                OAuthAttemptAnchor::Absent
            )
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_OAUTH_ATTEMPT_LIMIT"
    );
    assert_eq!(
        registry
            .begin(&ids[0], future_expiry(), OAuthAttemptAnchor::Absent)
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_OAUTH_ATTEMPT_CONFLICT"
    );
    let upper = ids[1].to_uppercase();
    for malformed in ["", "not-a-uuid", upper.as_str()] {
        assert_eq!(
            registry
                .begin(malformed, future_expiry(), OAuthAttemptAnchor::Absent)
                .unwrap_err()
                .code()
                .as_str(),
            "CLOUD_OAUTH_ATTEMPT_ID_INVALID"
        );
        assert_eq!(
            registry.state(malformed).unwrap_err().code().as_str(),
            "CLOUD_OAUTH_ATTEMPT_ID_INVALID"
        );
    }
}

#[tokio::test]
async fn terminal_attempts_are_retained_in_a_bounded_fifo() {
    let registry = OAuthAttemptRegistry::new();
    let mut ids = Vec::new();
    for _ in 0..(MAX_TRACKED_OAUTH_ATTEMPTS + 12) {
        let attempt_id = uuid::Uuid::new_v4().to_string();
        // 到点即终态：不占 live 名额，也不需要睡眠。
        registry
            .begin(&attempt_id, UtcMillis(0), OAuthAttemptAnchor::Absent)
            .unwrap();
        ids.push(attempt_id);
    }
    // 最早的终态按登记序回收……
    assert_eq!(
        registry.state(&ids[0]).unwrap_err().code().as_str(),
        "CLOUD_OAUTH_ATTEMPT_NOT_FOUND"
    );
    // ……最新登记的仍在表里（且已是 Expired 终态）。
    assert_eq!(
        registry.state(ids.last().unwrap()).unwrap(),
        OAuthAttemptState::Expired
    );
}
