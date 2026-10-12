//! 来源健康事实：真实搜索观测的持久化与投影（契约 §36.2 的运行时 `health`）。
//!
//! 事实来源仍然是 `SourceRegistryService` + SQLite：健康不是第二份来源目录，而是
//! 一次次真实搜索观测在设置存储里的有界投影。
//!
//! 规则：
//! - 只有**真实搜索观测**才写健康：空成功响应算成功，错误与超时算失败。被跳过的
//!   来源（没有参与者、分类不匹配、没有配置端点）和取消的查询都不是探测，不写任何
//!   健康事实——既不制造成功，也不把取消算成失败。
//! - 每个来源一条 settings 行（section = `source_health:<sourceId>`），与来源配置
//!   （`sources` 段）分离：写健康永远不会覆盖并发的停用/端点/凭据变更，也不会因为
//!   编辑一个来源而丢掉其它来源的历史。
//! - 观测在 `SettingsUoW` 的 BEGIN IMMEDIATE 事务内落地，并在**同一事务**里重新读取
//!   来源配置：只有「来源仍然存在」且「配置版本与分发时一致」的观测才会被采纳。
//!   端点编辑、来源删除、凭据变更以及 A→B→A 这样的地址回环都会让旧观测作废；
//!   更早开始的检查即使更晚完成，也不会回退更晚开始的结果。
//! - 配置版本变化后从**空窗口**重新开始：旧版本的窗口描述的不是当前来源，旧版本
//!   （可能领先的）时间戳也不得压掉新版本的观测。
//! - 记录里只有布尔窗口与时间戳：没有地址、查询串、凭据或错误原文。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};

use super::{SOURCES_SETTINGS_SECTION, SourceRegistryService, SourcesPayload};
use crate::services::settings::{SettingsTxPorts, SettingsUoW};
use crate::wire::{SourceDescriptorDto, SourceHealthDto};

/// 健康段 section 前缀：每个来源一行，避免不同来源的观测互相覆盖。
const HEALTH_SECTION_PREFIX: &str = "source_health:";
/// 健康行的 schema 版本。
const HEALTH_SCHEMA_VERSION: u32 = 1;
/// 有界观测窗口：滚动成功率与「最近是否仍在工作」只看最近这些次真实搜索。
pub(super) const HEALTH_WINDOW: usize = 20;
/// 连续失败达到这个次数即视为不可用；更少的连续失败且窗口里还有成功则是降级。
const DOWN_AFTER_CONSECUTIVE_FAILURES: u32 = 3;

/// 某个来源的健康段 section 名（后端内部使用，不出 IPC）。
pub(super) fn health_section(source_id: &str) -> String {
    format!("{HEALTH_SECTION_PREFIX}{source_id}")
}

/// 一次真实搜索观测。由分发方在真实请求结束后构造，携带分发时读到的配置版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSearchObservation {
    pub source_id: String,
    /// 分发时读到的来源配置版本；`None` = 该来源从未发生过影响出站的配置变更。
    pub source_revision: Option<String>,
    /// 该次检查的开始时间（分发时刻），用于拒绝乱序完成的旧检查。
    pub started_at: UtcMillis,
    /// 空成功响应也算成功；错误与超时算失败。
    pub succeeded: bool,
    pub latency_ms: u64,
}

/// 记录真实搜索观测的端口（`SearchSourceService` 只依赖这一个方法）。
#[async_trait::async_trait]
pub trait SourceHealthRecorder: Send + Sync {
    /// 写入一次观测；观测因陈旧/乱序被丢弃不是错误。
    async fn record_search_observation(
        &self,
        observation: SourceSearchObservation,
    ) -> Result<(), AppError> {
        self.record_search_observations(vec![observation]).await
    }

    /// 在一次事务里记录同一轮搜索的全部来源观测，避免每个来源各自争抢数据库写锁。
    async fn record_search_observations(
        &self,
        observations: Vec<SourceSearchObservation>,
    ) -> Result<(), AppError>;
}

#[async_trait::async_trait]
impl SourceHealthRecorder for SourceRegistryService {
    async fn record_search_observations(
        &self,
        observations: Vec<SourceSearchObservation>,
    ) -> Result<(), AppError> {
        self.apply_search_observations(&observations).await
    }
}

/// 持久化的健康记录（每个来源一行）。窗口有界，字段全部不含地址或凭据。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceHealthRecord {
    /// 写入时的来源配置版本；与当前版本不一致的记录不再描述当前来源。
    #[serde(default)]
    source_revision: Option<String>,
    /// 最近一次被采纳观测的开始时间（乱序完成的旧检查据此被拒绝）。
    #[serde(default)]
    last_started_at_ms: i64,
    /// 最近一次观测时间。
    #[serde(default)]
    last_checked_ms: i64,
    /// 最近一次观测延迟（成功或失败都记录真实耗时）。
    #[serde(default)]
    last_latency_ms: u64,
    /// 有界观测窗口（true = 成功），最新在末尾。
    #[serde(default)]
    window: Vec<bool>,
}

