//! 来源配置变更的事务纪律（契约 §36.2）。
//!
//! `sources` 段的**所有**写入都经 [`SourceRegistryService::mutate_sources`]：
//! 「BEGIN IMMEDIATE 内读取 authoritative 负载 → 闭包修改 → 以读到的行 revision
//! 作为 SQL 条件写」是同一个事务里的三步。于是两件旧路径做不到的事成立：
//!
//! - 并发的配置变更不会互相覆盖（旧路径的「读一次 → 整包 upsert」会让后写者把先写者
//!   的启用位/端点/自定义记录整段盖掉，也会让来源配置版本栅栏失效）；
//! - 条件写失败（并发方已先行提交）时返回 `REVISION_CONFLICT`，**绝不乐观覆盖**，
//!   也绝不向调用方谎报写入成功。
//! - **首次写入先播种出厂默认**（`initialize_factory_defaults`）：设置行尚不存在时，
//!   这一次写入先补齐出厂端点与出厂启用集合，再应用本次变更。于是 section 的初始状态
//!   不取决于哪一次写入先到——在第一次 `list` 之前显式停用某个出厂启用来源，这个选择
//!   同样会落地；而设置行一旦存在，出厂默认永不再播种，用户的选择不会被覆盖。
//!
//! 未注入 [`SettingsUoW`](crate::services::settings::SettingsUoW) 的组装面（只读面与
//! 测试替身）退回既有的「读 → 覆盖写」：那时没有事务通道，保留历史语义，而不是发明
//! 第三种写法。**首次播种不在这条退化之列**——出厂默认由同一份
//! `initialize_factory_defaults` 对每一次首次写入统一施加，有没有事务都一样，所以
//! 「设置行尚不存在时先播种再应用本次变更」在两条路径上都成立。退化的只是原子性与并发
//! 安全：没有条件写就没有 `REVISION_CONFLICT`，并发写入会互相覆盖，因此生产组合根必须
//! 注入真实事务。

use std::cell::RefCell;
use std::sync::Mutex;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};

use super::{
    PAYLOAD_SCHEMA_VERSION, SOURCES_SETTINGS_SECTION, SourceRegistryService, SourcesPayload,
    new_revision,
};
use crate::services::settings::SettingsTxPorts;

/// 一次负载变更的结果。
pub(super) enum SourcesMutation<T> {
    /// 负载没有实际变化（幂等重复设置）：不写库，也不产生新 revision。
    Unchanged(T),
    /// 负载已变化：条件写落地后返回。
    Changed(T),
}

/// `sources` 段负载 → 设置行（该段唯一的序列化点）。
pub(super) fn payload_row(payload: &SourcesPayload) -> Result<SettingsRow, AppError> {
    Ok(SettingsRow {
        section: SOURCES_SETTINGS_SECTION.to_owned(),
        schema_version: PAYLOAD_SCHEMA_VERSION,
        revision: new_revision(),
        data_json: serde_json::to_string(payload)
            .map_err(|e| haven_common::validation(format!("来源状态序列化失败: {e}")))?,
        updated_at: UtcMillis::now(),
    })
}

/// 设置行 JSON → 负载。损坏的配置是数据库错误，不猜、不静默丢弃。
pub(super) fn decode_payload(data_json: &str) -> Result<SourcesPayload, AppError> {
    serde_json::from_str(data_json).map_err(|e| {
        AppError::new(
            "DATABASE_ERROR",
            ErrorKind::Database,
            "来源启用状态数据损坏",
            true,
        )
        .with_source(e)
    })
}

/// 条件写失败：并发方已先行提交，本次变更不落地。
fn source_revision_conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "来源配置已被其他请求更新，请重新加载后再试",
        false,
    )
}

