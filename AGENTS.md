# AGENTS.md

Minimal operating guide for AI agents in this repo.

## Canonical spec

- **Single source of truth:** `docs/guidebook/` (see [guidebook README](docs/guidebook/README.md))

## Steps to do for any task

For every task:
1. Read the relevant chapter(s) in `docs/guidebook/`.
2. Map the task to the milestones in Chapter 10.
3. Understand how the task relates to other subsystems before implementing.
4. Implement without over-engineering. Prioritize readability and maintainability. The project currently has a low user base and MUST NOT implement any backward compatibility or legacy handling.
5. Review your own changes thoroughly to fix any issues found and verify the original goal was met.
6. If any oracle agent is available to you, ask it to review your changes as well thoroughly in paralell and check and fix the comments received from it.
7. Check if the changes done in the task need an update to `README.md` or `docs/guidebook` and make the changes if so including, but not limited to, marking the completed items appropriately in the plan.

## Agent execution rules

- All file operations in rust-bash go through `VirtualFs` — never use `std::fs` in command or interpreter code.
- No `std::process::Command` — all commands are in-process Rust implementations.
- Reuse existing types/traits before adding new abstractions.
- Keep changes incremental and test-gated.
- Run `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` on touched areas before finishing.
- File, function, and test names should describe behavior, not planning phases.
- Avoid clippy allow directives — fix the underlying issue instead.
- If requirements are unclear or conflicting, ask the user instead of guessing.
- When choosing between design approaches, present pros/cons and ask the user.
- Whenever adding any dependency crate, make sure we are using the latest version of it.
- NEVER commit any changes unless explcitly asked for by the user.