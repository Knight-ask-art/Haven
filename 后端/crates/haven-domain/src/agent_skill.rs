//! 内置 Agent Skill 的领域契约（原生 Skill 运行时）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §6。
//!
//! 三条不变量，任何调用方措辞都不能改变：
//!
//! 1. **技能正文不是调用方参数。** 正文只来自随应用分发的内容；IPC 只能引用
//!    `skill_id`。不存在"把任意提示词当作技能提交"的入口——那才是自由调用入口，
//!    被 §2 明确禁止。
//! 2. **启用状态与技能内容摘要绑定。** 一条启用记录只有在 `instructions_hash`
//!    与当前分发内容一致时才生效。内容变了而记录还是旧的，该记录就是 `stale`：
//!    **不生效**，也不静默沿用（fail closed），必须由用户重新确认。
//! 3. **启用技能不提升权限。** 它只改变模型请求里的说明性上下文；不新增工具、
//!    不改变能力清单、不产生 Apply 路径。技能的产出仍然只能是 pending Proposal。

use std::fmt;

use haven_common::{AppError, ErrorKind};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::setting_proposal::is_canonical_digest;

/// 受众标记的前缀/闭合形式。**语法是闭合的**：未知标记一律报错，不忽略。
///
/// 一份技能文档同时面向两条运行时，但两条运行时能做的事不同：原生运行时只有
/// 「结构化输出 + 待批准提案」，外部 Agent 才有 MCP 工具。把 MCP 工作流原样交给
/// 原生模型，等于让它去调用它根本不存在的工具。因此文档里用标记划分段落，
/// 由 [`AgentSkillInstructions::project`] 做**确定性**投影。
const AUDIENCE_OPEN_PREFIX: &str = "<!-- haven:audience=";
const AUDIENCE_OPEN_SUFFIX: &str = "-->";
const AUDIENCE_CLOSE: &str = "<!-- /haven:audience -->";

/// 两种规范标记共有的子串，以及所有 `haven` 命名空间标记的公共前缀。
///
/// 用来判断"这一行**看起来**想写标记"：命中就必须是上面两种规范形式之一。
const AUDIENCE_MARKER_TOKEN: &str = "haven:audience";
const HAVEN_MARKER_PREFIX: &str = "<!-- haven:";

/// 技能正文面向的运行时。闭合两态：没有"两者皆可"的第三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSkillAudience {
    /// 栖阅自己（编译期嵌入，只有结构化输出，没有工具）。
    Native,
    /// 外部 Agent（通过 MCP 拿到冻结的 9 个只读 / 提案工具）。
    External,
}

impl AgentSkillAudience {
    fn marker_value(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::External => "external",
        }
    }

    /// 该段落是否属于本受众。
    fn admits(self, region: Self) -> bool {
        self == region
    }
}

/// `skill_id` 的最大字符数（与 `tools/skills/skill-contract-check.py` 的 `MAX_NAME_CHARS` 同值）。
pub const AGENT_SKILL_ID_MAX_CHARS: usize = 64;
/// 技能描述的字符上限（与契约校验器的 `MAX_DESCRIPTION_CHARS` 同值）。
pub const AGENT_SKILL_DESCRIPTION_MAX_CHARS: usize = 1024;
/// 注入模型请求的技能正文上限（字符，不是字节）。
///
/// 正文会被拼进 Provider 的 system 提示词，因此必须有界：无界正文等于给了一条
/// 不受限的提示词通道。当前内置技能约 6 千字符，上限留出余量但仍远小于
/// `AGENT_CONTEXT_MAX_TOTAL_CHARS` 之外的任何载荷。
pub const AGENT_SKILL_INSTRUCTIONS_MAX_CHARS: usize = 24_000;

/// 技能内容摘要的 SHA-256 绑定域前缀（参与 preimage，避免与其它摘要用途混淆）。
const AGENT_SKILL_HASH_DOMAIN: &str = "haven:agent-skill:v1";

/// Markdown frontmatter 分隔行。
const FRONTMATTER_DELIMITER: &str = "---";

/// frontmatter 允许出现的键。**恰好两个**：多一个键就拒绝，不忽略。
///
/// 与 `tools/skills/skill-contract-check.py` 的 `ALLOWED_FRONTMATTER_KEYS` 同值。
/// 运行时与契约校验器对"什么是合法技能文档"必须给同一个答案，否则会出现
/// "契约校验通过、运行时拒绝加载"的分裂。
const ALLOWED_FRONTMATTER_KEYS: [&str; 2] = ["name", "description"];

/// 内置技能的稳定标识（hyphen-case slug）。
///
/// 形状与契约校验器一致：`^[a-z0-9-]+$`、无首尾 `-`、无连续 `--`、≤64 字符。
/// 拒绝比接受更重要：宽松的 id 会让"启用 A"实际上启用了一个别的文档。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentSkillId(String);