impl SourceRegistryService {
    /// 在单一设置事务里读 → 改 → 条件写。
    ///
    /// 闭包拿到的是**事务内**的 authoritative 负载与「是否已有落库行」（首次安装播种据此
    /// 判定）；闭包返回 [`SourcesMutation::Unchanged`] 时不写库——**除非**这一次是首次
    /// 写入、出厂播种真的改动了负载（那时必须落地，否则用户在这次写入里做的选择会连同
    /// 出厂默认一起丢掉）；返回 [`SourcesMutation::Changed`] 时条件写落地，条件不满足即
    /// `REVISION_CONFLICT`。
    ///
    /// 闭包是同步的，而且只做内存里的读 → 改；keystore 之类的外部 IO 必须留在**事务之外**，
    /// 也绝不能挤进闭包里。一次凭据写入的事务边界因此是
    /// 「暂存事务 → 外部写入 → 采纳/补偿事务」：外部 `set` **之前**有一个事务
    /// （登记暂存槽），**之后**还有一个事务（采纳，或补偿时登记清理义务）。把它说成
    /// 「外部 IO 都在事务之前」只说对了一半；准确的说法是外部写入夹在两个短事务之间，
    /// 两者之间不允许夹带任何数据库读取。见
    /// [`SourceRegistryService::set_custom_source_credential`]。
    pub(super) async fn mutate_sources<T>(
        &self,
        mutate: impl FnOnce(&mut SourcesPayload, bool) -> Result<SourcesMutation<T>, AppError>,
    ) -> Result<T, AppError> {
        let Some(uow) = self.settings_uow.clone() else {
            let row =
                SettingsRepository::get(self.settings.as_settings(), SOURCES_SETTINGS_SECTION)
                    .await?;
            let mut payload = match &row {
                Some(row) => decode_payload(&row.data_json)?,
                None => SourcesPayload::default(),
            };
            // 首次写入：先播种出厂默认，再应用这一次的变更（见 `initialize_factory_defaults`）。
            let seeded = row.is_none() && super::initialize_factory_defaults(&mut payload);
            let (value, write) = match mutate(&mut payload, row.is_some())? {
                SourcesMutation::Unchanged(value) => (value, seeded),
                SourcesMutation::Changed(value) => (value, true),
            };
            if !write {
                return Ok(value);
            }
            self.persist_payload(&payload).await?;
            return Ok(value);
        };

        // 闭包是 `FnOnce`（要移动 T 出去），而 `SettingsUoW::run` 收 `&dyn Fn`：
        // 用 RefCell 取出一次，保持外层闭包是 `Fn`。
        let mutate = RefCell::new(Some(mutate));
        let cell = Mutex::new(None::<Result<T, AppError>>);
        uow.run(&|tx: &dyn SettingsTxPorts| {
            let Some(mutate) = mutate.borrow_mut().take() else {
                // 事务闭包只会被调用一次；重复调用是契约破坏，按内部错误 fail closed。
                return Err(AppError::new(
                    "INTERNAL_ERROR",
                    ErrorKind::Internal,
                    "来源配置事务被重复执行",
                    false,
                ));
            };
            let row = tx.load(SOURCES_SETTINGS_SECTION)?;
            let mut payload = match &row {
                Some(row) => decode_payload(&row.data_json)?,
                None => SourcesPayload::default(),
            };
            // 设置行尚不存在（= 该 section 从未被写过）时，任何首次写入都要先播种出厂
            // 默认：否则「哪一次写入先到」会决定这个 section 的初始状态——先停用一个
            // 出厂启用来源会变成一次空操作，紧随其后的 `list()` 又把它自动打开。
            let seeded = row.is_none() && super::initialize_factory_defaults(&mut payload);
            let (value, write) = match mutate(&mut payload, row.is_some())? {
                SourcesMutation::Unchanged(value) => (value, seeded),
                SourcesMutation::Changed(value) => (value, true),
            };
            if write {
                let next = payload_row(&payload)?;
                let written = tx.cas_write(
                    SOURCES_SETTINGS_SECTION,
                    row.as_ref().map(|row| row.revision.as_str()),
                    &next,
                )?;
                if !written {
                    return Err(source_revision_conflict());
                }
            }
            *cell.lock().unwrap_or_else(|error| error.into_inner()) = Some(Ok(value));
            Ok(())
        })?;
        cell.lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .expect("闭包必然写入结果")
    }
}
