import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it, vi } from 'vitest';

const wasmBundlePath = fileURLToPath(new URL('../wasm/rust_bash.js', import.meta.url));
const hasBundledWasm = existsSync(wasmBundlePath);

describe.skipIf(!hasBundledWasm)('wasm loader', () => {
  it('loads the bundled wasm module and executes commands', async () => {
    vi.resetModules();

    const wasmLoader = await import('../src/wasm-loader.js');

    expect(wasmLoader.isWasmInitialized()).toBe(false);

    await wasmLoader.initWasm();

    expect(wasmLoader.isWasmInitialized()).toBe(true);

    const backend = wasmLoader.createWasmBackend({ '/hello.txt': 'hello from wasm\n' });
    const result = backend.exec('cat /hello.txt');

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toBe('hello from wasm\n');
  });
});
