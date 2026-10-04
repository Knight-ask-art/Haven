//! 内置 Agent Skill 的 Application 入口（原生 Skill 运行时）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §6。
//!
//! 这个服务把三件事接起来，缺一不可：
//!
//! ```text
//! 分发内容（AgentSkillRegistryPort）
//!   + 权威启用状态（AgentSkillStateRepository，SQLite）
//!   → 本次请求真正生效的技能说明（active_skills）
//!   → Provider 请求的说明性上下文
//! ```
//!
//! 三条边界：
//!
//! 1. **端口是只读的**。没有任何方法能注册、替换或注入一项技能——技能集合是构建
//!    产物，不是运行时输入。调用方只能**引用** `skill_id`，不能提供正文。
//! 2. **启用不改权限**。本服务不产生 capability、不产生工具、不产生 Apply 路径；
//!    它只回答"这次请求该带哪段说明性文字"。
//! 3. **状态是权威持久化事实**。没有内存缓存、没有 localStorage 影子副本；
//!    每次读取都回 SQLite，因此重启后行为完全一致。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::agent_skill::{
    ActiveAgentSkill, AgentSkillEnablement, AgentSkillId, BuiltinAgentSkill, resolve_active_skill,
};
use haven_domain::contracts::AgentSkillStateRepository;
use serde::{Deserialize, Serialize};

/// 内置技能目录端口。
///
/// 实现方（Infrastructure）提供随应用分发的技能正文。端口**只读**且不接受路径、
/// 不接受目录名、不接受任何调用方提供的定位符：能"按调用方给的路径加载技能"的端口，
/// 等价于一个任意文件读取入口。
pub trait AgentSkillRegistryPort: Send + Sync {
    /// 全部内置技能，按 `skill_id` 升序（顺序稳定，UI 与请求拼装都依赖它）。
    fn builtin_skills(&self) -> Vec<&BuiltinAgentSkill>;

    /// 按 id 查找；不存在返回 `None`（**不**回退到"第一个技能"）。
    fn find(&self, skill_id: &AgentSkillId) -> Option<&BuiltinAgentSkill>;
}

/// 一项内置技能对用户的可见状态。
///
/// 三态是**互斥且闭合**的：没有"部分启用"，也没有"未知"。
/// `Stale` 是这里最重要的一个：内容换了而记录还是旧的，技能**不生效**，
/// 用户要重新确认一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillActivationDto {
    /// 已启用，且内容摘要与启用时一致。
    Enabled,
    /// 未启用（从未启用过，或用户显式关闭）。
    Disabled,
    /// 启用过，但分发内容已变化。不生效；需要用户重新确认。
    Stale,
}

/// 单项技能的投影。**不含正文**：正文只在模型请求内部拼装，不进 wire。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSkillStateDto {
    pub schema_version: u32,
    pub skill_id: String,
    /// 来自技能文档 frontmatter 的 `description`，原样投影。
    pub description: String,
    /// 注入模型请求的正文长度（字符），让用户对"会送出去多少字"有事实依据。
    pub instructions_chars: u32,
    pub state: AgentSkillActivationDto,
}

/// `agent_skill_list` 响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSkillListResultDto {
    pub schema_version: u32,
    pub skills: Vec<AgentSkillStateDto>,
}

/// `agent_skill_set_enabled` 请求。
///
/// 只有 id 与布尔值。**没有**正文、没有 hash、没有路径：摘要由服务端按当前分发内容
/// 计算，调用方无法"声明"自己启用的到底是哪份内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSkillSetEnabledRequest {
    pub skill_id: String,
    pub enabled: bool,
}

/// 内置技能服务。
#[derive(Clone)]
pub struct AgentSkillService {
    registry: Arc<dyn AgentSkillRegistryPort>,
    states: Arc<dyn AgentSkillStateRepository>,
}

impl AgentSkillService {
    pub fn new(
        registry: Arc<dyn AgentSkillRegistryPort>,
        states: Arc<dyn AgentSkillStateRepository>,
    ) -> Self {
        Self { registry, states }
    }

