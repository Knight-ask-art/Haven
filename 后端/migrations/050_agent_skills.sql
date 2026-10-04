-- 050_agent_skills.sql
--
-- 内置 Agent Skill 的启用状态（原生 Skill 运行时，AI_SYSTEM.md §6）。
--
-- 只有"哪个技能、哪份内容、用户是否启用、何时改的"四件事。刻意没有正文列：
-- 技能正文随二进制分发（编译期嵌入），把它复制进数据库会制造第二个事实源，
-- 并让"启用时看到的内容"与"运行时加载的内容"有机会悄悄分叉。
--
-- instructions_hash 是启用当时那份内容的 SHA-256。内容变了而这里没变，
-- 读取侧就能判定该记录已 stale 并停止生效（fail closed），而不是让一份旧同意
-- 自动覆盖新内容。
CREATE TABLE agent_skill_states (
    skill_id TEXT PRIMARY KEY
        CHECK (length(skill_id) BETWEEN 1 AND 64),
    instructions_hash TEXT NOT NULL
        CHECK (length(instructions_hash) = 64),
    enabled INTEGER NOT NULL
        CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL
);
