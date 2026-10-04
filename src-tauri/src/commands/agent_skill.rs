//! 内置 Agent Skill Commands（原生 Skill 运行时）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../docs/architecture/AI_SYSTEM.md) §6。
//!
//! 命令层只做四件事：typed request 解析、调用 Application service、把 `AppError` 映射成
//! `ErrorDto`；同步 SQLite 在 blocking worker 上执行。
//!
//! 这里**没有**任何"注册技能 / 安装技能 / 从路径加载技能"的入口，也没有把技能正文
//! 作为参数收下的命令。调用方只能引用一个 `skillId`；正文来自随应用分发的内嵌内容。
//! 这正是"技能不是权限、也不是任意提示词通道"在命令层的落点。

use tauri::State;

use haven_application::services::agent_skill::{
    AgentSkillListResultDto, AgentSkillService, AgentSkillSetEnabledRequest, AgentSkillStateDto,
};
use haven_application::wire::ErrorDto;

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 列出全部内置技能及其权威启用状态。
pub async fn run_agent_skill_list(
    agent_skills: &AgentSkillService,
) -> Result<AgentSkillListResultDto, ErrorDto> {
    agent_skills
        .list()
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 启用/停用一项内置技能。
///
/// 幂等：重复提交同一状态不会改写权威记录的更新时间，也不会报错。
/// 对已 `stale` 的技能再次启用就是"对当前内容的重新确认"。
pub async fn run_agent_skill_set_enabled(
    agent_skills: &AgentSkillService,
    request: AgentSkillSetEnabledRequest,
) -> Result<AgentSkillStateDto, ErrorDto> {
    agent_skills
        .set_enabled(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn agent_skill_list(
    state: State<'_, AppState>,
) -> Result<AgentSkillListResultDto, ErrorDto> {
    let agent_skills = state.agent_skills.clone();
    run_blocking(move || async move { run_agent_skill_list(&agent_skills).await }).await
}

#[tauri::command]
pub async fn agent_skill_set_enabled(
    state: State<'_, AppState>,
    request: AgentSkillSetEnabledRequest,
) -> Result<AgentSkillStateDto, ErrorDto> {
    let agent_skills = state.agent_skills.clone();
    run_blocking(move || async move { run_agent_skill_set_enabled(&agent_skills, request).await })
        .await
}