impl SourceHealthRecord {
    /// 采纳一次观测：窗口、配置版本与「最后一次观测」一起推进。
    ///
    /// 四项必须一起推进——只追加窗口会让记录继续宣称一个已经过期的版本与时间戳，
    /// 投影就会把「上次检查」显示成上一次的真实时刻之外的东西。
    /// 「检查时刻」就是观测自己的开始时刻（Haven 以分发时刻计时），因此这里不需要
    /// 再读一次墙钟：健康事实因此完全可复现。
    fn observed(mut self, observation: &SourceSearchObservation) -> Self {
        if self.window.len() >= HEALTH_WINDOW {
            self.window.remove(0);
        }
        self.window.push(observation.succeeded);
        self.source_revision = observation.source_revision.clone();
        self.last_started_at_ms = observation.started_at.0;
        self.last_checked_ms = observation.started_at.0;
        self.last_latency_ms = observation.latency_ms;
        self
    }
}

/// Wire 描述符上的健康投影（`health` / `lastChecked` / `latencyMs` / `successRate`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SourceHealthProjection {
    pub health: SourceHealthDto,
    pub last_checked: Option<UtcMillis>,
    pub latency_ms: Option<u64>,
    pub success_rate: Option<f64>,
}

impl SourceHealthProjection {
    /// 没有可用观测时的诚实投影（出厂事实，契约 §36.2）。
    const UNKNOWN: Self = Self {
        health: SourceHealthDto::Unknown,
        last_checked: None,
        latency_ms: None,
        success_rate: None,
    };
}

/// 记录 → 投影。
///
/// 记录不存在、已损坏，或与当前来源配置版本不一致时诚实返回 unknown：陈旧记录描述的
/// 不是当前来源的配置，把它当作当前健康事实正是「编辑端点后旧结果仍然生效」的问题。
fn project(
    record: Option<&SourceHealthRecord>,
    current_revision: Option<&str>,
) -> SourceHealthProjection {
    let Some(record) = record else {
        return SourceHealthProjection::UNKNOWN;
    };
    if record.source_revision.as_deref() != current_revision || record.window.is_empty() {
        return SourceHealthProjection::UNKNOWN;
    }
    let consecutive_failures = record
        .window
        .iter()
        .rev()
        .take_while(|succeeded| !**succeeded)
        .count() as u32;
    let health = if consecutive_failures == 0 {
        SourceHealthDto::Ok
    } else if consecutive_failures >= DOWN_AFTER_CONSECUTIVE_FAILURES {
        SourceHealthDto::Down
    } else {
        SourceHealthDto::Degraded
    };
    let successes = record.window.iter().filter(|succeeded| **succeeded).count();
    SourceHealthProjection {
        health,
        last_checked: Some(UtcMillis(record.last_checked_ms)),
        latency_ms: Some(record.last_latency_ms),
        success_rate: Some(successes as f64 / record.window.len() as f64),
    }
}

impl SourceRegistryService {
    /// 在一次事务中记录一轮真实搜索的观测。
    ///
    /// 未注入 `SettingsUoW` 时不做任何事：那时没有任何写入通道，健康诚实保持 unknown，
    /// 绝不写入制造出来的成功或失败。
    ///
    /// 「读取来源配置 → 判定是否存在/版本是否一致 → 判定乱序 → 条件写入」全部在同一个
    /// BEGIN IMMEDIATE 事务内完成（与 `SettingsService` 同一条并发纪律），不使用克隆副本、
    /// 乐观 JSON 覆盖或重试。
    ///
    /// 观测被丢弃（陈旧/乱序/来源已删除）不是错误；真正的数据库错误由事务回滚并**向上
    /// 抛出**，条件写失败同样不谎报成功：调用方（搜索分发）把健康当作诊断事实，
    /// 不会因为一次健康写入失败而把已经完成的搜索判成失败。
    pub(super) async fn apply_search_observations(
        &self,
        observations: &[SourceSearchObservation],
    ) -> Result<(), AppError> {
        if observations.is_empty() {
            return Ok(());
        }
        let Some(uow): Option<Arc<dyn SettingsUoW>> = self.settings_uow.clone() else {
            return Ok(());
        };
        uow.run(&|tx: &dyn SettingsTxPorts| apply_observations(tx, observations))
    }

