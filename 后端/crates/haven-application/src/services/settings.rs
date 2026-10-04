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
use haven_domain::appearance::{AppearanceAssetId, AppearanceAssetKind, WallpaperSelection};
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

    /// **事务内跨表引用校验**：待写入的值里若引用了别的表的事实（当前只有
    /// `appearance.interfaceFontAssetId` → `interface_font_assets.id`），
    /// 必须在**同一个事务**里确认它存在，否则「设置引用了已删除的字体」这类
    /// 失效状态可以绕过删除保护写进设置。
    ///
    /// 默认实现不做校验（测试替身与不涉及跨表引用的实现无需关心）；
    /// SQLite 实现在同一连接上查询。返回 `Err` 时整个更新回滚，不写库、不发事件。
    fn validate_references(&self, _value: &SettingsValue) -> Result<(), AppError> {
        Ok(())
    }
    /// 读取可引用外观资产的**种类**（同一事务内）。`Ok(None)` 表示没有可引用的登记行
    /// （ID 不存在、状态尚未验证/已拒绝，或登记行的种类不是闭合集合里的任何一种）。
    ///
    /// `appearance.customFontAssetId` 与 `appearance.wallpaper.assetId` 是设置 JSON 里
    /// **仅有的**两处资产引用，而设置行与资产登记行之间没有外键：删除资产时判定的
    /// 「有没有人在用」与本方法判定的「引用的东西还在不在」必须是同一条事务边界上的
    /// 两次查询，否则两条路径都只看得到自己提交前的那一瞬间——一边删掉了资产，另一边
    /// 刚好把它的 ID 写进设置，留下的引用谁都不认识。
    ///
    /// 只查询、不写入：被拒绝的更新必须零写入（由 `SettingsUoW::run` 的事务回滚兜住）。
    fn validated_appearance_asset_kind(
        &self,
        id: AppearanceAssetId,
    ) -> Result<Option<AppearanceAssetKind>, AppError> {
        let _ = id;
        Ok(None)
    }
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
    /// - appearance 分区新写入的非空资产引用在**同一事务**里再确认一次存在且种类匹配
    ///   （见 [`appearance_reference_requirements`]）：不匹配则稳定
    ///   `APPEARANCE_ASSET_UNAVAILABLE`，零写入。
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

            // 跨表引用校验：与本次写入同一事务，避免「设置引用了已删除的字体」
            // 这类失效状态被写进 authoritative 行。
            tx.validate_references(&next_value)?;
            // 外观资产引用必须先落回真实的登记行，再允许写进设置：设置 JSON 与资产登记行
            // 之间没有外键，删除侧也只在**自己的**事务里判定占用，所以「删除刚提交、
            // 引用随即写入」这条缝隙只能在这里堵——查库与写入同属一个事务，查不到的引用
            // 一行都不写。
            for (id, required) in appearance_reference_requirements(&current_value, &next_value) {
                let actual = tx.validated_appearance_asset_kind(id)?;
                if actual == Some(required) {
                    continue;
                }
                let error = asset_reference_unavailable(required, actual);
                *cell.lock().unwrap() = Some(Err(error));
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

/// 本次更新**新写入的非空**外观资产引用 → 它必须匹配的资产种类。
///
/// 只比较新旧值，不做全量校验，这条边界因此是「改了什么就查什么」：
/// - 引用没变的字段（包括历史遗留的悬空 ID）不查库，旧数据不会阻塞无关字段的修改；
/// - 清空引用（`customFontAssetId: null` / `wallpaper: {"kind":"none"}`）不需要任何资产；
/// - 壁纸按**整个选择**比较：`static A` → `dynamic A` 的 ID 没变，语义却变了，
///   必须按新种类重新校验（同一个 ID 不可能既是静态又是动态壁纸）。
fn appearance_reference_requirements(
    previous: &SettingsValue,
    next: &SettingsValue,
) -> Vec<(AppearanceAssetId, AppearanceAssetKind)> {
    let (SettingsValue::Appearance(previous), SettingsValue::Appearance(next)) = (previous, next)
    else {
        return Vec::new();
    };

    let mut requirements = Vec::new();
    if next.custom_font_asset_id != previous.custom_font_asset_id {
        if let Some(id) = next.custom_font_asset_id {
            requirements.push((id, AppearanceAssetKind::Font));
        }
    }
    if next.wallpaper != previous.wallpaper {
        match next.wallpaper {
            WallpaperSelection::None => {}
            WallpaperSelection::Static(id) => {
                requirements.push((id, AppearanceAssetKind::StaticWallpaper));
            }
            WallpaperSelection::Dynamic(id) => {
                requirements.push((id, AppearanceAssetKind::DynamicWallpaper));
            }
        }
    }
    requirements
}

/// 新写入的外观资产引用对不上真实登记行。
///
/// 状态本身不非法（用户选的 ID 曾经真实存在），只是当前引用不上：可重试的是「换一个
/// 资产再存」，不是「原样重试」。因此是 `NotFound` + 不可重试，与读路径的
/// `APPEARANCE_ASSET_UNAVAILABLE` 共用同一个错误码——同一件事（这个资产用不了）
/// 在前端只该有一个分支。
///
/// 文案只说位置与种类（都是界面上已有的说法），不含任何路径、行号或内部字段名。
fn asset_reference_unavailable(
    required: AppearanceAssetKind,
    actual: Option<AppearanceAssetKind>,
) -> AppError {
    let message = match actual {
        Some(_) => format!("所选资产不是{}，请重新选择", required.label()),
        None => format!("所选{}资产不可用，请重新选择", required.label()),
    };
    AppError::new(
        "APPEARANCE_ASSET_UNAVAILABLE",
        haven_common::ErrorKind::NotFound,
        message,
        false,
    )
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
