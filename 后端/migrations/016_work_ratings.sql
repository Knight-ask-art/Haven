-- 016_work_ratings: 作品评分列（C1 元数据观感层）。
-- 豆瓣等元数据源回填的评分事实（value + 明确量表，契约 §11.2）。
ALTER TABLE works ADD COLUMN rating_value REAL;
ALTER TABLE works ADD COLUMN rating_scale REAL;
