# @rust-bash/core

A sandboxed bash interpreter powered by Rust — TypeScript API with native Node.js addon and WASM support.

## Installation

```bash
npm install @rust-bash/core
```

## Quick Start

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from '@rust-bash/core';

// Auto-detect backend: native addon (fast) or WASM (universal)
let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}

// Create a sandboxed bash instance
const bash = await Bash.create(createBackend, {
  files: {
    '/data.json': '{"name": "world"}',
    '/script.sh': 'echo "Hello, $(jq -r .name /data.json)!"',
  },
  env: { USER: 'agent' },
});

// Execute commands
const result = await bash.exec('bash /script.sh');
console.log(result.stdout); // "Hello, world!\n"
console.log(result.exitCode); // 0
```

## Features

- **Sandboxed execution** — all filesystem operations are in-memory
- **Native performance** — napi-rs addon for Node.js (near-native speed)
- **Browser support** — WASM build for browsers and edge runtimes
- **80+ commands** — echo, cat, grep, sed, awk, jq, find, sort, diff, curl, and more
- **Full bash syntax** — pipelines, redirections, variables, control flow, functions, globs, arithmetic, heredocs
- **Custom commands** — `defineCommand()` for extending the shell
- **Lazy file loading** — functions as file values, resolved at `Bash.create()` time
- **AI tool integration** — JSON Schema definitions for any AI framework
- **MCP server** — built-in Model Context Protocol server
- **Execution limits** — 10 configurable resource bounds
- **TypeScript-first** — full type definitions

## API Reference

### `Bash.create(backendFactory, options?)`

Create a sandboxed bash instance.

```typescript
const bash = await Bash.create(createBackend, {
  // Seed files in the virtual filesystem
  files: {
    '/data.txt': 'hello world',           // eager: written immediately
    '/lazy.txt': () => 'lazy content',     // lazy sync: resolved at Bash.create() time
    '/async.txt': async () => fetchData(), // lazy async: resolved at Bash.create() time
  },
  // Environment variables
  env: { USER: 'agent', HOME: '/home/agent' },
  // Working directory (default: "/")
  cwd: '/',
  // Execution limits (all optional, defaults are generous)
  executionLimits: {
    maxCommandCount: 10000,
    maxExecutionTimeSecs: 30,
    maxLoopIterations: 10000,
    maxOutputSize: 10485760,       // 10 MB
    maxCallDepth: 100,
    maxStringLength: 10485760,
    maxGlobResults: 100000,
    maxSubstitutionDepth: 50,
    maxHeredocSize: 10485760,
    maxBraceExpansion: 10000,
  },
  // Network configuration (disabled by default)
  network: {
    enabled: true,
    allowedUrlPrefixes: ['https://api.example.com/'],
    allowedMethods: ['GET', 'POST'],
  },
  // Custom commands
  customCommands: [myCommand],
});
```

**Options:**

| Field | Type | Description |
|-------|------|-------------|
| `files` | `Record<string, FileEntry>` | Seed files — eager strings, lazy sync functions, or lazy async functions |
| `env` | `Record<string, string>` | Environment variables |
| `cwd` | `string` | Initial working directory (default: `"/"`) |
| `executionLimits` | `Partial<ExecutionLimits>` | Resource bounds |
| `network` | `NetworkConfig` | Network policy for `curl` |
| `customCommands` | `CustomCommand[]` | Custom command definitions |

### `bash.exec(command, options?)`

Execute a bash command string. Returns a `Promise<ExecResult>`.

```typescript
const result = await bash.exec('echo hello | tr a-z A-Z');
// { stdout: "HELLO\n", stderr: "", exitCode: 0 }

// Per-exec options
const result2 = await bash.exec('cat /data.txt', {
  env: { LANG: 'en_US.UTF-8' },  // per-exec env overrides
  cwd: '/data',                   // per-exec working directory
  stdin: 'input data',            // standard input
  rawScript: false,               // apply whitespace normalization (default)
});
```

**ExecResult:**

| Field | Type | Description |
|-------|------|-------------|
| `stdout` | `string` | Standard output |
| `stderr` | `string` | Standard error |
| `exitCode` | `number` | Exit code (0 = success) |
| `env` | `Record<string, string>` | Final environment variables (optional) |

**ExecOptions:**

| Field | Type | Description |
|-------|------|-------------|
| `env` | `Record<string, string>` | Per-exec env overrides (merged with instance env) |
| `cwd` | `string` | Per-exec working directory |
| `stdin` | `string` | Standard input content |
| `rawScript` | `boolean` | If true, skip leading whitespace stripping |

### `bash.fs` — FileSystemProxy

Direct access to the virtual filesystem:

```typescript
bash.fs.writeFileSync('/output.txt', 'content');
const data = bash.fs.readFileSync('/output.txt');     // string
const exists = bash.fs.existsSync('/output.txt');     // boolean
bash.fs.mkdirSync('/dir', { recursive: true });
const entries = bash.fs.readdirSync('/');              // string[]
const stat = bash.fs.statSync('/output.txt');          // { isFile, isDirectory, size }
bash.fs.rmSync('/output.txt');
bash.fs.rmSync('/dir', { recursive: true });
```

### `defineCommand(name, execute)`

Create a custom command:

```typescript
import { defineCommand } from '@rust-bash/core';

const fetch = defineCommand('fetch', async (args, ctx) => {
  const url = args[0];
  const response = await globalThis.fetch(url);
  const text = await response.text();
  return { stdout: text, stderr: '', exitCode: 0 };
});

