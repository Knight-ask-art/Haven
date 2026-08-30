-- 002_history_consistency: HistoryEntry 内容层级一致性
--
-- 不变量：history_entries 的 (work_id, edition_id, media_item_id) 必须构成
-- work → edition → media_item 的合法层级链：
--   * edition 必须属于该 work
--   * media_item 必须属于该 edition
--
-- 说明：
-- * SQLite CHECK 无法跨表，因此用 BEFORE INSERT/UPDATE 触发器做 DB 级兜底
--   （ADR-002：DB 级不变量通过 CHECK/trigger 或 application invariant 表达）。
-- * Repository 层同时提供快速失败校验（友好错误码），触发器是最后防线。
-- * 已发布迁移禁止修改；如需扩展校验（如 progress/markers），追加新迁移。

CREATE TRIGGER trg_history_entries_chain_insert
BEFORE INSERT ON history_entries
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM editions e
        WHERE e.id = NEW.edition_id AND e.work_id = NEW.work_id
    )
    AND EXISTS (
        SELECT 1 FROM media_items m
        WHERE m.id = NEW.media_item_id AND m.edition_id = NEW.edition_id
    )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'HISTORY_CHAIN_INVALID: (work_id, edition_id, media_item_id) 必须构成合法层级链'
    );
END;

CREATE TRIGGER trg_history_entries_chain_update
BEFORE UPDATE OF work_id, edition_id, media_item_id ON history_entries
FOR EACH ROW
WHEN NOT (
    EXISTS (
        SELECT 1 FROM editions e
        WHERE e.id = NEW.edition_id AND e.work_id = NEW.work_id
    )
    AND EXISTS (
        SELECT 1 FROM media_items m
        WHERE m.id = NEW.media_item_id AND m.edition_id = NEW.edition_id
    )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'HISTORY_CHAIN_INVALID: (work_id, edition_id, media_item_id) 必须构成合法层级链'
    );
END;
