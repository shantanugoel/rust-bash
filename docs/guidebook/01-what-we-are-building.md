# Chapter 1: What We Are Building

## The Problem

AI agents need a bash tool. Today's options all have significant drawbacks:

| Approach | Problem |
|----------|---------|
| Real shell on host | Security nightmare — agents can `rm -rf /`, exfiltrate data, spawn processes |
| Docker/VM per agent | Heavy — 100-500ms startup, memory overhead, orchestration complexity |
| Node.js sandbox (just-bash) | Requires Node.js runtime, limited to JavaScript embedding |
| Restricted shell (rbash) | Only restricts *some* operations; still touches real filesystem |

There is no lightweight, embeddable, zero-dependency bash sandbox that can be dropped into any language or platform.

## The Solution

**rust-bash** is a sandboxed bash environment built in Rust. It parses and executes bash scripts entirely in-process, with all filesystem operations going through a virtual filesystem (VFS). No real files are touched, no processes are spawned, no network requests are made — unless explicitly allowed.

It deploys as:
- A **Rust crate** for native embedding
- A **static binary** (CLI) with zero runtime dependencies
- A **C FFI library** for embedding in Python, Go, Ruby, or any language with C interop
- A **WASM module** for browser and edge runtime embedding

## Design Principles

1. **Zero runtime dependencies** — ships as a static binary or library. No Node.js, no Python, no containers.

2. **No real OS access by default** — all filesystem operations go through a virtual filesystem. The default `InMemoryFs` has zero `std::fs` calls.

3. **No process spawning** — all commands are implemented in Rust, in-process. There is no `std::process::Command` anywhere in the codebase.

4. **Composable filesystem backends** — `InMemoryFs` for full sandboxing, `OverlayFs` for copy-on-write over real directories, `ReadWriteFs` for passthrough, `MountableFs` for mixing backends at different mount points.

5. **Execution limits** — prevent runaway scripts with configurable limits on depth, count, time, output size, and more.

6. **Parser reuse** — leverage `brush-parser`'s battle-tested bash grammar instead of hand-rolling a parser. We focus on execution, not parsing.

## Non-Goals

- **Full POSIX compliance** — we target the bash subset that AI agents actually use, not every obscure POSIX feature.
- **Interactive terminal features** — no job control (`fg`, `bg`, `jobs`), no signal handling, no `readline`. This is a scripting sandbox, not a terminal emulator.
- **Multi-process semantics** — no `fork()`, no background processes (`&`), no `wait`. Commands execute sequentially.
- **Performance at the expense of safety** — we prefer correctness and sandboxing guarantees over raw throughput.

## Target Users

1. **AI agent frameworks** — provide a bash tool that agents can use safely without container overhead.
2. **Code sandbox providers** — embed rust-bash for lightweight code execution environments.
3. **Education platforms** — let students run bash commands in-browser via WASM without server infrastructure.
4. **Testing tools** — run bash scripts in isolated environments for deterministic testing.

## Competitive Positioning

We evaluated six approaches to giving AI agents bash capabilities:

| Approach | Example | How it works |
|----------|---------|-------------|
| Container/MicroVM | E2B, Modal, Fly.io | Real `/bin/bash` inside an isolated VM or container |
| just-bash (TypeScript) | Vercel just-bash | Reimplemented bash interpreter + 75 commands in TypeScript |
| **rust-bash (this project)** | — | brush-parser + custom Rust interpreter + in-memory VFS |
| WASM bash binary | BusyBox → Emscripten | Real C bash/busybox compiled to WebAssembly |
| Real bash (no sandbox) | `std::process::Command` | Shell out to `/bin/bash` on the host |
| Restricted real bash | firejail, nsjail, bubblewrap | Real bash with OS-level sandboxing (seccomp, namespaces) |

### Summary Scorecard

Milestones M1–M4 (core interpreter, text processing, execution safety, and filesystem backends) are complete. M5 (C FFI, WASM, standalone CLI binary) is planned.

| Metric | Container | just-bash | **rust-bash** | WASM bash | Real bash | Restricted bash |
|--------|-----------|-----------|---------------|-----------|-----------|----------------|
| Startup latency | ⚠️ 150ms–12s | ⚠️ 50–100ms | ✅ **<1ms** | ⚠️ 50–200ms | ✅ 3ms | ⚠️ 10–50ms |
| Memory per sandbox | ❌ 30–128MB | ⚠️ 20–50MB | ✅ **1–5MB** | ⚠️ 10–30MB | ✅ 5MB | ✅ 5MB |
| Dependencies | ❌ Heavy | ⚠️ Node.js | ✅ **None** | ⚠️ WASM runtime | ✅ OS | ⚠️ Linux only |
| Bash compatibility | ✅ Perfect | ✅ Good | ⚠️ Growing | ✅ Perfect | ✅ Perfect | ✅ Perfect |
| Security | ✅ Strong | ✅ Good | ✅ Good | ⚠️ Medium | ❌ None | ⚠️ Medium |
| Browser support | ❌ No | ✅ Yes | ✅ **Yes (smaller)** | ✅ Yes (large) | ❌ No | ❌ No |
| Polyglot embedding | ❌ HTTP only | ❌ TS only | ✅ **Any language** | ⚠️ Via WASM | ✅ Subprocess | ⚠️ Linux only |
| Cost | ❌ Cloud billing | ✅ Free | ✅ **Free** | ✅ Free | ✅ Free | ✅ Free |
| Maturity | ✅ Production | ✅ Production | ❌ **Early dev** | ❌ Experimental | ✅ Decades | ⚠️ Niche |

### When to Use What

| Scenario | Best approach | Why |
|----------|--------------|-----|
| Full-featured cloud agent (needs pip, git, arbitrary binaries) | Container (E2B/Modal) | Only real OS can run arbitrary binaries |
| Lightweight agent tool (CLI, no infra, basic bash scripting) | **rust-bash** | Zero dependencies, sub-ms latency, library call |
| Browser-based coding assistant | **rust-bash (WASM)** or just-bash | Smallest bundle, no server needed |
| Existing TypeScript/Node.js agent | just-bash | Native integration, production-proven |
| Python/Go agent framework needing bash tool | **rust-bash (C FFI)** | Native embedding, no Node.js dependency |
| Edge worker (Cloudflare, Deno Deploy) | **rust-bash (WASM)** | Smallest footprint, fastest cold start |
| High-security (untrusted agents, must prevent escape) | Container + **rust-bash inside** | Defense in depth: VM isolation + no-OS-access interpreter |

### rust-bash's Advantages

- **Latency**: sub-ms per exec, no VM boot or GC pause
- **Memory**: ~1–5MB per sandbox vs 20–128MB for alternatives
- **Zero dependencies**: static binary, C FFI, or WASM — no runtime to install
- **Polyglot embedding**: any language with C FFI can use it natively
- **Browser size**: ~1–1.5MB WASM vs 2–10MB for alternatives

### rust-bash's Disadvantages

- **Maturity**: early development, not yet production-proven
- **Compatibility**: growing command set, doesn't cover every bash edge case
- **No real processes**: can't run `pip install`, `git clone`, or other real binaries

## Reference Implementation

[just-bash](https://github.com/vercel-labs/just-bash) by Vercel is the primary behavioral reference. It implements a sandboxed bash environment in TypeScript with an in-memory virtual filesystem. Our goal is functional equivalence with just-bash, plus the additional capabilities enabled by Rust (FFI, WASM, OverlayFs, better performance).