const bash = await Bash.create(createBackend, {
  customCommands: [fetch],
});

await bash.exec('fetch https://api.example.com/data');
```

**CommandContext (passed to execute):**

| Field | Type | Description |
|-------|------|-------------|
| `fs` | `FileSystemProxy` | Virtual filesystem access |
| `cwd` | `string` | Current working directory |
| `env` | `Record<string, string>` | Environment variables |
| `stdin` | `string` | Standard input |
| `exec` | `(cmd, opts?) => Promise<ExecResult>` | Execute sub-commands |

### AI Tool Integration

Framework-agnostic tool definitions for LLM function calling:

```typescript
import {
  bashToolDefinition,
  createBashToolHandler,
  formatToolForProvider,
  handleToolCall,
} from '@rust-bash/core';
```

#### `bashToolDefinition`

A plain JSON Schema tool definition object:

```typescript
{
  name: 'bash',
  description: 'Execute bash commands in a sandboxed environment...',
  inputSchema: {
    type: 'object',
    properties: { command: { type: 'string', description: '...' } },
    required: ['command'],
  },
}
```

#### `createBashToolHandler(backendFactory, options?)`

Create a tool handler with an embedded bash instance:

```typescript
const { handler, definition, bash } = createBashToolHandler(createNativeBackend, {
  files: { '/data.txt': 'hello world' },
  maxOutputLength: 10000,
});

const result = await handler({ command: 'grep hello /data.txt' });
// { stdout: 'hello world\n', stderr: '', exitCode: 0 }
```

#### `formatToolForProvider(definition, provider)`

Format a tool definition for a specific AI provider:

```typescript
const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
// { type: "function", function: { name, description, parameters } }

const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');
// { name, description, input_schema }

const mcpTool = formatToolForProvider(bashToolDefinition, 'mcp');
// { name, description, inputSchema }
```

#### `handleToolCall(bash, toolName, args)`

Dispatch tool calls in an agent loop:

```typescript
const result = await handleToolCall(bash, toolCall.name, toolCall.arguments);
// Supports: 'bash', 'readFile'/'read_file', 'writeFile'/'write_file',
//           'listDirectory'/'list_directory'
```

Additional tool definitions: `writeFileToolDefinition`, `readFileToolDefinition`, `listDirectoryToolDefinition`.

## Backend Detection

### Node.js

On Node.js, try the native addon first for best performance:

```typescript
import { tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from '@rust-bash/core';

let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}
```

### Browser

In the browser, only WASM is available. Use the `/browser` entry point:

```typescript
import { Bash, initWasm, createWasmBackend } from '@rust-bash/core/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, { /* options */ });
```

## Supported Commands

The interpreter supports 80+ commands including:

`awk`, `base64`, `basename`, `cat`, `cd`, `chmod`, `comm`, `column`, `cp`, `cut`, `date`, `diff`, `dirname`, `echo`, `env`, `eval`, `exit`, `expand`, `export`, `expr`, `false`, `find`, `fmt`, `fold`, `grep`, `head`, `hostname`, `jq`, `join`, `ln`, `ls`, `md5sum`, `mkdir`, `mv`, `nl`, `paste`, `printf`, `printenv`, `pwd`, `read`, `realpath`, `rev`, `rm`, `sed`, `seq`, `set`, `sha256sum`, `sleep`, `sort`, `source`, `stat`, `tac`, `tail`, `tee`, `test`, `touch`, `tr`, `trap`, `tree`, `true`, `uniq`, `unexpand`, `unset`, `uname`, `wc`, `which`, `whoami`, `xargs`, `yes`

Plus 18 interpreter builtins: `exit`, `cd`, `export`, `unset`, `set`, `shift`, `readonly`, `declare`, `read`, `eval`, `source`, `break`, `continue`, `:`, `let`, `local`, `return`, `trap`.

## Comparison with just-bash

| Feature | just-bash | @rust-bash/core |
|---------|-----------|-----------------|
| Language | Pure TypeScript | Rust → WASM + native addon |
| Performance | JS-speed | Near-native (native addon) / WASM |
| API | `new Bash(opts)` | `Bash.create(backend, opts)` |
| Custom commands | `defineCommand()` | `defineCommand()` (same API) |
| AI integration | Vercel AI SDK only | Framework-agnostic (OpenAI, Anthropic, Vercel, LangChain) |
| MCP server | ❌ | ✅ Built-in (`rust-bash --mcp`) |
| Browser | ✅ | ✅ (WASM) |
| Node.js native | ❌ | ✅ (napi-rs) |
| Network policy | ❌ | ✅ (URL allow-list, method restrictions) |
| Filesystem backends | In-memory only | InMemoryFs, OverlayFs, ReadWriteFs, MountableFs |

See the [Migration Guide](https://github.com/shantanugoel/rust-bash/blob/main/docs/recipes/migrating-from-just-bash.md) for step-by-step instructions.

## Links

- **Homepage**: [rustbash.dev](https://rustbash.dev)
- **Repository**: [github.com/shantanugoel/rust-bash](https://github.com/shantanugoel/rust-bash)
- **Recipes**: [docs/recipes](https://github.com/shantanugoel/rust-bash/tree/main/docs/recipes)
- **Guidebook**: [docs/guidebook](https://github.com/shantanugoel/rust-bash/tree/main/docs/guidebook)

## License

MIT
