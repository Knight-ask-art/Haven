---
doc_id: product.ai-boundaries
type: canonical
status: active
owner: product-and-architecture
visibility: public
source_of_truth: accepted-product-boundary
last_reviewed: 2026-10-14
review_after: 2026-12-20
---

# Haven Product AI Boundary (栖伴)

Naming: the internal codename in the agent runtime is `Copilot`
(`haven-application/src/services/agent.rs`), the user-visible name of this
assistant is 「栖伴」, and the settings section is 「AI 工作台」
(设置 → 智能). Component, file and directory names keep the `ai-assistant` /
`AiAssistantDialog` spelling. Use the visible name in product prose and the
neutral names when referring to code.

The boundary below is implemented, not aspirational: the settings section, its
Provider profile, credential, model catalog, settings-recommendation, agent
broker, MCP client auto-configuration and built-in Skill runtime entries are all
registered as implemented with real bindings and consumers. Real provider
responses remain a separate live-acceptance step.

Any product-AI capability must be context-bound and user-initiated. It may
read a bounded context and propose an action, but it never receives Haven's
general authority. Reads must use fixed Application services and a capability
manifest. Writes require a typed proposal, explicit user confirmation,
revision/CAS validation and a receipt.

Locator confidence must be explicit:

| Locator | Allowed behavior |
| --- | --- |
| Exact | Strict current-boundary answers and citations |
| Range | Answers limited to the known range |
| Unresolved | Selected/pasted text or metadata only, with a visible limitation |

Unresolved content must not be silently expanded to the entire work. The
capability Registry and runtime contracts determine the available surface, not
a design proposal or a mock conversation.
