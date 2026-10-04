//! 随应用分发的内置 Agent Skill 目录（原生 Skill 运行时）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §6。
//!
//! **"内置"在这里是编译期事实，不是运行时约定。** 技能正文用 `include_str!` 嵌入二进制，
//! 因此：
//!
//! - 它随每个 MSI / NSIS / 可执行文件一起分发，不依赖 `tauri.conf.json` 的
//!   `bundle.resources`，也不存在"装完之后资源目录里少了文件"的失败模式；
//! - 它不可能被运行时的目录内容替换——没有环境变量、没有搜索路径、没有发现文件；
//! - 它仍然受源码树约束：`embedded_documents_are_exactly_the_skill_packages_in_the_source_tree`
//!   与 `embedded_bytes_are_byte_identical_to_the_source_tree` 两个用例把"嵌入的到底是什么"
//!   钉在 `skills/` 目录上，因此"改了技能文档却忘了它是内嵌的"会在测试里立刻暴露。
//!
//! 这个模块**不读用户可写目录**，因此也不提供任何"按路径加载技能"的能力——
//! 那种能力等价于一个任意文件读取入口。

use haven_application::services::agent_skill::AgentSkillRegistryPort;
use haven_common::{AppError, ErrorKind};
use haven_domain::agent_skill::{AgentSkillId, BuiltinAgentSkill};

/// 内置技能文档表：`(源码树相对路径, 嵌入正文)`。
///
/// 路径字符串同时是**测试的锚点**（用来回读源码树比对字节），因此它不是装饰性的
/// 注释——改这里就必须改 `skills/` 下对应的真实文件。
const BUILTIN_SKILL_DOCUMENTS: &[(&str, &str)] = &[(
    "skills/haven-agent-proposal/SKILL.md",
    include_str!("../../../../skills/haven-agent-proposal/SKILL.md"),
)];

/// 内置技能的数量。
///
/// 公开它的理由只有一个：出站请求的字节预算必须**由"最多可能启用多少技能"推导**，
/// 而不是写一个凭感觉的固定数。注册表是唯一能产出 `ActiveAgentSkill` 的地方
/// （`active_skills()` 遍历的就是这张表），因此这里的长度就是那个"最多"。
/// 加了第二项技能却忘了同步预算，会由 `ai_provider` 的预算断言用例直接抓住。
pub const BUILTIN_SKILL_COUNT: usize = BUILTIN_SKILL_DOCUMENTS.len();

/// 内置技能目录。
///
/// 构造时解析全部文档：frontmatter 不合法的技能**不会**被静默跳过，
/// 而是让构造失败。一个"看起来加载成功、实际少了一项技能"的目录会让用户
/// 在界面上找不到他以为存在的东西。
pub struct BuiltinAgentSkillRegistry {
    skills: Vec<BuiltinAgentSkill>,
}

impl BuiltinAgentSkillRegistry {
    pub fn new() -> Result<Self, AppError> {
        let mut skills = Vec::with_capacity(BUILTIN_SKILL_DOCUMENTS.len());
        for (source, document) in BUILTIN_SKILL_DOCUMENTS {
            let skill = BuiltinAgentSkill::from_document(document).map_err(|error| {
                AppError::new(
                    "AGENT_SKILL_REGISTRY_INVALID",
                    ErrorKind::Internal,
                    format!("内置技能文档 {source} 不合法"),
                    false,
                )
                .with_source(error)
            })?;
            if skills
                .iter()
                .any(|existing: &BuiltinAgentSkill| existing.id() == skill.id())
            {
                return Err(AppError::new(
                    "AGENT_SKILL_REGISTRY_INVALID",
                    ErrorKind::Internal,
                    format!("内置技能 id 重复：{source}"),
                    false,
                ));
            }
            skills.push(skill);
        }
        // 顺序来自目录本身，与文件系统枚举顺序无关：请求拼装的顺序必须可复现。
        skills.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(Self { skills })
    }

    /// 内置技能数量。用于启动期自检与诊断，不用于"有没有技能"以外任何判断。
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

impl AgentSkillRegistryPort for BuiltinAgentSkillRegistry {
    fn builtin_skills(&self) -> Vec<&BuiltinAgentSkill> {
        self.skills.iter().collect()
    }

    fn find(&self, skill_id: &AgentSkillId) -> Option<&BuiltinAgentSkill> {
        self.skills.iter().find(|skill| skill.id() == skill_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};

    /// 仓库根的 `skills/` 目录（相对 crate 根：`后端/crates/haven-infrastructure`）。
    fn skills_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../skills")
    }

    fn registry() -> BuiltinAgentSkillRegistry {
        BuiltinAgentSkillRegistry::new().expect("内置技能必须可加载")
    }

    #[test]
    fn the_builtin_catalog_is_exactly_the_skill_packages_in_the_source_tree() {
        let mut on_disk: Vec<String> = std::fs::read_dir(skills_root())
            .expect("skills/ 目录必须存在")
            .map(|entry| entry.expect("目录项必须可读"))
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();

        let mut embedded: Vec<String> = BUILTIN_SKILL_DOCUMENTS
            .iter()
            .map(|(source, _)| {
                let mut parts = source.split('/');
                assert_eq!(parts.next(), Some("skills"), "路径必须以 skills/ 开头");
                let name = parts.next().expect("路径必须含技能目录名").to_owned();
                assert_eq!(parts.next(), Some("SKILL.md"), "技能入口必须是 SKILL.md");
                assert_eq!(parts.next(), None, "路径不得更深");
                name
            })
            .collect();
        embedded.sort();

        assert_eq!(
            embedded, on_disk,
            "内置技能集合必须与 skills/ 下的技能包一一对应：新增技能包要同时登记，\
             删掉技能包要同时从内置表移除"
        );
    }

