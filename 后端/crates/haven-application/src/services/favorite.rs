//! FavoriteService：`favorite_set`（SLICE-FAVORITE-001 后端）。
//!
//! 规则（契约 §15.1 / SLICE-FAVORITE-001 Acceptance）：
//! - Work 收藏目标；重复设置同一状态幂等（Repository upsert 语义）。
//! - Work 不存在 → 稳定错误码 `WORK_NOT_FOUND`。
//! - "检查 Work 存在 + 写入收藏"在**同一事务**内执行（UnitOfWork，BE-APP-001 事务编排）。
//! - 失效通知（favorite.changed Event）由 Interface 层发布，Service 只返回结果。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::entities::FavoriteTarget;
use haven_domain::ids::WorkId;

use crate::services::ports::{FavoritePorts, FavoriteTxPorts, UnitOfWork};
use crate::wire::FavoriteSetResult;

#[derive(Clone)]
pub struct FavoriteService {
    ports: Arc<dyn FavoritePorts>,
    uow: Arc<dyn UnitOfWork>,
}

/// 收藏设置结果（含状态变更信号；`changed=false` 表示幂等重复设置，不发布 Event）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteSetOutcome {
    pub result: FavoriteSetResult,
    /// true = 状态实际变更（应发布 favorite.changed）；false = 幂等重复设置。
    pub changed: bool,
}

impl FavoriteService {
    pub fn new(ports: Arc<dyn FavoritePorts>, uow: Arc<dyn UnitOfWork>) -> Self {
        Self { ports, uow }
    }

    /// `favorite_set`（契约：返回 `FavoriteSetResult`）。
    pub async fn set(
        &self,
        work_id: WorkId,
        favorite: bool,
    ) -> Result<FavoriteSetResult, AppError> {
        Ok(self.set_with_outcome(work_id, favorite).await?.result)
    }

    /// `favorite_set` + 状态变更信号（Interface 层据此决定是否发布 `favorite.changed`）。
    ///
    /// revision 语义（R-FAV-001，状态版本）：
    /// - 状态变化（未收藏↔收藏）→ 生成新 revision 并持久化（work_favorite_versions）。
    /// - 重复设置相同状态 → 不更新版本，返回当前 revision（幂等收敛）。
    /// - 从未变更过状态（无版本历史）→ revision 为 `None`（wire 上为 `null`；
    ///   favorite 字段为权威状态；幂等路径不发 Event，因此 Event 的 revision 恒非空）。
    pub async fn set_with_outcome(
        &self,
        work_id: WorkId,
        favorite: bool,
    ) -> Result<FavoriteSetOutcome, AppError> {
        use std::sync::Mutex;
        let outcome = Arc::new(Mutex::new(None::<(Option<String>, bool)>));
        // 事务：work 存在性检查 + 状态读取 + 收藏写入原子完成。
        self.uow.run_favorite(&|tx| {
            if !tx.work_exists(work_id)? {
                return Err(work_not_found());
            }
            let current = tx.favorite_state(&FavoriteTarget::Work(work_id))?;
            // R-FAV-002：None（从未变更/无历史）解释为默认 inactive，
            // 因此"首次 set(false)"与当前状态相同 → 幂等（revision=None、changed=false、
            // 不写库、不发 Event）。
            let current_active = current.as_ref().map(|s| s.active).unwrap_or(false);
            if current_active == favorite {
                let revision = current.and_then(|s| s.revision);
                *outcome.lock().unwrap() = Some((revision, false));
                return Ok(());
            }
            let revision = new_revision();
            tx.apply_favorite(&FavoriteTarget::Work(work_id), favorite, &revision)?;
            *outcome.lock().unwrap() = Some((Some(revision), true));
            Ok(())
        })?;
        let (revision, changed) = outcome
            .lock()
            .unwrap()
            .take()
            .expect("事务闭包必然写入 outcome");
        Ok(FavoriteSetOutcome {
            result: FavoriteSetResult {
                work_id: work_id.to_string(),
                favorite,
                revision,
            },
            changed,
        })
    }