impl AgentSkillId {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        if raw.is_empty() {
            return Err(invalid_id("技能 id 不能为空"));
        }
        if raw.chars().count() > AGENT_SKILL_ID_MAX_CHARS {
            return Err(invalid_id("技能 id 超长"));
        }
        if raw.starts_with('-') || raw.ends_with('-') || raw.contains("--") {
            return Err(invalid_id("技能 id 不能以 - 开头/结尾，也不能包含连续 --"));
        }
        if !raw
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(invalid_id("技能 id 只允许小写字母、数字与连字符"));
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentSkillId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 一段已校验的技能正文。
///
/// 它既可以是 `SKILL.md` 的正文（frontmatter **之后**的那一段），也可以是
/// [`Self::project`] 产出的受众投影——两者是同一个类型，因为对下游（system 消息拼装）
/// 来说都只是"一段已校验的说明性文字"。frontmatter 从不进入这个类型：它是给外部 Agent
/// 路由用的元数据，不是任何一条运行时读到的说明。
///
/// 校验的是"能不能安全地进入提示词"：有界、无控制字符（`\n` / `\t` 除外）。
/// 不校验内容语义——语义正确性由 `tools/skills/skill-contract-check.py` 负责。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillInstructions(String);

impl AgentSkillInstructions {
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        if raw.trim().is_empty() {
            return Err(invalid_document("技能正文为空"));
        }
        if raw.chars().count() > AGENT_SKILL_INSTRUCTIONS_MAX_CHARS {
            return Err(invalid_document("技能正文超出上限"));
        }
        if has_forbidden_control_chars(raw) {
            return Err(invalid_document("技能正文包含控制字符"));
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }

    /// 投影出**该受众真正会读到**的那一份正文。
    ///
    /// 规则是确定性的，且只有三条：
    ///
    /// 1. 标记之外的段落是两条路径共用的，原样保留；
    /// 2. `<!-- haven:audience=native -->` … `<!-- /haven:audience -->` 之间的段落只进原生投影，
    ///    `=external` 之间的只进外部投影；
    /// 3. 标记行本身不出现在任何投影里。
    ///
    /// 解析失败（未知受众名、未闭合、嵌套、多余闭合，以及任何"看起来像标记但不是规范形式"
    /// 的行）一律**报错**而不是"尽量保留"：一个解析失败的文档如果被宽松处理，最可能的
    /// 结果是把 MCP 工作流送进原生请求，而那正是本机制要防的事。
    ///
    /// 行尾**原样保留**：摘要仍然对原始字节敏感，因此把技能文件从 LF 改成 CRLF 是一次
    /// 内容变化（而不是被静默抹平），会照常触发"需要重新确认"。
    pub fn project(&self, audience: AgentSkillAudience) -> Result<Self, AppError> {
        let mut out = String::with_capacity(self.0.len());
        let mut open: Option<AgentSkillAudience> = None;
        for raw_line in self.0.split_inclusive('\n') {
            // 标记**识别**时做 trim：一行缩进的 `<!-- haven:audience=external -->` 如果被
            // 当成普通正文，它后面的整段外部内容就会静默漏进原生投影——那正是本机制
            // 要防的事。宁可把"看起来像标记"的行一律按标记处理。
            let line = strip_line_ending(raw_line).trim();
            if let Some(region) = parse_audience_open(line)? {
                if open.is_some() {
                    return Err(invalid_document("受众标记不能嵌套"));
                }
                open = Some(region);
                continue;
            }
            if line == AUDIENCE_CLOSE {
                if open.take().is_none() {
                    return Err(invalid_document("受众标记闭合行没有对应的开始行"));
                }
                continue;
            }
            if looks_like_marker(line) {
                return Err(invalid_document("技能文档含非规范或不支持的受众标记"));
            }
            match open {
                Some(region) if !audience.admits(region) => continue,
                _ => {}
            }
            out.push_str(raw_line);
        }
        if open.is_some() {
            return Err(invalid_document("受众标记没有闭合"));
        }
        // 分隔行本身不构成正文：只保留标记的文档等于空文档，而空文档不能进提示词。
        if out.trim().is_empty() {
            return Err(invalid_document("投影后的技能正文为空"));
        }
        Self::parse(&out)
    }

    /// 原生运行时的投影（`Skill` 内容与栖阅自己实际拥有的能力对齐）。
    pub fn native_projection(&self) -> Result<Self, AppError> {
        self.project(AgentSkillAudience::Native)
    }

    /// 外部运行时的投影：保留 MCP 工作流，摘掉只属于原生运行时的段落与所有标记行。
    ///
    /// 生产路径不调用它——外部 Agent 读的是源码树/zip 里的**整份**文档（打包器只做字节
    /// 搬运）。保留这个方法是为了让"两条运行时的正文到底差在哪"能被断言，而不是靠读文档。
    pub fn external_projection(&self) -> Result<Self, AppError> {
        self.project(AgentSkillAudience::External)
    }

