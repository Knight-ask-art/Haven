//! SettingsService：Settings 持久化与 Revision 并发控制（BE-SETTINGS-001 + R-MAIN-01 复审修复）。
//!
//! - Section 闭合枚举 + Typed DTO（未知字段/非法枚举在反序列化边界拒绝）。
//! - revision 为**状态版本**：实际变化生成新 revision（持久化）；相同值重复更新
//!   幂等返回当前 revision（不制造新版本、不发 Event）。
//! - **全部更新语义（读 authoritative current → expected 校验 → 应用 patch →
//!   语义值比较 → 条件写 → 返回 authoritative revision）在单一 SettingsUoW 事务内**：
//!   `expected_revision` 不匹配 → 稳定 `REVISION_CONFLICT`，**不静默覆盖、不短路**；
//!   从未保存的 Section 携带非空 expected → 冲突；已有行携带 expected=None → 冲突。
//! - 从未保存的 Section 返回默认值 + `revision: None`。
//! - Secret 禁止进入 settings.data_json（凭据走 CredentialStore）。

use std::sync::{Arc, Mutex};

use haven_common::AppError;
use haven_domain::contracts::SettingsRow;
use haven_domain::settings::{SettingsPatch, SettingsSection, SettingsValue};

/// 读取快照：当前值 + 状态版本（从未保存 → 默认值 + None）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingsSnapshot {
    pub value: SettingsValue,
    pub revision: Option<String>,
}

/// 更新结果（`changed=false` 表示幂等重复更新，不发布 settings.changed）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingsUpdateResult {
    pub value: SettingsValue,
    pub revision: Option<String>,
    pub changed: bool,
}

/// 事务内可用的 settings 操作（原子 CAS 的读写原语）。
pub trait SettingsTxPorts {
    fn load(&self, section: &str) -> Result<Option<SettingsRow>, AppError>;
    /// **数据库层条件写**（R-MAIN-07）：`expected_revision` 作为 SQL 条件
    /// （`WHERE revision = expected` / 首次 `INSERT` 无冲突直接成功）；
    /// 受影响行数 == 0 → 返回 `false`（并发竞争者已先行提交，调用方映射 REVISION_CONFLICT）。
    /// 绝不无条件覆盖。
    fn cas_write(
        &self,
        section: &str,
        expected_revision: Option<&str>,
        row: &SettingsRow,
    ) -> Result<bool, AppError>;
}

