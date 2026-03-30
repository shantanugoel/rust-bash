/**
 * Native addon loader for @shantanugoel/rust-bash.
 *
 * Loads the napi-rs native addon and provides a BashBackend implementation.
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

/** Interface matching the napi-rs NativeBash class. */
interface NativeBashConstructor {
  new (config: string): NativeBashInstance;
}

interface NativeBashInstance {
  exec(command: string): string;
  execWithOptions(command: string, optionsJson: string): string;
  writeFile(path: string, content: string): void;
  readFile(path: string): string;
  mkdir(path: string, recursive: boolean): void;
  exists(path: string): boolean;
  readdir(path: string): string[];
  stat(path: string): string;
  rm(path: string, recursive: boolean): void;
  getCwd(): string;
  getLastExitCode(): number;
  getCommandNames(): string[];
  registerCommand(
    name: string,
    callback: (argsJson: string) => string,
  ): void;
}

/** Native module shape. */
interface NativeModule {
  NativeBash: NativeBashConstructor;
}

let nativeModule: NativeModule | null = null;

/**
 * Try to load the native addon.
 * Returns true if native addon is available.
 */
export async function tryLoadNative(): Promise<boolean> {
  if (nativeModule) return true;

  try {
    // Use createRequire for ESM compatibility with native addons
    const { createRequire } = await import('node:module');
    const require = createRequire(import.meta.url);
    const mod = require('../native/rust-bash-native.node') as NativeModule;
    nativeModule = mod;
    return true;
  } catch {
    return false;
  }
}

/**
 * Check if the native addon is available.
 */
export function isNativeAvailable(): boolean {
  return nativeModule !== null;
}

/**
 * Create a native addon-backed BashBackend.
 */
export function createNativeBackend(
  files: Record<string, string>,
  options?: BashOptions,
): BashBackend {
  if (!nativeModule) {
    throw new Error('Native addon not loaded. Call tryLoadNative() first.');
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
    config.limits = options.executionLimits;
  }
  if (options?.network) {
    config.network = options.network;
  }

  const instance = new nativeModule.NativeBash(JSON.stringify(config));
  return new NativeBackend(instance);
}

class NativeBackend implements BashBackend {
  private instance: NativeBashInstance;

  constructor(instance: NativeBashInstance) {
    this.instance = instance;
  }

  exec(command: string): ExecResult {
    const json = this.instance.exec(command);
    return JSON.parse(json) as ExecResult;
  }

  execWithOptions(command: string, options: BackendExecOptions): ExecResult {
    const json = this.instance.execWithOptions(command, JSON.stringify(options));
    return JSON.parse(json) as ExecResult;
  }

  writeFile(path: string, content: string): void {
    this.instance.writeFile(path, content);
  }

  readFile(path: string): string {
    return this.instance.readFile(path);
  }

  mkdir(path: string, recursive: boolean): void {
    this.instance.mkdir(path, recursive);
  }

  exists(path: string): boolean {
    return this.instance.exists(path);
  }

  readdir(path: string): string[] {
    return this.instance.readdir(path);
  }

  stat(path: string): FileStat {
    const json = this.instance.stat(path);
    return JSON.parse(json) as FileStat;
  }

  rm(path: string, recursive: boolean): void {
    this.instance.rm(path, recursive);
  }

  getCwd(): string {
    return this.instance.getCwd();
  }

  getLastExitCode(): number {
    return this.instance.getLastExitCode();
  }

  getCommandNames(): string[] {
    return this.instance.getCommandNames();
  }

  registerCommand(
    name: string,
    callback: (args: string[], ctx: BackendCommandContext) => ExecResult,
  ): void {
    this.instance.registerCommand(name, (argsJson: string) => {
      const parsed = JSON.parse(argsJson) as {
        args: string[];
        ctx: {
          cwd: string;
          env: Record<string, string>;
          stdin: string;
        };
      };

      // Build a FileSystemProxy that delegates to this backend
      const fsProxy: FileSystemProxy = {
        readFileSync: (p: string) => this.readFile(p),
        writeFileSync: (p: string, c: string) => this.writeFile(p, c),
        existsSync: (p: string) => this.exists(p),
        mkdirSync: (p: string, opts?: { recursive?: boolean }) =>
          this.mkdir(p, opts?.recursive ?? false),
        readdirSync: (p: string) => this.readdir(p),
        statSync: (p: string) => this.stat(p),
        rmSync: (p: string, opts?: { recursive?: boolean }) =>
          this.rm(p, opts?.recursive ?? false),
      };

      const backendCtx: BackendCommandContext = {
        fs: fsProxy,
        cwd: parsed.ctx.cwd,
        env: parsed.ctx.env,
        stdin: parsed.ctx.stdin,
      };

      const result = callback(parsed.args, backendCtx);
      return JSON.stringify(result);
    });
  }
}
