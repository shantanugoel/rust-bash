# WASM Usage

## Goal

Build and run rust-bash in the browser (or any WASM-capable runtime) via WebAssembly. This covers building from source, using the low-level `wasm-bindgen` API, and using the recommended `rust-bash` npm package.

## Building WASM from Source

The `scripts/build-wasm.sh` script handles the full pipeline:

```bash
./scripts/build-wasm.sh
```

This script:
1. Installs the `wasm32-unknown-unknown` target via `rustup`
2. Builds with `--features wasm --no-default-features`
3. Runs `wasm-bindgen` to generate JS bindings in `pkg/`
4. Optionally runs `wasm-opt -Oz` for size optimization

Or build manually:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli

cargo build \
    --target wasm32-unknown-unknown \
    --features wasm \
    --no-default-features \
    --release

wasm-bindgen \
    target/wasm32-unknown-unknown/release/rust_bash.wasm \
    --out-dir pkg/ \
    --target bundler

# Optional: optimize with Binaryen
wasm-opt pkg/rust_bash_bg.wasm -Oz -o pkg/rust_bash_bg.wasm
```

Output files in `pkg/`:
- `rust_bash.js` — JavaScript bindings
- `rust_bash_bg.wasm` — WebAssembly binary
- `rust_bash.d.ts` — TypeScript declarations

## Low-Level wasm-bindgen API

The WASM module exports a `WasmBash` class that you can use directly:

```typescript
import init, { WasmBash } from './pkg/rust_bash.js';

// Initialize the WASM module
await init();

// Create an instance with configuration
const bash = new WasmBash({
  files: { '/data.txt': 'hello world' },
  env: { USER: 'agent' },
  cwd: '/home/user',
  executionLimits: {
    maxCommandCount: 10000,
    maxExecutionTimeSecs: 30,
  },
});

// Execute a command
const result = bash.exec('cat /data.txt | grep hello');
// result: { stdout: "hello world\n", stderr: "", exitCode: 0 }

// Execute with per-call options
const result2 = bash.exec_with_options('echo $PWD', {
  env: { EXTRA: 'value' },
  cwd: '/tmp',
  stdin: 'piped input',
});

// Filesystem operations
bash.write_file('/output.txt', 'content');
const content = bash.read_file('/output.txt');
bash.mkdir('/dir', true); // recursive
const exists = bash.exists('/dir');
const entries = bash.readdir('/');
// entries: [{ name: "dir", isDirectory: true }, { name: "data.txt", isDirectory: false }]

// State queries
console.log(bash.cwd());            // "/home/user"
console.log(bash.last_exit_code()); // 0
console.log(bash.command_names());  // ["echo", "cat", "grep", ...]

// Register a custom command
bash.register_command('greet', (args, ctx) => {
  return { stdout: `Hello, ${args[0]}!\n`, stderr: '', exitCode: 0 };
});
```

## Recommended: Using rust-bash (npm)

The `rust-bash` package wraps the low-level API with a high-level `Bash` class:

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();

const bash = await Bash.create(createWasmBackend, {
  files: {
    '/index.html': '<h1>Hello</h1>',
    '/data.json': '{"count": 42}',
  },
  env: { USER: 'browser-user' },
  cwd: '/',
});

const result = await bash.exec('cat /data.json | jq .count');
console.log(result.stdout);   // "42\n"
console.log(result.exitCode); // 0
```

See [npm Package](npm-package.md) for the full API reference.

## Browser Integration with Vite

```typescript
// vite.config.ts
import { defineConfig } from 'vite';

export default defineConfig({
  optimizeDeps: {
    exclude: ['rust-bash'],
  },
});
```

```typescript
// src/shell.ts
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

let bashInstance: Bash | null = null;

export async function getShell(): Promise<Bash> {
  if (!bashInstance) {
    await initWasm();
    bashInstance = await Bash.create(createWasmBackend, {
      files: { '/workspace/README.md': '# My Project' },
      cwd: '/workspace',
    });
  }
  return bashInstance;
}

// Usage in a component
const shell = await getShell();
const result = await shell.exec('ls /workspace');
```

## Browser Integration with webpack

```javascript
// webpack.config.js
module.exports = {
  experiments: {
    asyncWebAssembly: true,
  },
};
```

```typescript
// src/index.ts
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

async function main() {
  await initWasm();

  const bash = await Bash.create(createWasmBackend, {
    files: { '/hello.txt': 'Hello from WASM!' },
  });

  const result = await bash.exec('cat /hello.txt');
  document.getElementById('output')!.textContent = result.stdout;
}

main();
```

## WASM-Specific Limitations

| Limitation | Details |
|-----------|---------|
| No real `sleep` | `sleep` returns an error: "sleep: not supported in browser environment" |
| No network access | `curl`/`wget` are unavailable — use custom commands to bridge to `fetch()` |
| No threads | WASM target `wasm32-unknown-unknown` is single-threaded |
| Sync-only custom commands | `register_command()` callbacks on the low-level API must be synchronous |
| No host filesystem | Only `InMemoryFs` is available — no `OverlayFs` or `ReadWriteFs` |
| Time handling | `std::time` is replaced by `web-time` crate; `chrono` uses the `wasmbind` feature |

Using `rust-bash` mitigates some of these: custom commands via `defineCommand()` support async callbacks, and the `Bash` class handles lazy file loading with `async` functions.

## Bundle Size

The WASM binary is approximately **1–1.5 MB gzipped** (before `wasm-opt`). After `wasm-opt -Oz`, expect a further 10–20% reduction.

| Stage | Approximate Size |
|-------|-----------------|
| Raw `.wasm` | ~3–4 MB |
| After `wasm-opt -Oz` | ~2.5–3.5 MB |
| Gzipped | ~1–1.5 MB |

Tips to manage bundle size:
- Use `wasm-opt -Oz` (included in `build-wasm.sh`)
- Enable gzip/brotli compression on your server
- Lazy-load the WASM module (call `initWasm()` only when needed)

## Example: Interactive Browser Shell

```html
<!DOCTYPE html>
<html>
<head><title>rust-bash WASM Demo</title></head>
<body>
  <textarea id="script" rows="5" cols="60">echo "Hello from WASM!"
ls /
cat /data.json | jq .name</textarea>
  <button id="run">Run</button>
  <pre id="output"></pre>

  <script type="module">
    import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

    await initWasm();
    const bash = await Bash.create(createWasmBackend, {
      files: {
        '/data.json': '{"name": "rust-bash", "version": "0.1.0"}',
      },
    });

    document.getElementById('run').addEventListener('click', async () => {
      const script = document.getElementById('script').value;
      const result = await bash.exec(script);

      let output = '';
      if (result.stdout) output += result.stdout;
      if (result.stderr) output += `[stderr] ${result.stderr}`;
      output += `\n[exit code: ${result.exitCode}]`;

      document.getElementById('output').textContent = output;
    });
  </script>
</body>
</html>
```

## Next Steps

- [npm Package](npm-package.md) — full TypeScript API, Node.js + browser setup
- [Convenience API](convenience-api.md) — command filtering, transform plugins, safe args
- [Custom Commands](custom-commands.md) — bridge browser APIs (fetch, localStorage) into shell scripts
