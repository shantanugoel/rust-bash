# Migrating from just-bash

## Overview

`@shantanugoel/rust-bash` is designed as a drop-in replacement for `just-bash` with an expanded feature set. This guide covers the key differences and how to migrate your code.

## Installation

```bash
# Remove just-bash
npm uninstall just-bash

# Install @shantanugoel/rust-bash
npm install @shantanugoel/rust-bash
```

## API Comparison

### Creating a Bash Instance

**just-bash:**

```typescript
import { Bash } from 'just-bash';

const bash = new Bash({
  files: { '/data.txt': 'hello' },
  env: { USER: 'agent' },
});
```

**@shantanugoel/rust-bash:**

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from '@shantanugoel/rust-bash';

// Choose a backend (native addon for speed, WASM for portability)
let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}

const bash = await Bash.create(createBackend, {
  files: { '/data.txt': 'hello' },
  env: { USER: 'agent' },
});
```

**Key difference:** `Bash.create()` is async and requires a backend factory. The backend determines whether commands execute via the native Rust addon (near-native speed) or WASM.

### Executing Commands

**just-bash:**

```typescript
const result = bash.exec('echo hello');
console.log(result.stdout);   // "hello\n"
console.log(result.exitCode); // 0
```

**@shantanugoel/rust-bash:**

```typescript
const result = await bash.exec('echo hello');
console.log(result.stdout);   // "hello\n"
console.log(result.exitCode); // 0
```

**Key difference:** `exec()` returns a `Promise`. The result shape (`stdout`, `stderr`, `exitCode`) is the same.

### Custom Commands

**just-bash:**

```typescript
import { Bash, defineCommand } from 'just-bash';

const greet = defineCommand('greet', async (args, ctx) => {
  return { stdout: `Hello, ${args[0]}!\n`, stderr: '', exitCode: 0 };
});

const bash = new Bash({ customCommands: [greet] });
```

**@shantanugoel/rust-bash:**

```typescript
import { Bash, defineCommand } from '@shantanugoel/rust-bash';

const greet = defineCommand('greet', async (args, ctx) => {
  return { stdout: `Hello, ${args[0]}!\n`, stderr: '', exitCode: 0 };
});

const bash = await Bash.create(createBackend, { customCommands: [greet] });
```

**Key difference:** The `defineCommand()` API is identical. Only the `Bash` construction changes.

### Execution Limits

**just-bash:**

```typescript
const bash = new Bash({
  executionLimits: { maxCommandCount: 1000, maxExecutionTimeSecs: 5 },
});
```

**@shantanugoel/rust-bash:**

```typescript
const bash = await Bash.create(createBackend, {
  executionLimits: { maxCommandCount: 1000, maxExecutionTimeSecs: 5 },
});
```

The `executionLimits` interface is the same. All 10 limits are supported:

| Limit | Default |
|-------|---------|
| `maxCallDepth` | 100 |
| `maxCommandCount` | 10,000 |
| `maxLoopIterations` | 10,000 |
| `maxExecutionTimeSecs` | 30 |
| `maxOutputSize` | 10 MB |
| `maxStringLength` | 10 MB |
| `maxGlobResults` | 100,000 |
| `maxSubstitutionDepth` | 50 |
| `maxHeredocSize` | 10 MB |
| `maxBraceExpansion` | 10,000 |

## Quick Migration Checklist

| Step | Change |
|------|--------|
| 1. Package | `just-bash` → `@shantanugoel/rust-bash` |
| 2. Import | `from 'just-bash'` → `from '@shantanugoel/rust-bash'` |
| 3. Backend setup | Add backend detection (see above) |
| 4. Construction | `new Bash(opts)` → `await Bash.create(createBackend, opts)` |
| 5. Execution | `bash.exec(cmd)` → `await bash.exec(cmd)` |
| 6. Custom commands | No change — `defineCommand()` API is identical |
| 7. Limits | No change — same `executionLimits` interface |
| 8. Files | No change — same `files` option (eager + lazy supported) |

## New Features in @shantanugoel/rust-bash

Features available in `@shantanugoel/rust-bash` that aren't in `just-bash`:

### AI Tool Integration (Framework-Agnostic)

```typescript
import { bashToolDefinition, createBashToolHandler, formatToolForProvider } from '@shantanugoel/rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, {
  files: myFiles,
  maxOutputLength: 10000,
});

// Works with any AI framework — not locked to Vercel AI SDK
const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');
```

### MCP Server

```bash
# Built-in MCP server for Claude Desktop, Cursor, VS Code
rust-bash --mcp
```

### Network Policy

```typescript
const bash = await Bash.create(createBackend, {
  network: {
    enabled: true,
    allowedUrlPrefixes: ['https://api.example.com/'],
    allowedMethods: ['GET', 'POST'],
  },
});

await bash.exec('curl https://api.example.com/data');
```

### Direct Filesystem Access

```typescript
bash.fs.writeFileSync('/output.txt', 'content');
const data = bash.fs.readFileSync('/output.txt');
bash.fs.mkdirSync('/dir', { recursive: true });
const entries = bash.fs.readdirSync('/');
```

### Native Node.js Addon

When the native addon is available, commands execute at near-native speed via napi-rs — significantly faster than the pure TypeScript interpreter in `just-bash`.

### Multiple Filesystem Backends (Rust)

When using the Rust API directly, you get additional filesystem backends:

- **OverlayFs** — copy-on-write over real files
- **ReadWriteFs** — direct host filesystem access
- **MountableFs** — compose backends at mount points

## Browser Usage

**just-bash:**

```typescript
import { Bash } from 'just-bash';
const bash = new Bash({ files: { '/data.txt': 'hello' } });
```

**@shantanugoel/rust-bash:**

```typescript
import { Bash, initWasm, createWasmBackend } from '@shantanugoel/rust-bash/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  files: { '/data.txt': 'hello' },
});
```

**Key difference:** Browser usage requires initializing the WASM module first with `initWasm()`. Use the `/browser` entry point for tree-shaking.

## Troubleshooting

### "Cannot find module '@shantanugoel/rust-bash'"

Ensure you've installed the package:

```bash
npm install @shantanugoel/rust-bash
```

### "tryLoadNative is not available" / native addon not loading

The native addon requires platform-specific binaries. If they're not available for your platform, the package falls back to WASM automatically. This is normal — WASM provides the same functionality, just slightly slower.

### TypeScript types not resolving

Ensure your `tsconfig.json` has `"moduleResolution": "bundler"` or `"node16"` for proper ESM support:

```json
{
  "compilerOptions": {
    "moduleResolution": "bundler",
    "module": "ESNext",
    "target": "ES2022"
  }
}
```
