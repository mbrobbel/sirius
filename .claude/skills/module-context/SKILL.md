---
name: module-context
description: Find dependency API context for Sirius operator, memory, expression, and I/O work using the module maps and current dependency headers.
---

# Module Context

Use `.claude/skills/module-discover/docs/<library>/README.md` to find relevant modules and
Sirius call sites, then consult the needed `modules/<name>.md` files. Maps are available for
cuDF, RMM, DuckDB, cuCascade, and libkvikio. Select only what the task needs.

The generated maps are a discovery aid, not authoritative: signatures, versions, and Sirius
examples can be stale. Follow the checkout's dependency pins and build configuration to find
current headers; verify needed APIs there and usage patterns in current Sirius source. If the
headers are unavailable, say so rather than presenting an unverified signature as current.
Report relevant call sites, version-specific behavior, and any gaps or discrepancies.
