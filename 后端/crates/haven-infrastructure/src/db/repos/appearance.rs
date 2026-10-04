//! Appearance Repository（SQLite，Stage 1B）。
//!
//! 资产元数据与首页布局使用 045/046 的窄表；布局明细和 singleton meta 在同一事务内
//! 条件写入，保证 CAS 失败不会留下部分模块行。原始文件字节由 Application 注入的
//! AppearanceAssetStorage 管理，本 Repository 不接触路径。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, UtcMillis};
use haven_domain::appearance::{
    AppearanceAssetDeleteOutcome, AppearanceAssetId, AppearanceAssetKind, AppearanceAssetMetadata,
    AppearanceAssetReference, AssetValidationState, HomeLayout, HomeModuleId, HomeModulePlacement,
    HomeModuleSize, OverviewLayout, OverviewModuleId, OverviewModulePlacement, OverviewModuleSize,
};
use haven_domain::contracts::{AppearanceRepository, HomeLayoutSnapshot, OverviewLayoutSnapshot};
use rusqlite::OptionalExtension;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteAppearanceRepository {
    db: Arc<Db>,
}

impl SqliteAppearanceRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AppearanceRepository for SqliteAppearanceRepository {
    async fn get_asset(
        &self,
        id: AppearanceAssetId,
    ) -> Result<Option<AppearanceAssetMetadata>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT id, kind, validation_state, byte_size, display_name
             FROM appearance_assets WHERE id = ?1",
            rusqlite::params![id.to_string()],
            asset_from_row,
        )
        .optional()
        .map_err(map_db_error("查询外观资产失败"))
    }

    async fn list_assets(
        &self,
        kind: Option<AppearanceAssetKind>,
    ) -> Result<Vec<AppearanceAssetMetadata>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, validation_state, byte_size, display_name
                 FROM appearance_assets
                 WHERE (?1 IS NULL OR kind = ?1)
                 ORDER BY updated_at DESC, id ASC",
            )
            .map_err(map_db_error("查询外观资产列表失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![kind.map(|value| value.as_str())],
                asset_from_row,
            )
            .map_err(map_db_error("查询外观资产列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("读取外观资产列表失败"))
    }

    async fn create_asset(&self, asset: &AppearanceAssetMetadata) -> Result<(), AppError> {
        asset.validate()?;
        let now = UtcMillis::now().0;
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO appearance_assets
                (id, kind, validation_state, byte_size, display_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![
                asset.id.to_string(),
                asset.kind.as_str(),
                asset.state.as_str(),
                i64::try_from(asset.byte_size)
                    .map_err(|_| invalid_asset("资产体积超出 SQLite 范围"))?,
                asset.display_name,
                now,
            ],
        )
        .map_err(map_db_error("保存外观资产失败"))?;
        Ok(())
    }

    /// 条件删除：资产仍被 `settings.appearance` 引用时零写入。
    ///
    /// 引用藏在 settings 行的 JSON 里（`customFontAssetId` 与 `wallpaper.assetId`），
    /// 没有外键能表达，所以占用判定与登记行删除必须落在**同一个 SQLite 写事务**里。
    /// 事务纪律与 settings 写路径逐条一致：`BEGIN IMMEDIATE` 在事务开始时就取写锁，
    /// 判定因此不依赖「本进程只有一条连接」这个偶然事实——另一条连接（第二个窗口、
    /// 另一个进程）要么排在这次判定之前提交（引用被读到，删除被挡住），要么排在其后
    /// （由 settings 侧的同事务资产校验兜住，见 `SettingsService::update`）。
    /// 只靠进程内的连接锁做不到这一点：锁一放开，并发连接看到的就已经是删除后的状态。
    ///
    /// 判定用 `json_extract` 直接读那两个路径，而不是反序列化整个外观 section：
    /// 引用集合是闭合的（见 [`AppearanceAssetReference::ALL`]），多出来的未来字段
    /// 不该让这条判定失败，也不该被误当成引用。
    ///
    /// 被挡住与「资产本来就不存在」都是**零写入**：两条路径都显式回滚，而不是提交一个
    /// 空事务，于是「没删到东西就不该留下任何写入痕迹」是这条路径的既成事实。
    async fn delete_asset(
        &self,
        id: AppearanceAssetId,
    ) -> Result<AppearanceAssetDeleteOutcome, AppError> {
        let mut conn = self.db.lock();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启外观资产删除事务失败"))?;
        let asset_id = id.to_string();
        let references = appearance_asset_references(&tx, &asset_id)?;
        if !references.is_empty() {
            drop(tx);
            return Ok(AppearanceAssetDeleteOutcome::blocked(references));
        }
        let affected = tx
            .execute(
                "DELETE FROM appearance_assets WHERE id = ?1",
                rusqlite::params![asset_id],
            )
            .map_err(map_db_error("删除外观资产失败"))?;
        if affected == 0 {
            drop(tx);
            return Ok(AppearanceAssetDeleteOutcome::missing());
        }
        tx.commit()
            .map_err(map_db_error("提交外观资产删除事务失败"))?;
        Ok(AppearanceAssetDeleteOutcome::removed())
    }

    async fn get_home_layout(&self) -> Result<Option<HomeLayoutSnapshot>, AppError> {
        let conn = self.db.lock();
        let meta: Option<(String, u32)> = conn
            .query_row(
                "SELECT revision, schema_version
                 FROM appearance_home_layout_meta WHERE singleton_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_db_error("查询首页布局版本失败"))?;
        let Some((revision, schema_version)) = meta else {
            return Ok(None);
        };

        let mut stmt = conn
            .prepare(
                "SELECT module_id, size, row_index, column_index, sort_order
                 FROM appearance_home_modules ORDER BY sort_order ASC",
            )
            .map_err(map_db_error("查询首页布局模块失败"))?;
        let rows = stmt
            .query_map([], |row| {
                let module_id: String = row.get(0)?;
                let size: String = row.get(1)?;
                let module = HomeModuleId::parse(&module_id).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "未知首页模块 ID",
                        )),
                    )
                })?;
                let size = HomeModuleSize::parse(&size).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "未知首页模块档位",
                        )),
                    )
                })?;
                HomeModulePlacement::new(module, size, row.get(2)?, row.get(3)?, row.get(4)?)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                error.user_message().to_owned(),
                            )),
                        )
                    })
            })
            .map_err(map_db_error("读取首页布局模块失败"))?;
        let modules = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("读取首页布局模块失败"))?;
        let layout = HomeLayout::new(schema_version, modules)
            .map_err(|error| invalid_storage(error.user_message()))?;
        Ok(Some(HomeLayoutSnapshot { layout, revision }))
    }

    async fn cas_save_home_layout(
        &self,
        expected_revision: Option<&str>,
        layout: &HomeLayout,
        revision: &str,
        updated_at: UtcMillis,
    ) -> Result<bool, AppError> {
        layout.validate()?;
        let mut result = false;
        self.db.with_tx(|tx| {
            let current: Option<String> = tx
                .query_row(
                    "SELECT revision FROM appearance_home_layout_meta WHERE singleton_id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_db_error("读取首页布局版本失败"))?;
            if current.as_deref() != expected_revision {
                return Ok(());
            }

            tx.execute("DELETE FROM appearance_home_modules", [])
                .map_err(map_db_error("清理旧首页布局失败"))?;
            for placement in &layout.modules {
                tx.execute(
                    "INSERT INTO appearance_home_modules
                        (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        placement.module.as_str(),
                        placement.size.as_str(),
                        placement.row,
                        placement.column,
                        placement.order,
                        layout.schema_version,
                        updated_at.0,
                    ],
                )
                .map_err(map_db_error("保存首页布局模块失败"))?;
            }
            tx.execute(
                "INSERT INTO appearance_home_layout_meta
                    (singleton_id, revision, schema_version, updated_at)
                 VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(singleton_id) DO UPDATE SET
                    revision = excluded.revision,
                    schema_version = excluded.schema_version,
                    updated_at = excluded.updated_at",
                rusqlite::params![revision, layout.schema_version, updated_at.0],
            )
            .map_err(map_db_error("保存首页布局版本失败"))?;
            result = true;
            Ok(())
        })?;
        Ok(result)
    }

    async fn cas_reset_home_layout(
        &self,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let mut result = false;
        self.db.with_tx(|tx| {
            let current: Option<String> = tx
                .query_row(
                    "SELECT revision FROM appearance_home_layout_meta WHERE singleton_id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_db_error("读取首页布局版本失败"))?;
            if current.as_deref() != expected_revision {
                return Ok(());
            }
            // CAS 通过不等于「真的重置掉了什么」：meta 行本来就不存在时（`expected` 也是
            // `None`），下面两条 DELETE 一行都删不掉。这时如实保持 `false`——一次空操作
            // 不是一次重置，调用方据此不会报出一个磁盘上从未发生过的变化。
            if current.is_none() {
                return Ok(());
            }
            tx.execute("DELETE FROM appearance_home_modules", [])
                .map_err(map_db_error("重置首页布局模块失败"))?;
            tx.execute(
                "DELETE FROM appearance_home_layout_meta WHERE singleton_id = 1",
                [],
            )
            .map_err(map_db_error("重置首页布局版本失败"))?;
            result = true;
            Ok(())
        })?;
        Ok(result)
    }

    /// 总览布局的读取与首页布局同形（meta 行决定「是否自定义」，模块行在另一张窄表），
    /// 只是走 048 的两张表：`None` 表示从未自定义，`Some(empty)` 表示显式隐藏全部模块。
    async fn get_overview_layout(&self) -> Result<Option<OverviewLayoutSnapshot>, AppError> {
        let conn = self.db.lock();
        let meta: Option<(String, u32)> = conn
            .query_row(
                "SELECT revision, schema_version
                 FROM appearance_overview_layout_meta WHERE singleton_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_db_error("查询总览布局版本失败"))?;
        let Some((revision, schema_version)) = meta else {
            return Ok(None);
        };

        let mut stmt = conn
            .prepare(
                "SELECT module_id, size, row_index, column_index, sort_order
                 FROM appearance_overview_modules ORDER BY sort_order ASC",
            )
            .map_err(map_db_error("查询总览布局模块失败"))?;
        let rows = stmt
            .query_map([], |row| {
                let module_id: String = row.get(0)?;
                let size: String = row.get(1)?;
                let module = OverviewModuleId::parse(&module_id).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "未知总览模块 ID",
                        )),
                    )
                })?;
                let size = OverviewModuleSize::parse(&size).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "未知总览模块档位",
                        )),
                    )
                })?;
                OverviewModulePlacement::new(module, size, row.get(2)?, row.get(3)?, row.get(4)?)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                error.user_message().to_owned(),
                            )),
                        )
                    })
            })
            .map_err(map_db_error("读取总览布局模块失败"))?;
        let modules = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("读取总览布局模块失败"))?;
        let layout = OverviewLayout::new(schema_version, modules)
            .map_err(|error| invalid_storage(error.user_message()))?;
        Ok(Some(OverviewLayoutSnapshot { layout, revision }))
    }

    /// 与首页布局同一条事务纪律：CAS 校验在事务内、排在模块行删除之前，因此过期的
    /// `expected_revision` 一行都不改；模块行与 meta 行同写同滚。
    async fn cas_save_overview_layout(
        &self,
        expected_revision: Option<&str>,
        layout: &OverviewLayout,
        revision: &str,
        updated_at: UtcMillis,
    ) -> Result<bool, AppError> {
        layout.validate()?;
        let mut result = false;
        self.db.with_tx(|tx| {
            let current: Option<String> = tx
                .query_row(
                    "SELECT revision FROM appearance_overview_layout_meta WHERE singleton_id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_db_error("读取总览布局版本失败"))?;
            if current.as_deref() != expected_revision {
                return Ok(());
            }

            tx.execute("DELETE FROM appearance_overview_modules", [])
                .map_err(map_db_error("清理旧总览布局失败"))?;
            for placement in &layout.modules {
                tx.execute(
                    "INSERT INTO appearance_overview_modules
                        (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        placement.module.as_str(),
                        placement.size.as_str(),
                        placement.row,
                        placement.column,
                        placement.order,
                        layout.schema_version,
                        updated_at.0,
                    ],
                )
                .map_err(map_db_error("保存总览布局模块失败"))?;
            }
            tx.execute(
                "INSERT INTO appearance_overview_layout_meta
                    (singleton_id, revision, schema_version, updated_at)
                 VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(singleton_id) DO UPDATE SET
                    revision = excluded.revision,
                    schema_version = excluded.schema_version,
                    updated_at = excluded.updated_at",
                rusqlite::params![revision, layout.schema_version, updated_at.0],
            )
            .map_err(map_db_error("保存总览布局版本失败"))?;
            result = true;
            Ok(())
        })?;
        Ok(result)
    }

    async fn cas_reset_overview_layout(
        &self,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let mut result = false;
        self.db.with_tx(|tx| {
            let current: Option<String> = tx
                .query_row(
                    "SELECT revision FROM appearance_overview_layout_meta WHERE singleton_id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_db_error("读取总览布局版本失败"))?;
            if current.as_deref() != expected_revision {
                return Ok(());
            }
            // 与首页布局同一条语义：meta 行本来就不存在时，这次「重置」一行都没删掉，
            // 结果必须如实是 `false`。
            if current.is_none() {
                return Ok(());
            }
            tx.execute("DELETE FROM appearance_overview_modules", [])
                .map_err(map_db_error("重置总览布局模块失败"))?;
            tx.execute(
                "DELETE FROM appearance_overview_layout_meta WHERE singleton_id = 1",
                [],
            )
            .map_err(map_db_error("重置总览布局版本失败"))?;
            result = true;
            Ok(())
        })?;
        Ok(result)
    }
}

fn asset_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppearanceAssetMetadata> {
    let id_text: String = row.get("id")?;
    let id = AppearanceAssetId::parse(&id_text).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "外观资产 ID 非规范 UUID",
            )),
        )
    })?;
    let kind_text: String = row.get("kind")?;
    let kind = AppearanceAssetKind::parse(&kind_text).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "未知外观资产种类",
            )),
        )
    })?;
    let state_text: String = row.get("validation_state")?;
    let state = AssetValidationState::parse(&state_text).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "未知外观资产校验状态",
            )),
        )
    })?;
    let byte_size: i64 = row.get("byte_size")?;
    let byte_size = u64::try_from(byte_size).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "外观资产体积非法",
            )),
        )
    })?;
    AppearanceAssetMetadata::new(id, kind, state, byte_size, row.get("display_name")?).map_err(
        |error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.user_message().to_owned(),
                )),
            )
        },
    )
}

