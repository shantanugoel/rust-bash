# npm Package

## Goal

Install and use the `rust-bash` npm package to run sandboxed bash scripts from TypeScript or JavaScript — in Node.js or the browser.

## Installation

```bash
npm install rust-bash
```

Requires Node.js 18 or later.

## Basic Usage

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash';

await initWasm();

const bash = await Bash.create(createWasmBackend, {
  files: { '/data.txt': 'hello world' },
  env: { USER: 'agent' },
});

const result = await bash.exec('cat /data.txt | tr a-z A-Z');
console.log(result.stdout);   // "HELLO WORLD\n"
console.log(result.stderr);   // ""
console.log(result.exitCode); // 0
```

## Node.js: Auto-Detecting the Backend

In Node.js, the package supports two backends. The native addon is faster; the WASM backend is a universal fallback:

```typescript
import {
  Bash,
  tryLoadNative, createNativeBackend,
  initWasm, createWasmBackend,
} from 'rust-bash';

let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;  // Native addon (fast)
} else {
  await initWasm();
  createBackend = createWasmBackend;    // WASM fallback (universal)
}

const bash = await Bash.create(createBackend, {
  files: { '/data.txt': 'hello world' },
  env: { USER: 'agent' },
});

const result = await bash.exec('cat /data.txt | grep hello');
console.log(result.stdout); // "hello world\n"
```

## Browser Usage

In the browser, import from `rust-bash/browser` — this entry point excludes the native addon loader:

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();

const bash = await Bash.create(createWasmBackend, {
  files: { '/hello.txt': 'Hello from WASM!' },
});

const result = await bash.exec('cat /hello.txt');
console.log(result.stdout); // "Hello from WASM!\n"
```

Browser-specific considerations:
- Only `InMemoryFs` is available (no host filesystem access)
- Only WASM backend — no native addon
- `sleep` is not supported
- Use custom commands to bridge browser APIs like `fetch()`

## File Seeding

Populate the virtual filesystem at creation time with the `files` option:

### Eager Files (String)

Written immediately when the instance is created:

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/data.json': '{"name": "world"}',
    '/config.yml': 'debug: true',
    '/src/main.rs': 'fn main() {}',
  },
});
```

Parent directories are created automatically — `/src/` doesn't need to exist beforehand.

### Lazy Sync Files

Provide a function that returns a string. It's resolved on first `exec()` or `readFile()` call — not during `Bash.create()`:

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/config.json': () => JSON.stringify(getConfig()),
    '/timestamp.txt': () => new Date().toISOString(),
  },
});
```

### Lazy Async Files

Provide an async function. All lazy files are resolved concurrently on first `exec()` call:

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/remote-data.txt': async () => {
      const res = await fetch('https://api.example.com/data');
      return await res.text();
    },
    '/other-data.txt': async () => {
      const res = await fetch('https://api.example.com/other');
      return await res.text();
    },
  },
});
// Both fetches run in parallel on first exec() call
```

### Mixing All Three

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/data.txt': 'hello world',                       // eager
    '/config.json': () => JSON.stringify(getConfig()), // lazy sync
    '/remote.txt': async () => fetchData(),            // lazy async
  },
});
```

## Custom Commands with defineCommand

Create custom commands that scripts can call like built-ins:

```typescript
import { Bash, defineCommand } from 'rust-bash';

const greet = defineCommand('greet', async (args, ctx) => {
  const name = args[0] ?? 'world';
  return { stdout: `Hello, ${name}!\n`, stderr: '', exitCode: 0 };
});

const fetchCmd = defineCommand('fetch', async (args, ctx) => {
  const url = args[0];
  if (!url) {
    return { stdout: '', stderr: 'fetch: missing URL\n', exitCode: 1 };
  }
  const response = await globalThis.fetch(url);
  return {
    stdout: await response.text(),
    stderr: '',
    exitCode: response.ok ? 0 : 1,
  };
});

const bash = await Bash.create(createBackend, {
  customCommands: [greet, fetchCmd],
});

await bash.exec('greet Alice');           // "Hello, Alice!\n"
await bash.exec('fetch https://example.com | grep -i title');
```

### Command Context

Custom command callbacks receive `(args, ctx)` where `ctx` provides:

