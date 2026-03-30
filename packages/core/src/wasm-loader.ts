/**
 * WASM module loader for @shantanugoel/rust-bash.
 *
 * Loads the rust-bash WASM binary and provides a BashBackend implementation.
 */

import type {
  BashBackend,
  BashOptions,
  ExecResult,
  FileStat,
  BackendExecOptions,
  BackendCommandContext,
  FileSystemProxy,
} from './types.js';

/** Interface matching the wasm-bindgen WasmBash class. */
interface WasmBashModule {
  new (config: Record<string, unknown>): WasmBashInstance;
}

interface WasmBashInstance {
  exec(command: string): { stdout: string; stderr: string; exitCode: number };
  exec_with_options(
    command: string,
    options: Record<string, unknown>,
  ): { stdout: string; stderr: string; exitCode: number };
  write_file(path: string, content: string): void;
  read_file(path: string): string;
  mkdir(path: string, recursive: boolean): void;
  exists(path: string): boolean;
  readdir(path: string): { name: string; isDirectory: boolean }[];
  stat(path: string): {
    size: number;
    isDirectory: boolean;
    isFile: boolean;
    isSymlink: boolean;
  };
  remove_file(path: string): void;
  remove_dir_all(path: string): void;
  cwd(): string;
  last_exit_code(): number;
  command_names(): string[];
  register_command(
    name: string,
    callback: (args: string[], ctx: Record<string, unknown>) => {
      stdout: string;
      stderr: string;
      exitCode: number;
    },
  ): void;
}

/** WASM module initialization result. */
interface WasmModule {
  WasmBash: WasmBashModule;
}

let wasmModule: WasmModule | null = null;

/**
 * Initialize the WASM module.
 * Must be called before creating WasmBackend instances.
 */
export async function initWasm(module?: WasmModule): Promise<void> {
  if (module) {
    wasmModule = module;
    return;
  }

  // Dynamic import of the WASM package
  // The actual path depends on build configuration
  try {
    // Dynamic import of the WASM package — path resolved at runtime.
    // The wasm/ directory contains build artifacts from scripts/build-wasm.sh.
    const wasmPath = '../../wasm/rust_bash.js';
    const mod = (await import(wasmPath)) as unknown as WasmModule;
    wasmModule = mod;
  } catch {
    throw new Error(
      'Failed to load WASM module. Ensure the WASM build artifacts are available in the wasm/ directory.',
    );
  }
}

/** Check if WASM module is initialized. */
export function isWasmInitialized(): boolean {
  return wasmModule !== null;
}

/**
 * Create a WASM-backed BashBackend.
 */
export function createWasmBackend(
  files: Record<string, string>,
  options?: BashOptions,
): BashBackend {
  if (!wasmModule) {
    throw new Error('WASM module not initialized. Call initWasm() first.');
  }

  const config: Record<string, unknown> = {};
  if (Object.keys(files).length > 0) {
    config.files = files;
  }
  if (options?.env) {
    config.env = options.env;
  }
  if (options?.cwd) {
    config.cwd = options.cwd;
  }
  if (options?.executionLimits) {
    config.executionLimits = options.executionLimits;
  }

  const instance = new wasmModule.WasmBash(config);

  return new WasmBackend(instance);
}

class WasmBackend implements BashBackend {
  private instance: WasmBashInstance;

  constructor(instance: WasmBashInstance) {
    this.instance = instance;
  }

  exec(command: string): ExecResult {
    const result = this.instance.exec(command);
    return {
      stdout: result.stdout,
      stderr: result.stderr,
      exitCode: result.exitCode,
    };
  }

  execWithOptions(command: string, options: BackendExecOptions): ExecResult {
    const opts: Record<string, unknown> = {};
    if (options.env) opts.env = options.env;
    if (options.replaceEnv) opts.replaceEnv = true;
    if (options.cwd) opts.cwd = options.cwd;
    if (options.stdin) opts.stdin = options.stdin;

    const result = this.instance.exec_with_options(command, opts);
    return {
      stdout: result.stdout,
      stderr: result.stderr,
      exitCode: result.exitCode,
    };
  }

  writeFile(path: string, content: string): void {
    this.instance.write_file(path, content);
  }

  readFile(path: string): string {
    return this.instance.read_file(path);
  }

  mkdir(path: string, recursive: boolean): void {
    this.instance.mkdir(path, recursive);
  }

  exists(path: string): boolean {
    return this.instance.exists(path);
  }

  readdir(path: string): string[] {
    const entries = this.instance.readdir(path);
    return entries.map((e) => e.name);
  }

  stat(path: string): FileStat {
    const meta = this.instance.stat(path);
    return {
      isFile: meta.isFile,
      isDirectory: meta.isDirectory,
      size: meta.size,
    };
  }

  rm(path: string, recursive: boolean): void {
    if (recursive) {
      const meta = this.instance.stat(path);
      if (meta.isDirectory) {
        this.instance.remove_dir_all(path);
      } else {
        this.instance.remove_file(path);
      }
    } else {
      this.instance.remove_file(path);
    }
  }

  getCwd(): string {
    return this.instance.cwd();
  }

  getLastExitCode(): number {
    return this.instance.last_exit_code();
  }

  getCommandNames(): string[] {
    return this.instance.command_names();
  }

  registerCommand(
    name: string,
    callback: (args: string[], ctx: BackendCommandContext) => ExecResult,
  ): void {
    this.instance.register_command(
      name,
      (args: string[], ctx: Record<string, unknown>) => {
        const backendCtx: BackendCommandContext = {
          fs: ctx.fs as FileSystemProxy,
          cwd: ctx.cwd as string,
          env: ctx.env as Record<string, string>,
          stdin: ctx.stdin as string,
        };
        const result = callback(args, backendCtx);
        return {
          stdout: result.stdout,
          stderr: result.stderr,
          exitCode: result.exitCode,
        };
      },
    );
  }
}
