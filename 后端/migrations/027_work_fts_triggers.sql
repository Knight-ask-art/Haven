-- 027_work_fts_triggers: FTS5 virtual table and triggers for global search
-- Separated from 025 to avoid legacy DB upgrade issues with content sync

CREATE VIRTUAL TABLE work_fts USING fts5(
    search_title,
    search_original_title,
    search_body,
    content='works',
    content_rowid='rowid',
    tokenize='unicode61'
);

INSERT INTO work_fts(work_fts) VALUES('rebuild');

CREATE TRIGGER works_fts_ai AFTER INSERT ON works BEGIN
  INSERT INTO work_fts(rowid, search_title, search_original_title, search_body)
  VALUES (new.rowid, new.search_title, new.search_original_title, new.search_body);
END;

CREATE TRIGGER works_fts_ad AFTER DELETE ON works BEGIN
  INSERT INTO work_fts(work_fts, rowid, search_title, search_original_title, search_body)
  VALUES ('delete', old.rowid, old.search_title, old.search_original_title, old.search_body);
END;

CREATE TRIGGER works_fts_au AFTER UPDATE ON works BEGIN
  INSERT INTO work_fts(work_fts, rowid, search_title, search_original_title, search_body)
  VALUES ('delete', old.rowid, old.search_title, old.search_original_title, old.search_body);
  INSERT INTO work_fts(rowid, search_title, search_original_title, search_body)
  VALUES (new.rowid, new.search_title, new.search_original_title, new.search_body);
END;