    /// 内容摘要：64 位小写十六进制，带绑定域前缀。
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(AGENT_SKILL_HASH_DOMAIN.as_bytes());
        hasher.update(b"\n");
        hasher.update(self.0.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// 一项内置技能的可分发元数据。**不含正文**，因此可以安全地进 wire。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillManifest {
    id: AgentSkillId,
    description: String,
    instructions_hash: String,
    instructions_chars: usize,
}

impl AgentSkillManifest {
    pub fn id(&self) -> &AgentSkillId {
        &self.id
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// 当前分发内容的摘要。UI 用它判断一条启用记录是不是 `stale`。
    pub fn instructions_hash(&self) -> &str {
        &self.instructions_hash
    }

    pub fn instructions_chars(&self) -> usize {
        self.instructions_chars
    }
}

/// 一项随应用分发的内置技能：元数据 + **frontmatter 之后的正文在原生受众下的投影**。
///
/// 注意 `instructions` 既不是 `SKILL.md` 的原文，也不是它的正文原文，而是正文在原生受众
/// 下的投影（见 [`AgentSkillInstructions::project`]）。这是刻意的：启用记录里绑定的摘要、
/// 界面显示的"注入约 N 字"、以及最终进 Provider 请求的文字，三者必须是**同一份**内容；
/// 如果这里存原文而请求里拼投影，摘要就会去证明一段没人读过的文字。frontmatter 同样
/// 不参与——它既不是原生模型读到的文字，也不是用户同意的那段内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinAgentSkill {
    manifest: AgentSkillManifest,
    instructions: AgentSkillInstructions,
}

impl BuiltinAgentSkill {
    /// 从一份 `SKILL.md` 文档构造。id 与描述**只来自 frontmatter**，
    /// 不来自调用方传入的第二个参数——否则同一份正文可以被打上任意 id。
    ///
    /// **正文只取 frontmatter 之后的那一段。** frontmatter 里的 `description` 是给*外部*
    /// Agent 路由用的元数据（这一份明说"通过栖阅（Haven）的 MCP server …"），它不是任何一条
    /// 运行时读到的说明文字：进了原生投影就会把 MCP 工具语义塞给一个没有工具的模型，还会
    /// 让"只改一句元数据描述"作废用户的启用记录——而真正送出去的字节一个都没变。
    ///
    /// 标记解析失败即整体失败：一份受众划分不明的文档宁可拒绝加载，也不能让
    /// "原生请求里混进 MCP 工作流"这种错误以静默方式发生。
    pub fn from_document(markdown: &str) -> Result<Self, AppError> {
        let (id, description, body) = split_skill_document(markdown)?;
        let instructions = AgentSkillInstructions::parse(body)?.native_projection()?;
        let manifest = AgentSkillManifest {
            id,
            description,
            instructions_hash: instructions.content_hash(),
            instructions_chars: instructions.char_count(),
        };
        Ok(Self {
            manifest,
            instructions,
        })
    }

    pub fn manifest(&self) -> &AgentSkillManifest {
        &self.manifest
    }

    pub fn id(&self) -> &AgentSkillId {
        &self.manifest.id
    }

    pub fn instructions(&self) -> &AgentSkillInstructions {
        &self.instructions
    }
}

/// 一条持久化的启用记录。
///
/// `instructions_hash` 是**用户启用当时**看到的那份内容的摘要。它存在的唯一理由
/// 是让"分发内容被替换"这件事可被检测：摘要不一致即视为未启用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillEnablement {
    skill_id: AgentSkillId,
    instructions_hash: String,
    enabled: bool,
}

impl AgentSkillEnablement {
    pub fn parse(
        skill_id: AgentSkillId,
        instructions_hash: &str,
        enabled: bool,
    ) -> Result<Self, AppError> {
        if !is_canonical_digest(instructions_hash) {
            return Err(invalid_state("启用记录的内容摘要不是闭合摘要"));
        }
        Ok(Self {
            skill_id,
            instructions_hash: instructions_hash.to_owned(),
            enabled,
        })
    }

    pub fn skill_id(&self) -> &AgentSkillId {
        &self.skill_id
    }

    pub fn instructions_hash(&self) -> &str {
        &self.instructions_hash
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 内容摘要与当前分发内容不一致。**只有它为假时才允许生效。**
    pub fn is_stale_for(&self, skill: &BuiltinAgentSkill) -> bool {
        self.instructions_hash != skill.manifest().instructions_hash()
    }

    /// 该记录是否真的让这项技能生效：启用 **且** 摘要一致。
    pub fn is_active_for(&self, skill: &BuiltinAgentSkill) -> bool {
        self.enabled && !self.is_stale_for(skill)
    }
}

/// 一次模型请求实际携带的技能说明。
///
/// 这是**唯一**允许进入 Provider 请求的技能载荷类型：它只能由
/// [`resolve_active_skill`] 从"内置技能 + 权威启用记录"推导出来，不能由调用方构造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgentSkill {
    id: AgentSkillId,
    instructions: AgentSkillInstructions,
}

impl ActiveAgentSkill {
    pub fn id(&self) -> &AgentSkillId {
        &self.id
    }

