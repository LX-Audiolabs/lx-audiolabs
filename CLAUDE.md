# LX Audiolabs — Claude Code Bootstrap

## EXECUTION CONTRACT

**Dual-Truth System (2026-06-30):**

| Truth | Source | Location |
|-------|--------|----------|
| **Documentation** | MCP Vault (Obsidian) | `CLAP-vault/` via MCP |
| **Code** | Graphify | `graphify-out/` |

Before ANY planning, analysis or code changes you MUST read via MCP:

1. CLAUDE.md (Bootstrap)
2. GOVERNANCE.md (alle Agent-Regeln)
3. START-HERE.md
4. VAULT-SCHEMA.md
5. CURRENT-STATE.md
6. status/INDEX-SESSIONS.md
7. status/INDEX-OPEN-BUGS.md
8. status/todo-next-session.md

If the task targets a specific plugin, also read:

plugins/plugin-{name}.md

If ANY required document cannot be read:

STOP.

Inform the user.

Do not guess.

Do not continue.

Do not rely on previous conversation memory.

Do not make design decisions.

Do not implement until the required workflow has been completed.

The Vault always overrides memory.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