    /// 列出全部内置技能及其权威状态。
    ///
    /// 没有记录 = `Disabled`（而不是"未知"）：从未启用过就是未启用。
    pub async fn list(&self) -> Result<AgentSkillListResultDto, AppError> {
        let records = self.states.list().await?;
        let mut skills = Vec::new();
        for skill in self.registry.builtin_skills() {
            let record = records
                .iter()
                .find(|record| record.skill_id() == skill.id());
            skills.push(project(skill, record));
        }
        Ok(AgentSkillListResultDto {
            schema_version: 1,
            skills,
        })
    }

    /// 启用/停用一项技能，并返回写入后的状态。
    ///
    /// 启用的语义是"同意当前这一份内容"：服务端按当前分发内容算摘要，所以
    /// **对 `Stale` 的技能再次启用就是重新确认**，不需要第二个"确认"动作。
    /// 未知 id 是 `AGENT_SKILL_UNKNOWN`，不是"顺手创建一个"。
    pub async fn set_enabled(
        &self,
        request: &AgentSkillSetEnabledRequest,
    ) -> Result<AgentSkillStateDto, AppError> {
        let skill_id = AgentSkillId::parse(&request.skill_id)?;
        let skill = self.registry.find(&skill_id).ok_or_else(unknown_skill)?;
        let enablement = AgentSkillEnablement::parse(
            skill_id,
            skill.manifest().instructions_hash(),
            request.enabled,
        )?;
        self.states.put(&enablement, UtcMillis::now().0).await?;
        Ok(project(skill, Some(&enablement)))
    }

    /// 本次模型请求真正生效的技能说明。
    ///
    /// 这是唯一的"生成"入口，也是本模块存在的理由：请求路径不读 SQLite、
    /// 不读文件，只调用这里。任何未启用、已 stale、或 id 对不上的记录都在
    /// [`resolve_active_skill`] 里被丢弃，因此"启用过"从不等于"生效"。
    pub async fn active_skills(&self) -> Result<Vec<ActiveAgentSkill>, AppError> {
        let records = self.states.list().await?;
        let mut active = Vec::new();
        for skill in self.registry.builtin_skills() {
            let record = records
                .iter()
                .find(|record| record.skill_id() == skill.id());
            if let Some(resolved) = resolve_active_skill(skill, record) {
                active.push(resolved);
            }
        }
        Ok(active)
    }
}

fn project(skill: &BuiltinAgentSkill, record: Option<&AgentSkillEnablement>) -> AgentSkillStateDto {
    let state = match record {
        Some(record) if record.enabled() && record.is_stale_for(skill) => {
            AgentSkillActivationDto::Stale
        }
        Some(record) if record.enabled() => AgentSkillActivationDto::Enabled,
        _ => AgentSkillActivationDto::Disabled,
    };
    AgentSkillStateDto {
        schema_version: 1,
        skill_id: skill.id().as_str().to_owned(),
        description: skill.manifest().description().to_owned(),
        instructions_chars: skill
            .manifest()
            .instructions_chars()
            .try_into()
            .unwrap_or(u32::MAX),
        state,
    }
}

