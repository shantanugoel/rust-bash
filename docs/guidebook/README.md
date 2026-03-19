# rust-bash Guidebook

Internal engineering documentation for rust-bash — a Rust-based sandboxed bash environment for AI agents. This guidebook describes the system architecture, design decisions, and implementation details. It is the canonical reference for contributors and AI agents working on this codebase.

## Chapters

| # | Chapter | Description |
|---|---------|-------------|
| 1 | [What We Are Building](01-what-we-are-building.md) | Vision, goals, non-goals, competitive positioning |
| 2 | [Architecture Overview](02-architecture-overview.md) | High-level design, module structure, data flow |
| 3 | [Parsing Layer](03-parsing-layer.md) | brush-parser integration, AST types, parsing pipeline |
| 4 | [Interpreter Engine](04-interpreter-engine.md) | AST walking, word expansion, control flow, pipelines |
| 5 | [Virtual Filesystem](05-virtual-filesystem.md) | VFS trait, InMemoryFs, OverlayFs, MountableFs, ReadWriteFs |
| 6 | [Command System](06-command-system.md) | Command trait, registry, command categories, custom commands |
| 7 | [Execution Safety](07-execution-safety.md) | Limits, network policy, security model, threat mitigations |
| 8 | [Integration Targets](08-integration-targets.md) | C FFI, WASM, CLI binary, AI SDK tool definitions |
| 9 | [Testing Strategy](09-testing-strategy.md) | Unit tests, snapshot tests, differential testing, fuzzing |
| 10 | [Implementation Plan](10-implementation-plan.md) | Milestones, dependencies, build order, risk register |

## How to Use This Guidebook

- **New to the codebase?** Start with Chapter 1 for the "why", then Chapter 2 for the "how".
- **Implementing a feature?** Read the relevant chapter for the subsystem you're touching, then check Chapter 10 for milestone context and dependencies.
- **Adding a command?** Chapter 6 covers the command trait, registration, and conventions.
- **Working on the interpreter?** Chapters 3 and 4 cover parsing and execution in detail.
- **Security review?** Chapter 7 covers the threat model and all mitigations.

## Conventions

- Chapters describe the target architecture, not just what exists today. Sections that describe unimplemented features are marked with **(planned)**.
- Code examples show the target API. If the current implementation differs, both are shown.
- This guidebook is the single source of truth. If `AGENTS.md` or `README.md` diverge from it, follow the guidebook.

## Maintenance

When making changes to the codebase:
1. Update the relevant guidebook chapter(s) to reflect the change.
2. If a planned feature becomes implemented, remove the **(planned)** marker.
3. Keep Chapter 10 (Implementation Plan) current with milestone status.