    pub async fn is_favorite(&self, work_id: WorkId) -> Result<bool, AppError> {
        self.ports.is_favorite(&FavoriteTarget::Work(work_id)).await
    }
}

/// 状态版本 token（opaque；唯一性由时间戳 + 纳秒后缀保证）。
fn new_revision() -> String {
    format!(
        "fav-{:016x}-{:x}",
        haven_common::UtcMillis::now().0,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    )
}

fn work_not_found() -> AppError {
    AppError::new(
        "WORK_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "作品不存在",
        false,
    )
}

/// 内存版 UnitOfWork（测试用）：直接调用闭包，无真实事务（单线程语义等价）。
pub struct MemUnitOfWork;

impl UnitOfWork for MemUnitOfWork {
    fn run_favorite(
        &self,
        f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        f(&MemFavoriteTx)
    }

    fn run_source_import(
        &self,
        _provider: &str,
        _external_id: &str,
        _work: &haven_domain::entities::Work,
        _edition: &haven_domain::entities::Edition,
        _items: &[haven_domain::entities::MediaItem],
        _resources: &[haven_domain::entities::Resource],
    ) -> Result<(), AppError> {
        Err(source_import_uow_unavailable())
    }
}

fn source_import_uow_unavailable() -> AppError {
    AppError::new(
        "INTERNAL_ERROR",
        haven_common::ErrorKind::Internal,
        "测试 UnitOfWork 不支持来源导入事务",
        false,
    )
}

struct MemFavoriteTx;

impl FavoriteTxPorts for MemFavoriteTx {
    fn work_exists(&self, _work_id: WorkId) -> Result<bool, AppError> {
        Ok(true)
    }
    fn favorite_state(
        &self,
        _target: &FavoriteTarget,
    ) -> Result<Option<crate::services::ports::FavoriteState>, AppError> {
        Ok(None)
    }
    fn apply_favorite(
        &self,
        _target: &FavoriteTarget,
        _on: bool,
        _revision: &str,
    ) -> Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::contracts::{FavoriteRepository, WorkRepository};
    use haven_domain::entities::{Favorite, Work};
    use haven_domain::enums::ContentCategory;
    use haven_domain::enums::{WorkStatus, WorkType};

    /// 内存端口：记录真实收藏状态（评审要求：测试必须验证状态变化，而非只验证"不报错"）。
    struct MemPorts {
        works: Vec<Work>,
        favorites: std::sync::Arc<std::sync::Mutex<Vec<WorkId>>>,
    }