/// Settings Unit of Work：闭包在**单一事务**内执行（读→校验→比较→写原子）；
/// 失败自动回滚。
/// - `run`：**写路径，BEGIN IMMEDIATE**（进入即取 RESERVED 写锁；busy_timeout 内排队，
///   并发竞争者开启事务时看到最新已提交状态 → 稳定 REVISION_CONFLICT，不产生 BUSY_SNAPSHOT）。
/// - `run_read`：**读路径，BEGIN DEFERRED 只读**（WAL 下不阻塞写者、不取写锁）。
pub trait SettingsUoW: Send + Sync {
    fn run(&self, f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>)
    -> Result<(), AppError>;
    fn run_read(
        &self,
        f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SettingsService {
    uow: Arc<dyn SettingsUoW>,
}

impl SettingsService {
    pub fn new(uow: Arc<dyn SettingsUoW>) -> Self {
        Self { uow }
    }

    /// 读取指定 Section（默认值 + 状态版本）。读路径 BEGIN DEFERRED（不取写锁）。
    pub async fn get(&self, section: SettingsSection) -> Result<SettingsSnapshot, AppError> {
        let cell = Arc::new(Mutex::new(None::<Result<SettingsSnapshot, AppError>>));
        self.uow.run_read(&|tx| {
            let snapshot = match tx.load(section.as_str())? {
                Some(row) => SettingsSnapshot {
                    value: deserialize_value(section, &row.data_json)?,
                    revision: Some(row.revision),
                },
                None => SettingsSnapshot {
                    value: SettingsValue::default_for(section),
                    revision: None,
                },
            };
            *cell.lock().unwrap() = Some(Ok(snapshot));
            Ok(())
        })?;
        cell.lock().unwrap().take().expect("闭包必然写入结果")
    }

    /// 部分更新（R-MAIN-01：原子 CAS 语义全部在 SettingsUoW 事务内）。
    /// - `expected_revision` 校验**先于**一切（包括幂等短路）：
    ///   过期 revision 即使提交相同值也返回 `REVISION_CONFLICT`；
    ///   从未保存 + 非空 expected / 已有行 + expected=None → `REVISION_CONFLICT`。
    /// - 校验通过 + 相同值 → 幂等（`changed=false`，不写库不发 Event），
    ///   revision 为事务内读到的 authoritative 当前版本。
    /// - 校验通过 + 实际变化 → 新 revision 持久化，`changed=true`。
    pub async fn update(
        &self,
        section: SettingsSection,
        expected_revision: Option<&str>,
        patch: SettingsPatch,
    ) -> Result<SettingsUpdateResult, AppError> {
        if patch.section() != section {
            return Err(validation("patch 与 section 不一致"));
        }

        let expected = expected_revision.map(|s| s.to_owned());
        let cell = Arc::new(Mutex::new(None::<Result<SettingsUpdateResult, AppError>>));
        self.uow.run(&|tx| {
            // 事务内读取 authoritative current（读到的即提交时状态，无 TOCTOU）。
            let row = tx.load(section.as_str())?;
            let (current_value, current_revision) = match &row {
                Some(row) => (
                    deserialize_value(section, &row.data_json)?,
                    Some(row.revision.clone()),
                ),
                None => (SettingsValue::default_for(section), None),
            };

            // expected 校验（事务边界内，绝不提前返回）。
            let revision_matches = match (&current_revision, expected.as_deref()) {
                (Some(cur), Some(exp)) => cur == exp,
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            };
            if !revision_matches {
                *cell.lock().unwrap() = Some(Err(conflict()));
                return Ok(());
            }

            let next_value = patch.apply_to(&current_value);

            // 幂等：值与 authoritative current 相同 → 不写库，返回当前 revision。
            if next_value == current_value {
                *cell.lock().unwrap() = Some(Ok(SettingsUpdateResult {
                    value: next_value,
                    revision: current_revision,
                    changed: false,
                }));
                return Ok(());
            }

            let new_revision = new_revision();
            let data_json = serde_json::to_string(&next_value).map_err(|e| {
                AppError::new(
                    "INTERNAL_ERROR",
                    haven_common::ErrorKind::Internal,
                    "设置序列化失败",
                    false,
                )
                .with_source(e)
            })?;

            // 数据库层条件写（R-MAIN-07）：expected_revision 作为 SQL 条件；
            // 受影响行数 == 0（并发竞争者已先行提交）→ REVISION_CONFLICT，不覆盖。
            let written = tx.cas_write(
                section.as_str(),
                expected.as_deref(),
                &SettingsRow {
                    section: section.as_str().to_owned(),
                    schema_version: 1,
                    revision: new_revision.clone(),
                    data_json,
                    updated_at: haven_common::UtcMillis::now(),
                },
            )?;
            if !written {
                *cell.lock().unwrap() = Some(Err(conflict()));
                return Ok(());
            }

            *cell.lock().unwrap() = Some(Ok(SettingsUpdateResult {
                value: next_value,
                revision: Some(new_revision),
                changed: true,
            }));
            Ok(())
        })?;
        cell.lock().unwrap().take().expect("闭包必然写入结果")
    }
}

fn deserialize_value(section: SettingsSection, data_json: &str) -> Result<SettingsValue, AppError> {
    let value: SettingsValue = serde_json::from_str(data_json).map_err(|e| {
        AppError::new(
            "INTERNAL_ERROR",
            haven_common::ErrorKind::Internal,
            "设置数据损坏",
            false,
        )
        .with_source(e)
    })?;
    if value.section() != section {
        return Err(AppError::new(
            "INTERNAL_ERROR",
            haven_common::ErrorKind::Internal,
            "设置分区与存储不一致",
            false,
        ));
    }
    Ok(value)
}

/// 状态版本 token（opaque；唯一性由时间戳 + 纳秒后缀保证）。
fn new_revision() -> String {
    format!(
        "set-{:016x}-{:x}",
        haven_common::UtcMillis::now().0,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    )
}

fn conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        haven_common::ErrorKind::Conflict,
        "设置已被其他窗口/请求更新，请重新加载后再保存",
        false,
    )
}

fn validation(msg: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        haven_common::ErrorKind::Validation,
        msg,
        false,
    )
}
