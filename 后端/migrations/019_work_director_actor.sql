-- 019_work_director_actor: 作品演职员正列（A 方案）
-- 决策：放弃复用 original_title/language，按正规化新增 director/actor 列
-- 兼容：历史数据通过回填保留，原 original_title/language 不变

ALTER TABLE works ADD COLUMN director TEXT;
ALTER TABLE works ADD COLUMN actor TEXT;
-- 回填：若历史已把导演塞进 original_title（CMS10 导入旧逻辑），保留 original_title 同时回填 director
UPDATE works SET director = original_title WHERE director IS NULL AND original_title IS NOT NULL AND original_title != '';
-- 历史把演员塞进 language 的旧导入，回填 actor（仅当 actor 为空且 language 看似人名列表）
UPDATE works SET actor = language WHERE actor IS NULL AND language IS NOT NULL AND language != '' AND (instr(language, ',') > 0 OR instr(language, '，') > 0 OR instr(language, '/') > 0 OR instr(language, '、') > 0 OR length(language) > 20);
