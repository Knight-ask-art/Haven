---
doc_id: product.ai-boundaries
type: canonical
status: active
owner: product-and-architecture
visibility: public
source_of_truth: accepted-product-boundary
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Haven Copilot Boundary

Engineering agents and the future Haven Copilot are different systems.

Any Haven Copilot implementation must be context-bound and user-initiated. It may
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
current AI plans are design/implementation inputs, not evidence that a complete
Provider, stream, MCP or product Chat surface exists.