    #[async_trait::async_trait]
    impl WorkRepository for MemPorts {
        async fn get(&self, id: WorkId) -> Result<Option<Work>, AppError> {
            Ok(self.works.iter().find(|w| w.id == id).cloned())
        }
        async fn save(&self, _w: &Work) -> Result<(), AppError> {
            Ok(())
        }
        async fn list(&self, _limit: u32, _offset: u32) -> Result<Vec<Work>, AppError> {
            Ok(self.works.clone())
        }
        async fn list_sorted(
            &self,
            _order: haven_domain::contracts::WorkOrder,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            Ok(self.works.clone())
        }
        async fn list_filtered(
            &self,
            _order: haven_domain::contracts::WorkOrder,
            _category: Option<ContentCategory>,
            _media_types: Option<&[haven_domain::enums::MediaType]>,
            _query: Option<&str>,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            Ok(self.works.clone())
        }
        async fn count_filtered(
            &self,
            _category: Option<ContentCategory>,
            _media_types: Option<&[haven_domain::enums::MediaType]>,
            _query: Option<&str>,
        ) -> Result<u64, AppError> {
            Ok(self.works.len() as u64)
        }
        async fn id_for_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
        ) -> Result<Option<WorkId>, AppError> {
            Ok(None)
        }
        async fn save_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
            _work_id: WorkId,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn has_any_source_ref(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn delete(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl FavoriteRepository for MemPorts {
        async fn set(&self, target: &FavoriteTarget) -> Result<(), AppError> {
            if let FavoriteTarget::Work(id) = target {
                let mut favorites = self.favorites.lock().unwrap();
                if !favorites.contains(id) {
                    favorites.push(*id);
                }
            }
            Ok(())
        }
        async fn unset(&self, target: &FavoriteTarget) -> Result<bool, AppError> {
            if let FavoriteTarget::Work(id) = target {
                let mut favorites = self.favorites.lock().unwrap();
                let before = favorites.len();
                favorites.retain(|f| f != id);
                Ok(favorites.len() < before)
            } else {
                Ok(false)
            }
        }
        async fn is_favorite(&self, target: &FavoriteTarget) -> Result<bool, AppError> {
            Ok(match target {
                FavoriteTarget::Work(id) => self.favorites.lock().unwrap().contains(id),
                _ => false,
            })
        }
        async fn list(&self, _limit: u32, _offset: u32) -> Result<Vec<Favorite>, AppError> {
            Ok(vec![])
        }
    }

    fn sample_work(id: WorkId) -> Work {
        Work {
            id,
            canonical_title: "三体".into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Fiction,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    /// 测试用 Tx：work 存在性来自 ports（通过闭包捕获不可行），
    /// 这里用"始终存在" + 事务外幂等由 MemPorts 验证。
    /// revision 语义（R-FAV-001）：active 状态存于 favorites Vec；版本存于版本 Map。
    struct TxWithExistence {
        exists: bool,
        favorites: std::sync::Arc<std::sync::Mutex<Vec<WorkId>>>,
        revisions: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<WorkId, String>>>,
    }
    impl FavoriteTxPorts for TxWithExistence {
        fn work_exists(&self, _work_id: WorkId) -> Result<bool, AppError> {
            Ok(self.exists)
        }
        fn favorite_state(
            &self,
            target: &FavoriteTarget,
        ) -> Result<Option<crate::services::ports::FavoriteState>, AppError> {
            if let FavoriteTarget::Work(id) = target {
                let active = self.favorites.lock().unwrap().contains(id);
                let revision = self.revisions.lock().unwrap().get(id).cloned();
                if !active && revision.is_none() {
                    return Ok(None);
                }
                return Ok(Some(crate::services::ports::FavoriteState {
                    active,
                    revision,
                }));
            }
            Ok(None)
        }
        fn apply_favorite(
            &self,
            target: &FavoriteTarget,
            on: bool,
            revision: &str,
        ) -> Result<(), AppError> {
            if let FavoriteTarget::Work(id) = target {
                let mut favorites = self.favorites.lock().unwrap();
                if on {
                    if !favorites.contains(id) {
                        favorites.push(*id);
                    }
                } else {
                    favorites.retain(|f| f != id);
                }
                self.revisions
                    .lock()
                    .unwrap()
                    .insert(*id, revision.to_owned());
            }
            Ok(())
        }
    }

    struct TxUow {
        exists: bool,
        favorites: std::sync::Arc<std::sync::Mutex<Vec<WorkId>>>,
        revisions: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<WorkId, String>>>,
    }
    impl UnitOfWork for TxUow {
        fn run_favorite(
            &self,
            f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            f(&TxWithExistence {
                exists: self.exists,
                favorites: self.favorites.clone(),
                revisions: self.revisions.clone(),
            })
        }

        fn run_source_import(
            &self,
            _provider: &str,
            _external_id: &str,
            _work: &haven_domain::entities::Work,
            _edition: &haven_domain::entities::Edition,
            _items: &[haven_domain::entities::MediaItem],
            _resources: &[haven_domain::entities::Resource],
        ) -> Result<(), AppError> {
            Err(source_import_uow_unavailable())
        }
    }

    #[tokio::test]
    async fn set_favorite_changes_state_idempotently() {
        let work_id = WorkId::new();
        let favorites = std::sync::Arc::new(std::sync::Mutex::new(Vec::<WorkId>::new()));
        let revisions = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            WorkId,
            String,
        >::new()));
        let ports = Arc::new(MemPorts {
            works: vec![sample_work(work_id)],
            favorites: favorites.clone(),
        });
        let service = FavoriteService::new(
            ports,
            Arc::new(TxUow {
                exists: true,
                favorites: favorites.clone(),
                revisions: revisions.clone(),
            }),
        );

        // 状态变化 → changed=true + 非空 revision
        let changed = service.set_with_outcome(work_id, true).await.unwrap();
        assert!(changed.changed, "首次收藏为状态变更");
        assert_eq!(changed.result.work_id, work_id.to_string());
        assert!(changed.result.favorite);
        assert!(changed.result.revision.is_some(), "变更必须带 revision");
        assert!(service.is_favorite(work_id).await.unwrap(), "收藏后应生效");
        let first_revision = changed.result.revision.clone();

        // 幂等重复 → changed=false + 相同 revision（状态版本语义，R-FAV-001）
        let repeated = service.set_with_outcome(work_id, true).await.unwrap();
        assert!(!repeated.changed, "重复设置相同状态不得视为变更");
        assert_eq!(
            repeated.result.revision, first_revision,
            "幂等返回当前 revision，不制造新版本"
        );
        assert_eq!(favorites.lock().unwrap().len(), 1, "重复收藏幂等");

        // 取消 → changed=true + 新 revision
        let off = service.set_with_outcome(work_id, false).await.unwrap();
        assert!(off.changed);
        assert!(!off.result.favorite);
        assert_ne!(off.result.revision, first_revision, "状态变化生成新版本");
        assert!(!service.is_favorite(work_id).await.unwrap(), "取消后应失效");

        // 重复取消 → changed=false + 相同 revision（版本行保留）
        let off_again = service.set_with_outcome(work_id, false).await.unwrap();
        assert!(!off_again.changed, "重复取消幂等");
        assert_eq!(
            off_again.result.revision, off.result.revision,
            "版本行保留供幂等返回"
        );
    }

    #[tokio::test]
    async fn first_false_is_idempotent_without_revision() {
        // R-FAV-002：从未收藏的 Work 首次 set(false) 必须是幂等——
        // revision=None、changed=false、不写库、不发 Event。
        let work_id = WorkId::new();
        let favorites = std::sync::Arc::new(std::sync::Mutex::new(Vec::<WorkId>::new()));
        let revisions = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            WorkId,
            String,
        >::new()));
        let ports = Arc::new(MemPorts {
            works: vec![sample_work(work_id)],
            favorites: favorites.clone(),
        });
        let service = FavoriteService::new(
            ports,
            Arc::new(TxUow {
                exists: true,
                favorites: favorites.clone(),
                revisions: revisions.clone(),
            }),
        );

        let outcome = service.set_with_outcome(work_id, false).await.unwrap();
        assert!(!outcome.changed, "首次 false 不得视为状态变化");
        assert!(
            outcome.result.revision.is_none(),
            "无版本历史 → revision=null"
        );
        assert!(!outcome.result.favorite);
        assert!(favorites.lock().unwrap().is_empty(), "不得写入 favorites");
        assert!(
            revisions.lock().unwrap().is_empty(),
            "不得写入版本行（不发 Event）"
        );

        // 重复首次 false 同样幂等且 revision 仍为 None
        let again = service.set_with_outcome(work_id, false).await.unwrap();
        assert!(!again.changed);
        assert!(again.result.revision.is_none());

        // 之后收藏 → 状态变化 → 新 revision
        let on = service.set_with_outcome(work_id, true).await.unwrap();
        assert!(on.changed);
        assert!(on.result.revision.is_some());
    }

    #[tokio::test]
    async fn missing_work_returns_stable_error() {
        let favorites = std::sync::Arc::new(std::sync::Mutex::new(Vec::<WorkId>::new()));
        let revisions = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            WorkId,
            String,
        >::new()));
        let ports = Arc::new(MemPorts {
            works: vec![],
            favorites: favorites.clone(),
        });
        let service = FavoriteService::new(
            ports,
            Arc::new(TxUow {
                exists: false,
                favorites,
                revisions,
            }),
        );
        let err = service.set(WorkId::new(), true).await.unwrap_err();
        assert_eq!(err.code().as_str(), "WORK_NOT_FOUND");
        assert!(!err.retryable());
    }
}