| Property | Type | Description |
|----------|------|-------------|
| `ctx.fs` | `FileSystemProxy` | Read/write the virtual filesystem |
| `ctx.cwd` | `string` | Current working directory |
| `ctx.env` | `Record<string, string>` | Shell environment variables |
| `ctx.stdin` | `string` | Piped standard input |
| `ctx.exec` | `(cmd, opts?) => Promise<ExecResult>` | Execute sub-commands |

```typescript
const deploy = defineCommand('deploy', async (args, ctx) => {
  // Read a file from VFS
  const manifest = ctx.fs.readFileSync('/app/manifest.json');
  const version = JSON.parse(manifest).version;

  // Execute a sub-command
  const build = await ctx.exec('cat /app/build.log | tail -1');

  return {
    stdout: `Deploying v${version}: ${build.stdout}`,
    stderr: '',
    exitCode: 0,
  };
});
```

## TypeScript Types Overview

### Core Types

```typescript
// File entry types
type EagerFile = string;
type LazySyncFile = () => string;
type LazyAsyncFile = () => Promise<string>;
type FileEntry = EagerFile | LazySyncFile | LazyAsyncFile;

// Execution result
interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

// Per-exec options
interface ExecOptions {
  env?: Record<string, string>;
  replaceEnv?: boolean;
  cwd?: string;
  stdin?: string;
  rawScript?: boolean;
  args?: string[];
}
```

### Configuration Types

```typescript
interface BashOptions {
  files?: Record<string, FileEntry>;
  env?: Record<string, string>;
  cwd?: string;
  commands?: string[];                  // command allow-list
  customCommands?: CustomCommand[];
  executionLimits?: ExecutionLimits;
}

interface ExecutionLimits {
  maxCommandCount?: number;             // default: 10,000
  maxExecutionTimeSecs?: number;        // default: 30
  maxLoopIterations?: number;           // default: 10,000
  maxOutputSize?: number;               // default: 10 MB
  maxCallDepth?: number;                // default: 25
  maxStringLength?: number;             // default: 10 MB
  maxGlobResults?: number;              // default: 100,000
  maxSubstitutionDepth?: number;        // default: 50
  maxHeredocSize?: number;              // default: 10 MB
  maxBraceExpansion?: number;           // default: 10,000
}
```

### Custom Command Types

```typescript
interface CustomCommand {
  name: string;
  execute: (args: string[], ctx: CommandContext) => Promise<ExecResult>;
}

interface CommandContext {
  fs: FileSystemProxy;
  cwd: string;
  env: Record<string, string>;
  stdin: string;
  exec: (command: string, options?: { cwd?: string; stdin?: string }) => Promise<ExecResult>;
}
```

### FileSystemProxy

```typescript
interface FileSystemProxy {
  readFileSync(path: string): string;
  writeFileSync(path: string, content: string): void;
  existsSync(path: string): boolean;
  mkdirSync(path: string, options?: { recursive?: boolean }): void;
  readdirSync(path: string): string[];
  statSync(path: string): FileStat;
  rmSync(path: string, options?: { recursive?: boolean }): void;
}

interface FileStat {
  isFile: boolean;
  isDirectory: boolean;
  size: number;
}
```

## Package Exports Structure

| Export Path | Environment | Includes |
|-------------|-------------|----------|
| `rust-bash` | Node.js | `Bash`, `defineCommand`, native + WASM loaders, AI tool helpers |
| `rust-bash/browser` | Browser | `Bash`, `defineCommand`, WASM loader only |

### Key Exports

| Export | Description |
|--------|-------------|
| `Bash` | Main class — `Bash.create(backend, options)` |
| `defineCommand` | Factory for custom commands |
| `initWasm` | Initialize the WASM module |
| `createWasmBackend` | Create a WASM-backed instance |
| `tryLoadNative` | Check if the native addon is available (Node.js only) |
| `createNativeBackend` | Create a native addon instance (Node.js only) |
| `bashToolDefinition` | JSON Schema tool definition for AI function calling |
| `createBashToolHandler` | Factory for AI tool handlers |
| `formatToolForProvider` | Format tools for OpenAI, Anthropic, or MCP |
| `handleToolCall` | Multi-tool dispatcher |

## Next Steps

- [Getting Started](getting-started.md) — Rust and TypeScript quick start
- [WASM Usage](wasm-usage.md) — building WASM from source, browser integration
- [Convenience API](convenience-api.md) — command filtering, transform plugins, safe args
- [Custom Commands](custom-commands.md) — in-depth custom command guide
- [Embedding in an AI Agent](ai-agent-tool.md) — use as a tool for LLM function calling