fn invalid_asset(message: impl Into<String>) -> AppError {
    AppError::new(
        "APPEARANCE_INVALID_ASSET",
        haven_common::ErrorKind::Validation,
        message,
        false,
    )
}

fn invalid_storage(message: impl Into<String>) -> AppError {
    AppError::new(
        "APPEARANCE_STORAGE_CORRUPT",
        haven_common::ErrorKind::Database,
        message,
        false,
    )
}

/// `settings.appearance` 中引用 `asset_id` 的位置（空表示没有任何引用）。
///
/// 判定的路径**不在这里手写**：它们逐条来自 [`AppearanceAssetReference::json_path`]，
/// 与领域侧声明的「哪个字段持有资产 ID」是同一份事实——领域测试直接拿这些路径在
/// `AppearanceSettings` 的 serde 输出上取值。这里再抄一份字符串常量，就等于给这条
/// 删除守卫留了第二个真相源：字段改名后守卫会在「明明还有人用」的时候放行。
///
/// `json_valid` 让一行损坏的 JSON 变成「读不出引用」而不是一次 SQL 错误——
/// 损坏的设置行在任何读取路径上都已经回落到默认值，不该额外把删除变成数据库故障。
fn appearance_asset_references(
    conn: &rusqlite::Connection,
    asset_id: &str,
) -> Result<Vec<AppearanceAssetReference>, AppError> {
    // 路径是编译期常量（`json_path` 只返回两个字面量之一），不是用户输入；
    // 这里拼进 SQL 的每一个字符都来自领域，没有任何调用方可以影响它。
    let projections: Vec<String> = AppearanceAssetReference::ALL
        .into_iter()
        .map(|reference| {
            format!(
                "CASE WHEN json_valid(data_json) \
                 THEN json_extract(data_json, '$.{}') END",
                reference.json_path()
            )
        })
        .collect();
    let sql = format!(
        "SELECT {} FROM settings WHERE section = 'appearance'",
        projections.join(", ")
    );

    let mut statement = conn
        .prepare(&sql)
        .map_err(map_db_error("查询外观资产引用失败"))?;
    let mut rows = statement
        .query([])
        .map_err(map_db_error("查询外观资产引用失败"))?;
    let Some(row) = rows.next().map_err(map_db_error("查询外观资产引用失败"))? else {
        return Ok(Vec::new());
    };

    let mut references = Vec::new();
    for (index, reference) in AppearanceAssetReference::ALL.into_iter().enumerate() {
        let value: Option<String> = row
            .get(index)
            .map_err(map_db_error("读取外观资产引用失败"))?;
        if value.as_deref() == Some(asset_id) {
            references.push(reference);
        }
    }
    Ok(references)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::appearance::{
        HOME_LAYOUT_SCHEMA_VERSION, HomeModuleId, HomeModulePlacement, HomeModuleSize,
        OVERVIEW_LAYOUT_SCHEMA_VERSION,
    };

    #[tokio::test]
    async fn home_layout_repository_preserves_missing_and_empty_states() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db);
        assert!(repo.get_home_layout().await.unwrap().is_none());
        let empty =
            HomeLayout::new(haven_domain::appearance::HOME_LAYOUT_SCHEMA_VERSION, vec![]).unwrap();
        assert!(
            repo.cas_save_home_layout(None, &empty, "appearance-test-1", UtcMillis::now())
                .await
                .unwrap()
        );
        let saved = repo.get_home_layout().await.unwrap().unwrap();
        assert!(saved.layout.modules.is_empty());
        assert_eq!(saved.revision, "appearance-test-1");
        assert!(
            !repo
                .cas_save_home_layout(
                    None,
                    &HomeLayout::default(),
                    "appearance-test-2",
                    UtcMillis::now()
                )
                .await
                .unwrap()
        );
        assert!(
            repo.cas_reset_home_layout(Some("appearance-test-1"))
                .await
                .unwrap()
        );
        assert!(repo.get_home_layout().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn asset_roundtrip_and_filters_are_typed() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db);
        let asset = AppearanceAssetMetadata::new(
            AppearanceAssetId::parse("0196f0d2-0000-7000-8000-000000000001").unwrap(),
            AppearanceAssetKind::Font,
            AssetValidationState::Validated,
            12,
            Some("  示例字体  ".into()),
        )
        .unwrap();
        repo.create_asset(&asset).await.unwrap();
        assert_eq!(repo.get_asset(asset.id).await.unwrap(), Some(asset.clone()));
        assert_eq!(
            repo.list_assets(Some(AppearanceAssetKind::Font))
                .await
                .unwrap(),
            vec![asset.clone()]
        );
        assert!(repo.delete_asset(asset.id).await.unwrap().deleted);
        assert!(repo.get_asset(asset.id).await.unwrap().is_none());
    }

    /// 从未保存过布局时，「重置」是一次空操作，必须如实报 `false`。
    ///
    /// CAS 通过（`None == None`）不等于「真的重置掉了什么」：meta 行本来就不存在，两条
    /// DELETE 一行都删不掉。把这次空操作报成 `true`，就是对一个磁盘上从未发生过的变化
    /// 下结论——调用方据此会提示用户「已恢复默认」，而库里本来就没有任何自定义。
    #[tokio::test]
    async fn resetting_a_layout_that_was_never_saved_reports_no_change() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());

        assert!(
            !repo.cas_reset_home_layout(None).await.unwrap(),
            "从未保存过首页布局时，重置不得报成一次真实变化"
        );
        assert!(
            !repo.cas_reset_overview_layout(None).await.unwrap(),
            "从未保存过总览布局时，重置不得报成一次真实变化"
        );
        assert!(repo.get_home_layout().await.unwrap().is_none());
        assert!(repo.get_overview_layout().await.unwrap().is_none());

        // 真的保存过之后，重置就是一次真实变化。
        let saved = layout_of(vec![placement(
            HomeModuleId::Continue,
            HomeModuleSize::Small,
            0,
            0,
            0,
        )]);
        assert!(
            repo.cas_save_home_layout(None, &saved, "appearance-reset-1", UtcMillis::now())
                .await
                .unwrap()
        );
        assert!(
            repo.cas_reset_home_layout(Some("appearance-reset-1"))
                .await
                .unwrap(),
            "存在保存状态时重置必须报成一次真实变化"
        );
        assert!(repo.get_home_layout().await.unwrap().is_none());
    }

    #[test]
    fn domain_layout_rows_used_by_repository_are_real_grid_values() {
        let placement =
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Medium, 0, 0, 0)
                .unwrap();
        assert_eq!(placement.occupied_cells().len(), 2);
    }

    fn layout_of(placements: Vec<HomeModulePlacement>) -> HomeLayout {
        HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, placements).unwrap()
    }

    fn placement(
        module: HomeModuleId,
        size: HomeModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> HomeModulePlacement {
        HomeModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的放置必须合法")
    }

    fn module_row_count(db: &Db) -> i64 {
        db.lock()
            .query_row("SELECT COUNT(*) FROM appearance_home_modules", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// `settings.appearance` 行的原始落库形态（revision + data_json）。被占用挡住的删除
    /// 必须连这一行都不动：判定用的是设置行的 JSON，任何「顺手改写一下」都会让
    /// 「零写入」变成空话。
    fn appearance_settings_row(db: &Db) -> Option<(String, String)> {
        db.lock()
            .query_row(
                "SELECT revision, data_json FROM settings WHERE section = 'appearance'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    /// CAS 校验在事务内、且排在模块行删除之前：过期的 `expected_revision` 必须
    /// **一行都不改**（既不删旧模块，也不写新 revision）。
    #[tokio::test]
    async fn stale_cas_leaves_the_previous_layout_untouched() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let kept = layout_of(vec![placement(
            HomeModuleId::Continue,
            HomeModuleSize::Small,
            0,
            0,
            0,
        )]);
        assert!(
            repo.cas_save_home_layout(None, &kept, "appearance-keep", UtcMillis::now())
                .await
                .unwrap()
        );

        let replacement = layout_of(vec![placement(
            HomeModuleId::RecentlyAdded,
            HomeModuleSize::Large,
            4,
            0,
            0,
        )]);
        let saved = repo
            .cas_save_home_layout(None, &replacement, "appearance-stale", UtcMillis::now())
            .await
            .unwrap();
        assert!(!saved, "过期 revision 的 CAS 必须失败");

        let current = repo.get_home_layout().await.unwrap().unwrap();
        assert_eq!(current.revision, "appearance-keep");
        assert_eq!(current.layout, kept);
        assert_eq!(module_row_count(&db), 1);
    }

    /// 事务语义：模块行与 meta 行同一个事务。meta 行写失败（这里是 revision 长度违反
    /// 046 的 CHECK）时，已经删掉并重写的模块行必须一起回滚——不能留下「meta 还是旧
    /// 版本、模块已经换成新布局」这种谁也解释不了的半写状态。
    #[tokio::test]
    async fn failed_meta_write_rolls_back_the_module_rows() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let kept = layout_of(vec![placement(
            HomeModuleId::Continue,
            HomeModuleSize::Small,
            0,
            0,
            0,
        )]);
        assert!(
            repo.cas_save_home_layout(None, &kept, "appearance-keep", UtcMillis::now())
                .await
                .unwrap()
        );

        let replacement = layout_of(vec![placement(
            HomeModuleId::RecentlyAdded,
            HomeModuleSize::Large,
            4,
            0,
            0,
        )]);
        // revision 只有 5 个字符，046 的 CHECK 会在模块行已被重写之后拒绝 meta 行。
        let error = repo
            .cas_save_home_layout(
                Some("appearance-keep"),
                &replacement,
                "short",
                UtcMillis::now(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");

        let current = repo.get_home_layout().await.unwrap().unwrap();
        assert_eq!(current.revision, "appearance-keep", "meta 行必须还是旧版本");
        assert_eq!(current.layout, kept, "模块行必须随 meta 行一起回滚");
        assert_eq!(module_row_count(&db), 1);
    }

    /// 只有模块行、没有 meta 行（正常写入路径产不出来，但可能来自手工改动或半应用的
    /// 迁移）：读取侧必须把它当成「从未保存」，而不是返回半个布局；下一次保存会把
    /// 残留行整体替换掉，不留叠加。
    #[tokio::test]
    async fn module_rows_without_meta_are_never_returned_as_a_layout() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        db.lock()
            .execute(
                "INSERT INTO appearance_home_modules
                    (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                 VALUES ('continue', 'small', 0, 0, 0, 1, 1)",
                [],
            )
            .unwrap();
        assert!(repo.get_home_layout().await.unwrap().is_none());

        let saved_layout = HomeLayout::default();
        assert!(
            repo.cas_save_home_layout(
                None,
                &saved_layout,
                "appearance-orphan-fix",
                UtcMillis::now()
            )
            .await
            .unwrap()
        );
        let current = repo.get_home_layout().await.unwrap().unwrap();
        assert_eq!(current.layout, saved_layout);
        assert_eq!(
            module_row_count(&db),
            saved_layout.modules.len() as i64,
            "残留行不得与保存的布局叠加"
        );
    }

    // ---------- 总览布局（048）----------

    fn overview_placement(
        module: OverviewModuleId,
        size: OverviewModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> OverviewModulePlacement {
        OverviewModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的总览放置必须合法")
    }

    fn overview_layout_of(placements: Vec<OverviewModulePlacement>) -> OverviewLayout {
        OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, placements).unwrap()
    }

    /// 总览布局的模块行数与首页布局的模块行数（互不影响的证据）。
    fn overview_module_row_count(db: &Db) -> i64 {
        db.lock()
            .query_row(
                "SELECT COUNT(*) FROM appearance_overview_modules",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// 与首页布局同一条存储纪律：`None`（从未自定义）与 `Some(empty)`（显式隐藏全部
    /// 模块）是两种不同事实，且总览的保存状态**不影响**首页布局。
    #[tokio::test]
    async fn overview_layout_repository_preserves_missing_and_empty_states() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        assert!(repo.get_overview_layout().await.unwrap().is_none());

        let empty = OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, vec![]).unwrap();
        assert!(
            repo.cas_save_overview_layout(None, &empty, "overview-test-1", UtcMillis::now())
                .await
                .unwrap()
        );
        let saved = repo.get_overview_layout().await.unwrap().unwrap();
        assert!(saved.layout.modules.is_empty());
        assert_eq!(saved.revision, "overview-test-1");

        // 陈旧 revision（`null`）不得写入。
        assert!(
            !repo
                .cas_save_overview_layout(
                    None,
                    &OverviewLayout::default(),
                    "overview-test-2",
                    UtcMillis::now()
                )
                .await
                .unwrap()
        );

        // 总览的自定义不产生首页布局：两张事实各自独立。
        assert!(
            repo.get_home_layout().await.unwrap().is_none(),
            "保存总览布局不得产生首页布局状态"
        );
        assert_eq!(module_row_count(&db), 0);

        assert!(
            repo.cas_reset_overview_layout(Some("overview-test-1"))
                .await
                .unwrap()
        );
        assert!(repo.get_overview_layout().await.unwrap().is_none());
    }

    /// 首页布局的自定义同样不产生总览布局（反方向）。
    #[tokio::test]
    async fn home_layout_save_does_not_create_an_overview_layout() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        assert!(
            repo.cas_save_home_layout(
                None,
                &HomeLayout::default(),
                "appearance-home-1",
                UtcMillis::now()
            )
            .await
            .unwrap()
        );
        assert!(repo.get_overview_layout().await.unwrap().is_none());
        assert_eq!(overview_module_row_count(&db), 0);

        // 反过来：写了首页的 revision 不能当总览的 CAS token 用。
        assert!(
            !repo
                .cas_save_overview_layout(
                    Some("appearance-home-1"),
                    &OverviewLayout::default(),
                    "overview-1",
                    UtcMillis::now()
                )
                .await
                .unwrap(),
            "首页的 revision 不是总览的 revision"
        );
    }

    /// CAS 校验在事务内、且排在模块行删除之前：过期 revision 一行都不改。
    #[tokio::test]
    async fn stale_overview_cas_leaves_the_previous_layout_untouched() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let kept = overview_layout_of(vec![overview_placement(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Small,
            0,
            0,
            0,
        )]);
        assert!(
            repo.cas_save_overview_layout(None, &kept, "overview-keep", UtcMillis::now())
                .await
                .unwrap()
        );

        let replacement = overview_layout_of(vec![overview_placement(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Large,
            4,
            0,
            0,
        )]);
        assert!(
            !repo
                .cas_save_overview_layout(None, &replacement, "overview-stale", UtcMillis::now())
                .await
                .unwrap(),
            "过期 revision 的 CAS 必须失败"
        );

        let current = repo.get_overview_layout().await.unwrap().unwrap();
        assert_eq!(current.revision, "overview-keep");
        assert_eq!(current.layout, kept);
        assert_eq!(overview_module_row_count(&db), 1);
    }

    /// 事务语义：总览的模块行与 meta 行同一个事务。meta 行写失败（revision 长度违反 048
    /// 的 CHECK）时，已删掉并重写的模块行必须一起回滚。
    #[tokio::test]
    async fn failed_overview_meta_write_rolls_back_the_module_rows() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let kept = overview_layout_of(vec![overview_placement(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Small,
            0,
            0,
            0,
        )]);
        assert!(
            repo.cas_save_overview_layout(None, &kept, "overview-keep", UtcMillis::now())
                .await
                .unwrap()
        );

        let replacement = overview_layout_of(vec![overview_placement(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Large,
            4,
            0,
            0,
        )]);
        let error = repo
            .cas_save_overview_layout(
                Some("overview-keep"),
                &replacement,
                "short",
                UtcMillis::now(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");

        let current = repo.get_overview_layout().await.unwrap().unwrap();
        assert_eq!(current.revision, "overview-keep", "meta 行必须还是旧版本");
        assert_eq!(current.layout, kept, "模块行必须随 meta 行一起回滚");
        assert_eq!(overview_module_row_count(&db), 1);
    }

    /// 只有模块行、没有 meta 行时读取侧必须当成「从未保存」，而不是返回半个布局。
    #[tokio::test]
    async fn overview_module_rows_without_meta_are_never_returned_as_a_layout() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        db.lock()
            .execute(
                "INSERT INTO appearance_overview_modules
                    (module_id, size, row_index, column_index, sort_order, schema_version, updated_at)
                 VALUES ('metrics', 'small', 0, 0, 0, 1, 1)",
                [],
            )
            .unwrap();
        assert!(repo.get_overview_layout().await.unwrap().is_none());

        let saved_layout = OverviewLayout::default();
        assert!(
            repo.cas_save_overview_layout(
                None,
                &saved_layout,
                "overview-orphan-fix",
                UtcMillis::now()
            )
            .await
            .unwrap()
        );
        let current = repo.get_overview_layout().await.unwrap().unwrap();
        assert_eq!(current.layout, saved_layout);
        assert_eq!(
            overview_module_row_count(&db),
            saved_layout.modules.len() as i64,
            "残留行不得与保存的布局叠加"
        );
    }

    /// 默认总览布局必须能落库并原样读回：这是「三列网格 + 5 个模块」在 SQL 侧也成立的
    /// 证据（越界 CHECK、占格触发器与 UNIQUE(sort_order) 都不拒绝它）。
    #[tokio::test]
    async fn default_overview_layout_round_trips_through_sqlite() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db);
        let layout = OverviewLayout::default();
        assert!(
            repo.cas_save_overview_layout(None, &layout, "overview-default", UtcMillis::now())
                .await
                .unwrap()
        );
        let saved = repo.get_overview_layout().await.unwrap().unwrap();
        assert_eq!(saved.layout, layout);
        assert_eq!(saved.layout.ordered_modules().len(), 5);
    }

    /// `delete_asset` 只在真的删掉登记行时返回 `deleted = true`：调用方据此决定是否
    /// 清理字节，因此「没删到」必须是可区分的结果，而不是静默成功。
    #[tokio::test]
    async fn delete_asset_reports_whether_a_row_was_removed() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db);
        let asset = AppearanceAssetMetadata::new(
            AppearanceAssetId::parse("0196f0d2-0000-7000-8000-000000000002").unwrap(),
            AppearanceAssetKind::DynamicWallpaper,
            AssetValidationState::Validated,
            32,
            None,
        )
        .unwrap();
        repo.create_asset(&asset).await.unwrap();
        assert!(repo.delete_asset(asset.id).await.unwrap().deleted);
        assert!(!repo.delete_asset(asset.id).await.unwrap().deleted);
    }

    /// 占用判定直接读 settings 行的 JSON 引用：被引用的资产删除时零写入，
    /// 未相关的外观设置（自定义主题等）不影响删除。
    #[tokio::test]
    async fn delete_asset_is_blocked_while_appearance_settings_reference_it() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let font = AppearanceAssetId::parse("0196f0d2-0000-7000-8000-0000000000a1").unwrap();
        let wallpaper = AppearanceAssetId::parse("0196f0d2-0000-7000-8000-0000000000a2").unwrap();
        let unused = AppearanceAssetId::parse("0196f0d2-0000-7000-8000-0000000000a3").unwrap();
        for id in [font, wallpaper, unused] {
            repo.create_asset(
                &AppearanceAssetMetadata::new(
                    id,
                    AppearanceAssetKind::Font,
                    AssetValidationState::Validated,
                    16,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        }

        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
                 VALUES ('appearance', 1, 'rev-1', ?1, 1)",
                rusqlite::params![format!(
                    "{{\"section\":\"appearance\",\"theme\":\"custom\",\"density\":\"comfortable\",\"sidebar\":\"auto\",\"reduceMotion\":false,\"customFontAssetId\":\"{font}\",\"wallpaper\":{{\"kind\":\"static\",\"assetId\":\"{wallpaper}\"}}}}"
                )],
            )
            .unwrap();
        }
        let settings_before = appearance_settings_row(&db).expect("设置行必须已写入");

        let blocked = repo.delete_asset(font).await.unwrap();
        assert!(!blocked.deleted, "被字体引用的资产不得删除");
        assert_eq!(
            blocked.references,
            vec![AppearanceAssetReference::CustomFont]
        );

        let blocked = repo.delete_asset(wallpaper).await.unwrap();
        assert!(!blocked.deleted, "被壁纸引用的资产不得删除");
        assert_eq!(
            blocked.references,
            vec![AppearanceAssetReference::Wallpaper]
        );

        // 零写入：被挡住时 settings 行必须逐字节还是原来那一行。
        let settings_after = appearance_settings_row(&db);
        assert_eq!(settings_after, Some(settings_before), "设置行不得被改写");

        assert!(repo.delete_asset(unused).await.unwrap().deleted);
        assert!(repo.get_asset(font).await.unwrap().is_some());
        assert!(repo.get_asset(wallpaper).await.unwrap().is_some());
    }

    /// 外观设置里清除引用后，同一次删除立刻放行。
    #[tokio::test]
    async fn delete_asset_is_allowed_once_the_reference_is_cleared() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteAppearanceRepository::new(db.clone());
        let id = AppearanceAssetId::parse("0196f0d2-0000-7000-8000-0000000000b1").unwrap();
        repo.create_asset(
            &AppearanceAssetMetadata::new(
                id,
                AppearanceAssetKind::Font,
                AssetValidationState::Validated,
                16,
                None,
            )
            .unwrap(),
        )
        .await
        .unwrap();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
                 VALUES ('appearance', 1, 'rev-1', ?1, 1)",
                rusqlite::params![format!(
                    "{{\"section\":\"appearance\",\"customFontAssetId\":\"{id}\"}}"
                )],
            )
            .unwrap();
            conn.execute(
                "UPDATE settings SET data_json = '{\"section\":\"appearance\",\"customFontAssetId\":null}' WHERE section = 'appearance'",
                [],
            )
            .unwrap();
        }
        assert!(repo.delete_asset(id).await.unwrap().deleted);
    }
}