    pub fn instructions(&self) -> &AgentSkillInstructions {
        &self.instructions
    }
}

/// 从"内置技能 + 该技能的持久化记录"推导本次请求要携带的技能说明。
///
/// 返回 `None` 是**正常结果**，表示不携带任何技能上下文。三种情况都返回 `None`：
/// 没有记录、记录未启用、记录已 `stale`。三者对请求路径是同一个结果（fail closed），
/// 但 UI 需要区分它们，因此区分放在 `AgentSkillStateDto` 而不是这里。
pub fn resolve_active_skill(
    skill: &BuiltinAgentSkill,
    enablement: Option<&AgentSkillEnablement>,
) -> Option<ActiveAgentSkill> {
    let enablement = enablement?;
    if !enablement.enabled() {
        return None;
    }
    if enablement.skill_id() != skill.id() {
        return None;
    }
    if enablement.is_stale_for(skill) {
        return None;
    }
    Some(ActiveAgentSkill {
        id: skill.id().clone(),
        instructions: skill.instructions().clone(),
    })
}

/// 从一份 `SKILL.md` 文档解析 frontmatter，返回 `(id, description)`。
///
/// 解析是**闭合**的：未知键、缺键、重复键、多行值、空值、`<` / `>` 一律拒绝。
/// 与契约校验器同规则；运行时不接受"契约校验器会拒绝"的文档。
pub fn parse_skill_document(markdown: &str) -> Result<(AgentSkillId, String), AppError> {
    let (id, description, _) = split_skill_document(markdown)?;
    Ok((id, description))
}

/// frontmatter 的完整解析结果：`(id, description, frontmatter 之后的正文切片)`。
///
/// 解析与"正文从哪里开始"必须是**同一次**遍历的结果：分成两套行切分规则，迟早会对
/// 同一份文档给出两个不同的正文起点，而正文起点错了就是元数据漏进提示词或正文被截掉。
///
/// 第三个元素是原始字节切片，行尾原样保留（CRLF 不会被抹平），因此摘要仍然对原始字节敏感；
/// 它也是**唯一**允许进入 [`AgentSkillInstructions`] 的内容，见 [`BuiltinAgentSkill::from_document`]。
fn split_skill_document(markdown: &str) -> Result<(AgentSkillId, String, &str), AppError> {
    let mut lines = markdown.split_inclusive('\n');
    let mut body_start = 0usize;

    let first_line = lines.next().unwrap_or_default();
    body_start += first_line.len();
    if strip_line_ending(first_line) != FRONTMATTER_DELIMITER {
        return Err(invalid_document("技能文档必须以 frontmatter 分隔行开头"));
    }

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut closed = false;
    for raw_line in lines {
        body_start += raw_line.len();
        let line = strip_line_ending(raw_line);
        if line == FRONTMATTER_DELIMITER {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(invalid_document("frontmatter 行缺少键值分隔符"));
        };
        let key = key.trim();
        let value = value.trim();
        if !ALLOWED_FRONTMATTER_KEYS.contains(&key) {
            return Err(invalid_document("frontmatter 含未允许的键"));
        }
        if value.is_empty() {
            return Err(invalid_document("frontmatter 的值不能为空"));
        }
        if value.contains('<') || value.contains('>') {
            return Err(invalid_document("frontmatter 的值不能包含尖括号"));
        }
        if value.chars().any(is_forbidden_control_char) {
            return Err(invalid_document("frontmatter 的值包含控制字符"));
        }
        let slot = match key {
            "name" => &mut name,
            _ => &mut description,
        };
        if slot.is_some() {
            return Err(invalid_document("frontmatter 出现重复键"));
        }
        *slot = Some(value.to_owned());
    }
    if !closed {
        return Err(invalid_document("frontmatter 没有闭合分隔行"));
    }
    let name = name.ok_or_else(|| invalid_document("frontmatter 缺少 name"))?;
    let description =
        description.ok_or_else(|| invalid_document("frontmatter 缺少 description"))?;
    if description.chars().count() > AGENT_SKILL_DESCRIPTION_MAX_CHARS {
        return Err(invalid_document("frontmatter 的 description 超长"));
    }
    Ok((
        AgentSkillId::parse(&name)?,
        description,
        &markdown[body_start..],
    ))
}

/// frontmatter 与正文共享的控制字符策略：允许 `\n` 与 `\t`，其余 C0 与 DEL 一律拒绝。
fn is_forbidden_control_char(character: char) -> bool {
    match character {
        '\n' | '\t' => false,
        _ => character.is_control(),
    }
}

/// 这一行是不是"看起来想写受众标记、但不是规范形式"的一行。
///
/// 只检查"以标记前缀开头"是不够的：一行**前置了说明文字**的标记
/// （`说明 <!-- haven:audience=external -->`）会以普通正文的身份进入投影，它后面的整段
/// 外部内容于是**没有开始行**——既不触发嵌套，也不会报"缺少闭合"，而是静默漏进原生请求。
/// 同样漏掉的还有 `<!--haven:audience=external-->`（`<!--` 后缺空格）、
/// `<!-- haven:audience = external -->`（等号两侧有空格）与带尾巴的闭合行。
fn looks_like_marker(line: &str) -> bool {
    line.contains(AUDIENCE_MARKER_TOKEN) || line.contains(HAVEN_MARKER_PREFIX)
}

/// 解析一行受众开始标记。不是标记行返回 `Ok(None)`；是标记行但受众名不认识则报错。
fn parse_audience_open(line: &str) -> Result<Option<AgentSkillAudience>, AppError> {
    let Some(rest) = line.strip_prefix(AUDIENCE_OPEN_PREFIX) else {
        return Ok(None);
    };
    let Some(value) = rest.strip_suffix(AUDIENCE_OPEN_SUFFIX) else {
        return Err(invalid_document("受众标记没有闭合的 -->"));
    };
    let value = value.trim();
    for audience in [AgentSkillAudience::Native, AgentSkillAudience::External] {
        if value == audience.marker_value() {
            return Ok(Some(audience));
        }
    }
    Err(invalid_document("受众标记的受众名不在闭合集合里"))
}

/// 保留 Windows CRLF 的原始字节用于摘要，同时只允许成对的 `\r\n`。
fn has_forbidden_control_chars(value: &str) -> bool {
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\n' | '\t' => {}
            '\r' if characters.peek() == Some(&'\n') => {}
            _ if character.is_control() => return true,
            _ => {}
        }
    }
    false
}

fn strip_carriage_return(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// 去掉行尾（`\n` 或 `\r\n`）。只用于**判断**这一行是不是标记——投影输出时写回的是
/// 原始行，因此行尾不会被投影悄悄抹平。
fn strip_line_ending(line: &str) -> &str {
    strip_carriage_return(line.strip_suffix('\n').unwrap_or(line))
}

fn invalid_id(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SKILL_ID_INVALID",
        ErrorKind::Validation,
        detail,
        false,
    )
}

fn invalid_document(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SKILL_DOCUMENT_INVALID",
        ErrorKind::Validation,
        detail,
        false,
    )
}