    /// 把持久化健康事实投影到描述符（陈旧记录按 unknown 处理）。
    ///
    /// 只读取已落库的观测，不发起任何探测或网络访问。
    pub(super) async fn project_health(
        &self,
        sources: &mut [SourceDescriptorDto],
        payload: &SourcesPayload,
    ) -> Result<(), AppError> {
        for source in sources.iter_mut() {
            let section = health_section(&source.source_id);
            let record: Option<SourceHealthRecord> =
                match SettingsRepository::get(self.settings.as_settings(), &section).await? {
                    Some(row) => serde_json::from_str(&row.data_json).ok(),
                    None => None,
                };
            let projection = project(
                record.as_ref(),
                payload
                    .source_revisions
                    .get(&source.source_id)
                    .map(String::as_str),
            );
            source.health = projection.health;
            source.last_checked = projection
                .last_checked
                .map(crate::mapper::utc_millis_to_rfc3339);
            source.latency_ms = projection.latency_ms;
            source.success_rate = projection.success_rate;
        }
        Ok(())
    }
}

/// 事务内的观测落地。
///
/// 丢弃而不报错的三种情况都是**诚实**的一部分：来源已删除（迟到的观测不得复活它）、
/// 配置版本已变（旧观测描述的不是当前来源）、观测比已落库的检查更早开始（乱序完成）。
fn apply_observations(
    tx: &dyn SettingsTxPorts,
    observations: &[SourceSearchObservation],
) -> Result<(), AppError> {
    // 一轮搜索只开一次事务、只读取一次来源配置：健康事实只相对**当前**配置成立。
    let payload = match tx.load(SOURCES_SETTINGS_SECTION)? {
        Some(row) => match serde_json::from_str::<SourcesPayload>(&row.data_json) {
            Ok(payload) => payload,
            // 配置本身损坏时由配置读取路径负责报错；这里不写健康，也不猜测来源身份。
            Err(_) => return Ok(()),
        },
        None => SourcesPayload::default(),
    };

    for observation in observations {
        if !source_is_registered(&payload, &observation.source_id)
            || payload
                .source_revisions
                .get(&observation.source_id)
                .map(String::as_str)
                != observation.source_revision.as_deref()
        {
            continue;
        }

        let section = health_section(&observation.source_id);
        let existing_row = tx.load(&section)?;
        // 损坏的健康行按「无记录」处理：它是诊断数据，不是权威配置，下一次真实观测覆盖它。
        // 版本不一致的记录同样按「无记录」处理：它描述的不是当前配置，于是新版本从**空窗口**
        // 重新开始，旧版本（可能领先）的时间戳也不再参与乱序判定——否则一次改址之后，
        // 旧版本一个「未来」的检查时刻会把新版本的真实观测永久压掉。
        let existing = existing_row
            .as_ref()
            .and_then(|row| serde_json::from_str::<SourceHealthRecord>(&row.data_json).ok())
            .filter(|record| record.source_revision == observation.source_revision);
        if let Some(existing) = &existing
            && observation.started_at.0 < existing.last_started_at_ms
        {
            // 更早开始的检查晚到：它描述的是更早的时刻，不得回退已经落库的更新观测。
            // 开始时戳相同不算更早（同一毫秒内开始的两个检查没有谁更新之分），按完成顺序采纳。
            continue;
        }

        let next = existing.unwrap_or_default().observed(observation);
        let data_json = serde_json::to_string(&next).map_err(|error| {
            AppError::new(
                "INTERNAL_ERROR",
                haven_common::ErrorKind::Internal,
                "来源健康数据序列化失败",
                false,
            )
            .with_source(error)
        })?;
        let row = SettingsRow {
            section,
            schema_version: HEALTH_SCHEMA_VERSION,
            revision: super::new_revision(),
            data_json,
            updated_at: UtcMillis::now(),
        };
        // 条件写：受影响行数为 0 表示同一来源的健康行被并发方推进，本次观测没有落地——
        // 如实返回冲突，不谎报写入成功。
        let written = tx.cas_write(
            &row.section,
            existing_row.as_ref().map(|row| row.revision.as_str()),
            &row,
        )?;
        if !written {
            return Err(health_revision_conflict());
        }
    }
    Ok(())
}

/// 同一来源的健康行被并发方推进：本次观测未被采纳（不是写入成功）。
fn health_revision_conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "来源健康事实已被并发更新",
        true,
    )
}

/// 来源是否仍然登记在册：内置目录条目，或仍然存在的自定义来源记录。
///
/// 迟到的观测绝不能为已删除的来源写回任何行（也不为未知 sourceId 造行）。
fn source_is_registered(payload: &SourcesPayload, source_id: &str) -> bool {
    if SourceRegistryService::is_custom_source_id(source_id) {
        return payload
            .custom_sources
            .iter()
            .any(|custom| custom.source_id == source_id);
    }
    super::builtin_catalog()
        .map(|catalog| catalog.iter().any(|source| source.source_id == source_id))
        .unwrap_or(false)
}
