/**
 * WASM loader — loads the real rust-bash WASM binary.
 *
 * Uses the `--target web` output from wasm-bindgen.
 * The JS glue code is imported as a normal ES module (Vite resolves it).
 * The `.wasm` binary lives in `public/` so it's served as a static asset
 * in both dev and production modes.
 */

import init, { WasmBash } from '../../../pkg/rust_bash.js';

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface BashInstance {
  exec(command: string): Promise<ExecResult>;
  writeFile(path: string, content: string): void;
  readFile(path: string): string;
  getCwd(): string;
  getCommandNames(): string[];
  listDir(dir: string): string[];
}

let wasmReady: Promise<unknown> | null = null;

function ensureInit(): Promise<unknown> {
  if (!wasmReady) {
    // The WASM binary is in public/ — served at the root in both dev and prod.
    wasmReady = init({ module_or_path: '/rust_bash_bg.wasm' });
  }
  return wasmReady;
}

/**
 * Kick off WASM download early — call this as soon as possible.
 *
 * The key benefit is timing: calling this before any animations or other work
 * allows the fetch to run in parallel. When `createBash()` later calls
 * `ensureInit()` it will receive the same cached promise and resolve immediately
 * (or very quickly) instead of waiting for the full download + compile.
 */
export function preloadWasm(): void {
  ensureInit(); // starts the fetch; result is cached in wasmReady
}

export async function createBash(options: {
  files: Record<string, string>;
  cwd: string;
}): Promise<BashInstance> {
  await ensureInit();

  const instance = new WasmBash({
    files: options.files,
    cwd: options.cwd,
  });

  return {
    async exec(command: string): Promise<ExecResult> {
      const result = instance.exec(command) as {
        stdout: string;
        stderr: string;
        exitCode: number;
      };
      return {
        stdout: result.stdout,
        stderr: result.stderr,
        exitCode: result.exitCode,
      };
    },
    writeFile(path: string, content: string): void {
      instance.write_file(path, content);
    },
    readFile(path: string): string {
      return instance.read_file(path);
    },
    getCwd(): string {
      return instance.cwd();
    },
    getCommandNames(): string[] {
      return instance.command_names();
    },
    listDir(dir: string): string[] {
      const entries = instance.readdir(dir) as Array<{
        name: string;
        isDirectory: boolean;
      }>;
      return entries.map((e) =>
        e.isDirectory ? `${e.name}/` : e.name,
      );
    },
  };
}
