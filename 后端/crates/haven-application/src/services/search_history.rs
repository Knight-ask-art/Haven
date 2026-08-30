//! SearchHistoryService（V02-SETTINGS-PRIVACY-DATA-007）。
//!
//! 负责搜索词的规范化、数量上限和 Wire 映射；SQLite 只由 Repository 负责。
//! 搜索历史不与播放/阅读历史共享命令或清理范围。

use std::sync::Arc;

use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{SearchHistoryEntry, SearchHistoryRepository};

use crate::mapper::time::utc_millis_to_rfc3339;
use crate::wire::SearchHistoryEntryDto;

pub const SEARCH_HISTORY_LIMIT: u32 = 10;
pub const SEARCH_HISTORY_MAX_TERM_CHARS: usize = 200;

#[derive(Clone)]
pub struct SearchHistoryService {
    repository: Arc<dyn SearchHistoryRepository>,
}

impl SearchHistoryService {
    pub fn new(repository: Arc<dyn SearchHistoryRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(&self, limit: Option<u32>) -> Result<Vec<SearchHistoryEntryDto>, AppError> {
        let limit = limit
            .unwrap_or(SEARCH_HISTORY_LIMIT)
            .min(SEARCH_HISTORY_LIMIT);
        Ok(self
            .repository
            .list(limit)
            .await?
            .iter()
            .map(to_dto)
            .collect())
    }

    pub async fn record(&self, term: &str) -> Result<SearchHistoryEntryDto, AppError> {
        let term = normalize_term(term)?;
        let now = UtcMillis::now();
        self.repository.record(&term, now).await?;
        Ok(SearchHistoryEntryDto {
            term,
            last_used_at: utc_millis_to_rfc3339(now),
        })
    }

    pub async fn remove(&self, term: &str) -> Result<bool, AppError> {
        let term = normalize_term(term)?;
        self.repository.delete(&term).await
    }

    pub async fn clear(&self) -> Result<u64, AppError> {
        self.repository.clear_all().await
    }
}

fn normalize_term(raw: &str) -> Result<String, AppError> {
    let term = raw.trim();
    if term.is_empty() {
        return Err(invalid_term());
    }
    if term.chars().count() > SEARCH_HISTORY_MAX_TERM_CHARS {
        return Err(invalid_term());
    }
    Ok(term.to_owned())
}

fn invalid_term() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        haven_common::ErrorKind::Validation,
        "搜索词不能为空且不能超过 200 个字符",
        false,
    )
}

fn to_dto(entry: &SearchHistoryEntry) -> SearchHistoryEntryDto {
    SearchHistoryEntryDto {
        term: entry.term.clone(),
        last_used_at: utc_millis_to_rfc3339(entry.last_used_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MemoryRepo(Mutex<Vec<SearchHistoryEntry>>);

    #[async_trait::async_trait]
    impl SearchHistoryRepository for MemoryRepo {
        async fn list(&self, limit: u32) -> Result<Vec<SearchHistoryEntry>, AppError> {
            let mut values = self.0.lock().unwrap().clone();
            values.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at));
            values.truncate(limit as usize);
            Ok(values)
        }
        async fn record(&self, term: &str, at: UtcMillis) -> Result<(), AppError> {
            let mut values = self.0.lock().unwrap();
            values.retain(|entry| entry.term != term);
            values.push(SearchHistoryEntry {
                term: term.to_owned(),
                last_used_at: at,
            });
            Ok(())
        }
        async fn delete(&self, term: &str) -> Result<bool, AppError> {
            let mut values = self.0.lock().unwrap();
            let before = values.len();
            values.retain(|entry| entry.term != term);
            Ok(before != values.len())
        }
        async fn clear_all(&self) -> Result<u64, AppError> {
            let mut values = self.0.lock().unwrap();
            let count = values.len() as u64;
            values.clear();
            Ok(count)
        }
    }

    #[tokio::test]
    async fn record_normalizes_and_rejects_invalid_terms() {
        let service = SearchHistoryService::new(Arc::new(MemoryRepo(Mutex::new(vec![]))));
        assert_eq!(service.record("  栖阅  ").await.unwrap().term, "栖阅");
        assert_eq!(
            service.record("   ").await.unwrap_err().code().as_str(),
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            service
                .record(&"x".repeat(201))
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "INVALID_ARGUMENT"
        );
    }
}