fn invalid_state(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SKILL_STATE_INVALID",
        ErrorKind::Validation,
        detail,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份形状合法的最小文档。`name` 必须与 id 规则一致。
    const DOCUMENT: &str = "---\nname: haven-agent-proposal\ndescription: 读取脱敏上下文并创建待批准提案\n---\n\n# 标题\n\n正文。\n";

    /// `DOCUMENT` 里 frontmatter 闭合行**之后**的那一段：`from_document` 唯一会投影、
    /// 计长、求摘要的内容。
    const DOCUMENT_BODY: &str = "\n# 标题\n\n正文。\n";

    fn skill() -> BuiltinAgentSkill {
        BuiltinAgentSkill::from_document(DOCUMENT).expect("最小文档必须可解析")
    }

    #[test]
    fn parses_frontmatter_and_derives_id_from_the_document() {
        assert!(
            DOCUMENT.ends_with(DOCUMENT_BODY),
            "前提：fixture 的正文常量就是元数据之后的那一段"
        );
        let skill = skill();
        assert_eq!(skill.id().as_str(), "haven-agent-proposal");
        assert_eq!(
            skill.manifest().description(),
            "读取脱敏上下文并创建待批准提案"
        );
        assert_eq!(skill.instructions().as_str(), DOCUMENT_BODY);
        assert!(is_canonical_digest(skill.manifest().instructions_hash()));
    }

    #[test]
    fn accepts_crlf_documents_without_changing_the_content_hash_contract() {
        let crlf = DOCUMENT.replace('\n', "\r\n");
        let skill = BuiltinAgentSkill::from_document(&crlf).expect("CRLF 文档同样可解析");
        // 正文摘要覆盖原始字节，因此 CRLF 与 LF 文档是**不同**内容，
        // 这正是"换了内容就要重新确认"的证据，而不是需要抹平的差异。
        assert_ne!(
            skill.manifest().instructions_hash(),
            BuiltinAgentSkill::from_document(DOCUMENT)
                .unwrap()
                .manifest()
                .instructions_hash()
        );
        // 行尾原样穿过元数据切分与受众投影：去掉 frontmatter 不等于顺手把正文也规整了。
        assert!(
            skill.instructions().as_str().contains("\r\n"),
            "正文的行尾必须原样保留"
        );
        assert_eq!(
            skill.manifest().instructions_chars(),
            DOCUMENT_BODY.replace('\n', "\r\n").chars().count()
        );
    }

    /// frontmatter 是给**外部** Agent 路由用的元数据，两条运行时都不该在正文里读到它。
    ///
    /// 它会进原生投影这件事本身就是缺陷，且有两个后果：`description` 是"MCP server …"的
    /// 触发说明，会被塞给一个没有工具的模型；只改一句元数据描述就会作废用户的启用记录，
    /// 而真正送出去的字节一个都没变。
    #[test]
    fn frontmatter_metadata_never_reaches_the_native_instructions() {
        let skill = skill();
        let instructions = skill.instructions().as_str();

        assert_eq!(instructions, DOCUMENT_BODY);
        assert!(
            !instructions.starts_with(FRONTMATTER_DELIMITER),
            "正文不得以 frontmatter 分隔行开头：{instructions:?}"
        );
        for metadata_only in [
            "name:",
            "description:",
            "读取脱敏上下文并创建待批准提案",
            "haven-agent-proposal",
        ] {
            assert!(
                !instructions.contains(metadata_only),
                "元数据不得进入原生正文：{metadata_only}"
            );
        }
        // 界面上的"注入约 N 字"与摘要描述的是同一份文字。
        assert_eq!(
            skill.manifest().instructions_chars(),
            DOCUMENT_BODY.chars().count()
        );
    }

    /// 只改元数据不该让用户的启用记录失效——送出去的文字没变。
    #[test]
    fn editing_only_the_frontmatter_does_not_stale_an_enablement_record() {
        let base = skill();
        let enablement = AgentSkillEnablement::parse(
            base.id().clone(),
            base.manifest().instructions_hash(),
            true,
        )
        .expect("启用记录必须可构造");

        let reworded = DOCUMENT.replace("读取脱敏上下文并创建待批准提案", "另一段元数据描述");
        let edited = BuiltinAgentSkill::from_document(&reworded).expect("元数据改动后仍须可解析");

        assert_eq!(edited.manifest().description(), "另一段元数据描述");
        assert_eq!(
            base.manifest().instructions_hash(),
            edited.manifest().instructions_hash(),
            "元数据不进摘要"
        );
        assert_eq!(base.instructions().as_str(), edited.instructions().as_str());
        assert!(
            enablement.is_active_for(&edited),
            "只改一句元数据描述不得让启用记录失效：原生模型读到的字节一个都没变"
        );
    }

    /// frontmatter 之后没有任何原生正文的文档不能进提示词。
    ///
    /// 元数据被摘掉之后这条路径才真正够得着：以前 frontmatter 自己就"凑"出了一份非空正文。
    #[test]
    fn a_document_with_no_native_body_is_rejected_by_the_constructor() {
        let only_external = "---\nname: haven-agent-proposal\ndescription: d\n---\n\n\
                             <!-- haven:audience=external -->\n只有外部段落。\n\
                             <!-- /haven:audience -->\n";
        assert!(
            BuiltinAgentSkill::from_document(only_external).is_err(),
            "没有原生正文的文档必须被拒绝，而不是拿元数据当正文"
        );

        let no_body = "---\nname: haven-agent-proposal\ndescription: d\n---\n";
        assert!(BuiltinAgentSkill::from_document(no_body).is_err());
    }

    #[test]
    fn rejects_documents_that_the_contract_checker_would_also_reject() {
        let cases: [(&str, &str); 8] = [
            ("no-frontmatter", "# 只有正文\n"),
            (
                "unclosed",
                "---\nname: haven-agent-proposal\ndescription: d\n",
            ),
            ("missing-name", "---\ndescription: d\n---\n"),
            (
                "missing-description",
                "---\nname: haven-agent-proposal\n---\n",
            ),
            (
                "extra-key",
                "---\nname: haven-agent-proposal\ndescription: d\nallowed-tools: x\n---\n",
            ),
            (
                "duplicate-key",
                "---\nname: a\ndescription: d\nname: b\n---\n",
            ),
            (
                "angle-bracket",
                "---\nname: haven-agent-proposal\ndescription: a<b\n---\n",
            ),
            ("uppercase-name", "---\nname: Haven\ndescription: d\n---\n"),
        ];
        for (label, document) in cases {
            assert!(
                BuiltinAgentSkill::from_document(document).is_err(),
                "{label} 必须被拒绝"
            );
        }
    }

    #[test]
    fn rejects_control_characters_in_the_body() {
        let document = "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文\u{7}。\n";
        let error = BuiltinAgentSkill::from_document(document).expect_err("控制字符必须被拒绝");
        assert_eq!(error.code().as_str(), "AGENT_SKILL_DOCUMENT_INVALID");

        let bare_carriage_return =
            "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文\rX。\n";
        assert!(
            BuiltinAgentSkill::from_document(bare_carriage_return).is_err(),
            "没有跟随换行的 CR 必须被拒绝"
        );
    }

    #[test]
    fn skill_id_shape_is_closed() {
        assert!(AgentSkillId::parse("haven-agent-proposal").is_ok());
        for raw in [
            "",
            "Haven",
            "-lead",
            "trail-",
            "double--hyphen",
            "with space",
            "with_underscore",
            "dot.dot",
        ] {
            assert!(AgentSkillId::parse(raw).is_err(), "{raw} 必须被拒绝");
        }
        assert!(AgentSkillId::parse(&"a".repeat(AGENT_SKILL_ID_MAX_CHARS)).is_ok());
        assert!(AgentSkillId::parse(&"a".repeat(AGENT_SKILL_ID_MAX_CHARS + 1)).is_err());
    }

    #[test]
    fn enablement_is_active_only_when_enabled_and_not_stale() {
        let skill = skill();
        let hash = skill.manifest().instructions_hash();
        let enabled = AgentSkillEnablement::parse(skill.id().clone(), hash, true).unwrap();
        let disabled = AgentSkillEnablement::parse(skill.id().clone(), hash, false).unwrap();
        let stale = AgentSkillEnablement::parse(skill.id().clone(), &"0".repeat(64), true).unwrap();

        assert!(enabled.is_active_for(&skill));
        assert!(!enabled.is_stale_for(&skill));
        assert!(!disabled.is_active_for(&skill));
        assert!(stale.is_stale_for(&skill));
        assert!(!stale.is_active_for(&skill));
    }

    #[test]
    fn enablement_rejects_a_non_digest_hash() {
        let skill = skill();
        for raw in ["", "ABC", &"a".repeat(63), &"A".repeat(64)] {
            assert!(
                AgentSkillEnablement::parse(skill.id().clone(), raw, true).is_err(),
                "{raw} 必须被拒绝"
            );
        }
    }

    #[test]
    fn resolve_active_skill_fails_closed_on_every_incomplete_input() {
        let skill = skill();
        assert!(resolve_active_skill(&skill, None).is_none());

        let hash = skill.manifest().instructions_hash();
        let disabled = AgentSkillEnablement::parse(skill.id().clone(), hash, false).unwrap();
        assert!(resolve_active_skill(&skill, Some(&disabled)).is_none());

        let stale = AgentSkillEnablement::parse(skill.id().clone(), &"0".repeat(64), true).unwrap();
        assert!(resolve_active_skill(&skill, Some(&stale)).is_none());

        let other =
            AgentSkillEnablement::parse(AgentSkillId::parse("other-skill").unwrap(), hash, true)
                .unwrap();
        assert!(resolve_active_skill(&skill, Some(&other)).is_none());

        let active = AgentSkillEnablement::parse(skill.id().clone(), hash, true).unwrap();
        let resolved = resolve_active_skill(&skill, Some(&active)).expect("启用且未过期必须生效");
        assert_eq!(resolved.id().as_str(), "haven-agent-proposal");
        assert_eq!(resolved.instructions().as_str(), DOCUMENT_BODY);
    }

    /// 一份同时面向两条运行时的文档：中间两段各自只属于一条路径。
    const TWO_AUDIENCE_DOCUMENT: &str = r#"---
name: haven-agent-proposal
description: 读取脱敏上下文并创建待批准提案
---

# 标题

两条路径共用的一段。

<!-- haven:audience= native  -->
原生：你没有工具，只能输出严格 JSON。
<!-- /haven:audience -->

<!-- haven:audience= external  -->
外部：第 0 步调用 `get_system_capabilities`。
<!-- /haven:audience -->

结尾共用。
"#;

    #[test]
    fn the_native_projection_drops_the_mcp_workflow_and_keeps_shared_text() {
        let instructions = AgentSkillInstructions::parse(TWO_AUDIENCE_DOCUMENT).unwrap();
        let native = instructions.native_projection().unwrap();
        assert!(native.as_str().contains("两条路径共用的一段。"));
        assert!(native.as_str().contains("结尾共用。"));
        assert!(native.as_str().contains("原生：你没有工具"));
        assert!(
            !native.as_str().contains("get_system_capabilities"),
            "原生投影不得保留 MCP 工作流——那个工具在原生运行时并不存在：\n{}",
            native.as_str()
        );
        assert!(
            !native.as_str().contains("haven:audience"),
            "标记行本身不进投影"
        );
    }

    #[test]
    fn the_external_projection_keeps_the_mcp_workflow_and_drops_native_only_text() {
        let instructions = AgentSkillInstructions::parse(TWO_AUDIENCE_DOCUMENT).unwrap();
        let external = instructions.external_projection().unwrap();
        assert!(external.as_str().contains("get_system_capabilities"));
        assert!(external.as_str().contains("两条路径共用的一段。"));
        assert!(!external.as_str().contains("原生：你没有工具"));
        assert!(!external.as_str().contains("haven:audience"));
    }

    #[test]
    fn malformed_audience_markers_are_rejected_rather_than_ignored() {
        // 宽松解析的代价是把外部工作流送进原生请求，因此每一种畸形都必须报错。
        // 每一条都在**解析到畸形处**就失败，所以片段本身不需要写完整。
        let cases: [(&str, &str); 7] = [
            ("unknown-audience", "<!-- haven:audience=server -->"),
            ("unclosed", "<!-- haven:audience=native -->"),
            ("stray-close", "<!-- /haven:audience -->"),
            (
                "nested",
                "<!-- haven:audience=native -->\n<!-- haven:audience=external -->",
            ),
            ("unknown-marker", "<!-- haven:whatever -->"),
            ("no-arrow", "<!-- haven:audience=native"),
            // 缩进的标记必须被当成标记（而不是普通正文）：否则它后面的外部段落会漏进原生投影。
            ("indented-unclosed", "  <!-- haven:audience=external -->"),
        ];
        for (label, body) in cases {
            let document =
                format!("---\nname: haven-agent-proposal\ndescription: d\n---\n\n{body}\n");
            let instructions = AgentSkillInstructions::parse(&document).unwrap();
            let projected = instructions.native_projection();
            assert!(projected.is_err(), "{label} 必须被拒绝");
            let error = projected.expect_err("上一行已断言必然失败");
            assert_eq!(
                error.code().as_str(),
                "AGENT_SKILL_DOCUMENT_INVALID",
                "{label}"
            );
        }
    }

    /// "看起来像标记、但不是规范形式"的行同样必须让整份文档失败。
    ///
    /// 与上面那组分开设，是因为**漏法不同**：上面每一条运行时会看到一个坏标记；这里每一条
    /// 都会被"只认前缀"的宽松解析当成**普通正文**，于是标记凭空消失——它后面的外部内容
    /// 没有开始行，既不触发嵌套、也不报"缺少闭合"，而是静默漏进原生投影。断言因此落在
    /// "整份文档被拒绝"上，而不是"输出里恰好没有那几个字"。
    #[test]
    fn marker_lookalikes_are_rejected_rather_than_treated_as_body_text() {
        let bodies = [
            "前言 <!-- haven:audience=external -->",
            "前言 <!-- /haven:audience -->",
            "<!--haven:audience=external-->",
            "<!-- haven:audience = external -->",
            "<!-- haven:audience=external --> 尾巴",
            "<!-- /haven:audience --> 尾巴",
        ];
        for body in bodies {
            let document =
                format!("---\nname: haven-agent-proposal\ndescription: d\n---\n\n{body}\n");
            let instructions = AgentSkillInstructions::parse(&document).unwrap();
            let projected = instructions.native_projection();
            assert!(
                projected.is_err(),
                "非规范标记必须被拒绝，而不是当成正文：{body}"
            );
        }
    }

    /// 一条前置了说明文字的标记**不能**把它后面的外部内容偷渡进原生投影。
    ///
    /// 这是这套机制的存在理由本身：外部段落里写着只有 MCP 才有的工具，原生模型没有这些
    /// 工具，读到只会被指使去调用不存在的东西。宽松解析在这里会**静默成功**，因此断言
    /// 落在"整份文档被拒绝"上，而不是"输出里恰好没有那几个字"。
    #[test]
    fn a_prefixed_marker_cannot_smuggle_external_text_into_the_native_projection() {
        let document = r#"---
name: haven-agent-proposal
description: d
---

前言 <!-- haven:audience=external -->

外部专用：这里写的是只有 MCP 才有的工作流。
"#;
        let instructions = AgentSkillInstructions::parse(document).unwrap();

        let projected = instructions.native_projection();
        assert!(
            projected.is_err(),
            "前置文字的标记必须让整份文档失败，而不是把外部段落当成共用正文"
        );
    }

    /// 缩进与受众名两侧空格被容忍（按 trim 后识别），且两条投影互不串味。
    ///
    /// 与 `marker_lookalikes_are_rejected_rather_than_treated_as_body_text` 是一对：
    /// **缩进**被容忍，**前置文字**不被容忍。
    #[test]
    fn marker_whitespace_variants_are_accepted_and_keep_the_audiences_apart() {
        let document = r#"---
name: haven-agent-proposal
description: d
---

共用开头。

  <!-- haven:audience= native  -->
原生段落。
<!-- /haven:audience -->

<!-- haven:audience= external  -->
EXTERNAL-ONLY-SENTINEL
<!-- /haven:audience -->

共用结尾。
"#;
        let instructions = AgentSkillInstructions::parse(document).unwrap();

        let native = instructions.native_projection().unwrap();
        assert!(native.as_str().contains("共用开头。"));
        assert!(native.as_str().contains("原生段落。"));
        assert!(native.as_str().contains("共用结尾。"));
        assert!(
            !native.as_str().contains("EXTERNAL-ONLY-SENTINEL"),
            "外部段落的内容不得出现在原生投影里"
        );
        assert!(!native.as_str().contains("haven:audience"));

        let external = instructions.external_projection().unwrap();
        assert!(external.as_str().contains("EXTERNAL-ONLY-SENTINEL"));
        assert!(!external.as_str().contains("原生段落。"));
        assert!(!external.as_str().contains("haven:audience"));
    }

    /// 这些是"YAML 会接受、但运行时拒绝"的 frontmatter 写法。
    ///
    /// 运行时是参考实现（Python 侧 `tools/skills/skill-contract-check.py` 必须给出同一个
    /// 答案），因此把边界钉在这里，而不是只钉在某一侧的用例里。
    #[test]
    fn frontmatter_is_a_closed_single_line_grammar() {
        let rejected: [(&str, &str); 6] = [
            (
                "quoted-name",
                "---\nname: \"haven-agent-proposal\"\ndescription: d\n---\n",
            ),
            // 多行值：第二行没有 `键:` 形状，整份文档按行解析仍然会失败。
            (
                "block-scalar",
                "---\nname: haven-agent-proposal\ndescription: |\n  d\n---\n",
            ),
            (
                "folded-scalar",
                "---\nname: haven-agent-proposal\ndescription: >\n  d\n---\n",
            ),
            (
                "comment-line",
                "---\nname: haven-agent-proposal\n# 注释\ndescription: d\n---\n",
            ),
            (
                "blank-line",
                "---\nname: haven-agent-proposal\n\ndescription: d\n---\n",
            ),
            // 分隔行必须**恰好**是 `---`：`----` 之类的变体不算 frontmatter。
            (
                "long-delimiter",
                "----\nname: haven-agent-proposal\ndescription: d\n---\n",
            ),
        ];
        for (label, document) in rejected {
            assert!(
                BuiltinAgentSkill::from_document(document).is_err(),
                "{label} 必须被拒绝"
            );
        }

        // 反过来：YAML 会**改写**、而运行时按字面接受的形式。两条路径都必须保留字面值，
        // 因此这里断言解析出来的就是原文里的那串字符——契约校验器不能"顺手"把引号、
        // 中括号或行尾注释去掉，否则它接受的就是一份运行时会拒绝的文档。
        let literal_cases: [(&str, &str, &str); 3] = [
            (
                "quoted",
                "---\nname: haven-agent-proposal\ndescription: \"d\"\n---\n",
                "\"d\"",
            ),
            (
                "inline-list",
                "---\nname: haven-agent-proposal\ndescription: [d]\n---\n",
                "[d]",
            ),
            (
                "trailing-comment",
                "---\nname: haven-agent-proposal\ndescription: d # 注释\n---\n",
                "d # 注释",
            ),
        ];
        for (label, document, expected) in literal_cases {
            let parsed = parse_skill_document(document).expect("必须按字面接受");
            assert_eq!(parsed.1, expected, "{label} 必须按字面保留");
        }
    }

    /// 启用记录绑定的是**会被送出去的那一份文字**，而不是磁盘上的原始文件。
    ///
    /// "送出去的那一份"要排除两样东西：只属于外部受众的段落，以及 frontmatter 元数据。
    #[test]
    fn the_content_hash_binds_to_the_native_projection_not_the_raw_file() {
        let base = BuiltinAgentSkill::from_document(TWO_AUDIENCE_DOCUMENT).unwrap();

        let external_edited = TWO_AUDIENCE_DOCUMENT.replace("第 0 步调用", "第 0 步先调用");
        let edited = BuiltinAgentSkill::from_document(&external_edited).unwrap();
        assert_eq!(
            base.manifest().instructions_hash(),
            edited.manifest().instructions_hash(),
            "只改外部段落的文字，不该让一条原生启用记录失效——送出去的文字没变"
        );

        let metadata_edited =
            TWO_AUDIENCE_DOCUMENT.replace("读取脱敏上下文并创建待批准提案", "另一段元数据描述");
        let edited = BuiltinAgentSkill::from_document(&metadata_edited).unwrap();
        assert_eq!(
            base.manifest().instructions_hash(),
            edited.manifest().instructions_hash(),
            "只改 frontmatter 元数据，同样不该让启用记录失效"
        );

        let shared_edited = TWO_AUDIENCE_DOCUMENT.replace("结尾共用。", "结尾共用了。");
        let edited = BuiltinAgentSkill::from_document(&shared_edited).unwrap();
        assert_ne!(
            base.manifest().instructions_hash(),
            edited.manifest().instructions_hash(),
            "共用段落变了就必须重新确认"
        );
    }

    #[test]
    fn content_hash_is_stable_and_domain_separated() {
        let instructions = AgentSkillInstructions::parse("abc").unwrap();
        // 同一输入重复求值必须一致（启用记录靠它比对）。
        assert_eq!(instructions.content_hash(), instructions.content_hash());
        // 换个域前缀就会得到另一个摘要——本测试用"另一个内容的摘要不同"来钉住
        // preimage 里确实掺入了本文内容之外的东西。
        assert_ne!(
            instructions.content_hash(),
            AgentSkillInstructions::parse("abd").unwrap().content_hash()
        );
        assert_ne!(
            instructions.content_hash(),
            AgentSkillInstructions::parse("abc\n")
                .unwrap()
                .content_hash()
        );
    }
}