fn unknown_skill() -> AppError {
    AppError::new(
        "AGENT_SKILL_UNKNOWN",
        ErrorKind::NotFound,
        "没有这项内置技能",
        false,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::Mutex;

    const DOCUMENT: &str = "---\nname: haven-agent-proposal\ndescription: 读取脱敏上下文并创建待批准提案\n---\n\n# 标题\n\n正文。\n";
    /// `DOCUMENT` 里 frontmatter 之后的那一段——真正会进请求正文的字节。
    const DOCUMENT_BODY: &str = "\n# 标题\n\n正文。\n";
    const SECOND_DOCUMENT: &str =
        "---\nname: other-skill\ndescription: 第二项内置技能\n---\n\n# 另一份\n";

    /// 只读目录的测试替身。
    pub(crate) struct FakeRegistry {
        skills: Vec<BuiltinAgentSkill>,
    }

    impl FakeRegistry {
        pub(crate) fn new(documents: &[&str]) -> Self {
            Self {
                skills: documents
                    .iter()
                    .map(|document| {
                        BuiltinAgentSkill::from_document(document).expect("测试文档必须合法")
                    })
                    .collect(),
            }
        }
    }

    impl AgentSkillRegistryPort for FakeRegistry {
        fn builtin_skills(&self) -> Vec<&BuiltinAgentSkill> {
            self.skills.iter().collect()
        }

        fn find(&self, skill_id: &AgentSkillId) -> Option<&BuiltinAgentSkill> {
            self.skills.iter().find(|skill| skill.id() == skill_id)
        }
    }

    /// 内存状态仓储的测试替身。写入语义与 SQLite 实现一致：覆盖，不追加。
    #[derive(Default)]
    pub(crate) struct FakeStates {
        pub(crate) rows: Mutex<Vec<AgentSkillEnablement>>,
        pub(crate) writes: Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl AgentSkillStateRepository for FakeStates {
        async fn list(&self) -> Result<Vec<AgentSkillEnablement>, AppError> {
            Ok(self.rows.lock().unwrap().clone())
        }

        async fn get(
            &self,
            skill_id: &AgentSkillId,
        ) -> Result<Option<AgentSkillEnablement>, AppError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|record| record.skill_id() == skill_id)
                .cloned())
        }

        async fn put(
            &self,
            enablement: &AgentSkillEnablement,
            _updated_at_ms: i64,
        ) -> Result<(), AppError> {
            *self.writes.lock().unwrap() += 1;
            let mut rows = self.rows.lock().unwrap();
            rows.retain(|record| record.skill_id() != enablement.skill_id());
            rows.push(enablement.clone());
            Ok(())
        }
    }

    pub(crate) fn service(documents: &[&str]) -> (AgentSkillService, Arc<FakeStates>) {
        let states = Arc::new(FakeStates::default());
        let registry = Arc::new(FakeRegistry::new(documents));
        let registry: Arc<dyn AgentSkillRegistryPort> = registry;
        let states_port: Arc<dyn AgentSkillStateRepository> = states.clone();
        (AgentSkillService::new(registry, states_port), states)
    }

    #[tokio::test]
    async fn list_reports_disabled_without_any_record() {
        let (service, _) = service(&[DOCUMENT]);
        let listed = service.list().await.unwrap();
        assert_eq!(listed.schema_version, 1);
        assert_eq!(listed.skills.len(), 1);
        assert_eq!(listed.skills[0].skill_id, "haven-agent-proposal");
        assert_eq!(listed.skills[0].state, AgentSkillActivationDto::Disabled);
        assert!(listed.skills[0].instructions_chars > 0);
    }

    #[tokio::test]
    async fn enabling_persists_and_becomes_active() {
        let (service, _) = service(&[DOCUMENT]);
        let enabled = service
            .set_enabled(&AgentSkillSetEnabledRequest {
                skill_id: "haven-agent-proposal".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(enabled.state, AgentSkillActivationDto::Enabled);

        // 重新读取（模拟重启后的一次全新读取）必须看到同一件事。
        let listed = service.list().await.unwrap();
        assert_eq!(listed.skills[0].state, AgentSkillActivationDto::Enabled);

        let active = service.active_skills().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id().as_str(), "haven-agent-proposal");
        // 生效的是**正文**（frontmatter 之后的那一段），不是整份文件：元数据是给外部 Agent
        // 路由用的说明，原生模型读不到它，它也不参与启用摘要。
        assert_eq!(active[0].instructions().as_str(), DOCUMENT_BODY);
    }

    #[tokio::test]
    async fn disabling_writes_a_record_and_stops_consumption() {
        let (service, states) = service(&[DOCUMENT]);
        let request = AgentSkillSetEnabledRequest {
            skill_id: "haven-agent-proposal".into(),
            enabled: true,
        };
        service.set_enabled(&request).await.unwrap();
        let disabled = service
            .set_enabled(&AgentSkillSetEnabledRequest {
                enabled: false,
                ..request
            })
            .await
            .unwrap();
        assert_eq!(disabled.state, AgentSkillActivationDto::Disabled);
        assert!(service.active_skills().await.unwrap().is_empty());
        // 禁用是记录而不是删除：行还在，因此"曾经启用过"仍是可读事实。
        assert_eq!(states.rows.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_changed_document_turns_an_enabled_record_stale_and_never_consumed() {
        let (service, states) = service(&[DOCUMENT]);
        service
            .set_enabled(&AgentSkillSetEnabledRequest {
                skill_id: "haven-agent-proposal".into(),
                enabled: true,
            })
            .await
            .unwrap();

        // 模拟"应用升级后技能正文变了"：同一 id，另一份内容。
        let upgraded = "---\nname: haven-agent-proposal\ndescription: 读取脱敏上下文并创建待批准提案\n---\n\n# 标题\n\n正文改过了。\n";
        let registry: Arc<dyn AgentSkillRegistryPort> = Arc::new(FakeRegistry::new(&[upgraded]));
        let states_port: Arc<dyn AgentSkillStateRepository> = states.clone();
        let upgraded_service = AgentSkillService::new(registry, states_port);

        let listed = upgraded_service.list().await.unwrap();
        assert_eq!(listed.skills[0].state, AgentSkillActivationDto::Stale);
        assert!(
            upgraded_service.active_skills().await.unwrap().is_empty(),
            "stale 的技能不得被消费"
        );

        // 再次启用 = 对**新**内容的重新确认。
        let reconfirmed = upgraded_service
            .set_enabled(&AgentSkillSetEnabledRequest {
                skill_id: "haven-agent-proposal".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(reconfirmed.state, AgentSkillActivationDto::Enabled);
        let active = upgraded_service.active_skills().await.unwrap();
        assert_eq!(
            active[0].instructions().as_str(),
            "\n# 标题\n\n正文改过了。\n"
        );
    }

    #[tokio::test]
    async fn unknown_skill_is_rejected_without_writing() {
        let (service, states) = service(&[DOCUMENT]);
        let error = service
            .set_enabled(&AgentSkillSetEnabledRequest {
                skill_id: "not-a-builtin".into(),
                enabled: true,
            })
            .await
            .expect_err("未知技能必须被拒绝");
        assert_eq!(error.code().as_str(), "AGENT_SKILL_UNKNOWN");
        assert_eq!(*states.writes.lock().unwrap(), 0, "拒绝路径必须零写入");
    }

    #[tokio::test]
    async fn a_malformed_skill_id_is_a_validation_error_not_a_lookup() {
        let (service, states) = service(&[DOCUMENT]);
        let error = service
            .set_enabled(&AgentSkillSetEnabledRequest {
                skill_id: "Not A Slug".into(),
                enabled: true,
            })
            .await
            .expect_err("非法 id 必须被拒绝");
        assert_eq!(error.code().as_str(), "AGENT_SKILL_ID_INVALID");
        assert_eq!(*states.writes.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn every_enabled_skill_is_returned_in_a_stable_order() {
        let (service, _) = service(&[SECOND_DOCUMENT, DOCUMENT]);
        for skill_id in ["haven-agent-proposal", "other-skill"] {
            service
                .set_enabled(&AgentSkillSetEnabledRequest {
                    skill_id: skill_id.into(),
                    enabled: true,
                })
                .await
                .unwrap();
        }
        let active = service.active_skills().await.unwrap();
        let ids: Vec<&str> = active.iter().map(|skill| skill.id().as_str()).collect();
        // 顺序来自目录端口，不来自数据库行序，因此与启用顺序无关。
        assert_eq!(ids, vec!["other-skill", "haven-agent-proposal"]);
    }
}