    /// 打包保真：编译进二进制的字节必须与源码树里那份**逐字节相同**。
    ///
    /// 这是"外部 Agent 拿到的技能"与"栖阅内嵌的技能"同源这件事在本仓库里的可执行证据。
    #[test]
    fn embedded_bytes_are_byte_identical_to_the_source_tree() {
        for (source, document) in BUILTIN_SKILL_DOCUMENTS {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(source);
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("读取 {source} 失败：{error}"));
            assert_eq!(
                on_disk, *document,
                "{source} 的内嵌内容与源码树不一致——技能正文改了但没重新构建"
            );
        }
    }

    #[test]
    fn the_registry_exposes_the_first_builtin_skill_and_its_provenance() {
        let registry = registry();
        assert_eq!(registry.len(), BUILTIN_SKILL_DOCUMENTS.len());
        assert!(!registry.is_empty());

        let id = AgentSkillId::parse("haven-agent-proposal").expect("id 必须合法");
        let skill = registry.find(&id).expect("首项内置技能必须存在");
        assert_eq!(skill.id(), &id);
        // 描述来自 frontmatter，不是这里另抄一份。
        assert!(!skill.manifest().description().is_empty());
        assert!(skill.manifest().instructions_chars() > 0);
        assert_eq!(skill.manifest().instructions_hash().len(), 64);
    }

    #[test]
    fn an_unknown_id_is_not_resolved_to_the_first_skill() {
        let registry = registry();
        let id = AgentSkillId::parse("not-a-builtin").expect("id 形状合法");
        assert!(registry.find(&id).is_none());
    }

    /// 真实技能文件的投影：随应用分发的那份 `SKILL.md` 里，MCP 工作流属于**外部**受众，
    /// 因此不得出现在栖阅自己会送出去的正文里。
    ///
    /// 这是"原生模型不会被指使去调用它没有的工具"在真实产物上的证据——单元测试里的
    /// 合成文档只能证明投影函数会工作，证明不了**这一份**技能文档的标记划对了地方。
    #[test]
    fn the_shipped_skill_native_projection_excludes_the_mcp_workflow() {
        let registry = registry();
        let id = AgentSkillId::parse("haven-agent-proposal").expect("id 必须合法");
        let skill = registry.find(&id).expect("首项内置技能必须存在");
        let native = skill.instructions().as_str();

        // 共用段落与原生段落必须在。
        assert!(
            native.contains("写设置永远只有一条路"),
            "共用权限边界不得被裁掉"
        );
        assert!(native.contains("你没有工具"), "原生段落必须在");

        // 外部受众的内容整段不在：这些是 MCP 工具名、参考文件与工作流标题。
        for external_only in [
            "get_system_capabilities",
            "get_settings_snapshot",
            "propose_settings_patch",
            "references/mcp-tool-map.md",
            "### 工作流",
        ] {
            assert!(
                !native.contains(external_only),
                "原生投影里不应出现外部段落的内容：{external_only}"
            );
        }
        assert!(!native.contains("haven:audience"), "受众标记不进正文");

        // 投影必须真的比原文短，否则说明标记根本没生效（而上面的断言会一起变空转）。
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../skills/haven-agent-proposal/SKILL.md");
        let canonical = std::fs::read_to_string(&shipped).expect("技能文档必须可读");
        assert!(
            native.len() < canonical.len(),
            "原生投影（{} 字节）必须短于原文（{} 字节）",
            native.len(),
            canonical.len()
        );
    }

    /// frontmatter 是给**外部**客户端路由用的元数据，原生投影里一个字都不该有。
    ///
    /// 真实产物上的证据，而不是合成文档上的：这一份 `description` 明说"通过栖阅（Haven）的
    /// MCP server …"，把它送进一个没有工具的模型的 system 消息，等于邀请它编造工具调用；
    /// 反过来，只改这句描述也不该作废用户的启用记录——所以摘要同样不能覆盖它。
    #[test]
    fn the_shipped_skill_native_projection_excludes_the_frontmatter_metadata() {
        let registry = registry();
        let id = AgentSkillId::parse("haven-agent-proposal").expect("id 必须合法");
        let skill = registry.find(&id).expect("首项内置技能必须存在");
        let native = skill.instructions().as_str();

        assert!(
            !native.starts_with("---"),
            "原生投影不得以 frontmatter 分隔行开头：{native:?}"
        );
        for metadata_only in [
            "name: haven-agent-proposal",
            "description:",
            "通过栖阅（Haven）的 MCP server",
            "在以下情况使用",
            "即使用户没有说出技能名称",
        ] {
            assert!(
                !native.contains(metadata_only),
                "frontmatter 元数据不得进入原生投影：{metadata_only}"
            );
        }

        // 界面上的"注入约 N 字"与摘要描述的都是这一段字节。
        assert_eq!(
            skill.manifest().instructions_chars(),
            native.chars().count()
        );
        assert_eq!(skill.manifest().instructions_hash().len(), 64);
    }

    #[test]
    fn ids_are_unique_and_the_order_is_stable() {
        let first = registry();
        let second = registry();
        let first_ids: Vec<&str> = first
            .builtin_skills()
            .iter()
            .map(|skill| skill.id().as_str())
            .collect();
        let second_ids: Vec<&str> = second
            .builtin_skills()
            .iter()
            .map(|skill| skill.id().as_str())
            .collect();
        assert_eq!(first_ids, second_ids);
        let mut sorted = first_ids.clone();
        sorted.sort_unstable();
        assert_eq!(first_ids, sorted);
    }
}
