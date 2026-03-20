/**
 * Bash class — backend-agnostic wrapper around WASM or native addon.
 *
 * Provides a TypeScript-first API for the rust-bash interpreter with
 * lazy file loading, custom commands, and per-exec isolation.
 */

import type {
  BashOptions,
  BashBackend,
  ExecOptions,
  ExecResult,
  FileEntry,
  FileSystemProxy,
  FileStat,
  CustomCommand,
  CommandContext,
  BackendCommandContext,
} from './types.js';

/**
 * Resolve a `FileEntry` to its string content.
 * Handles eager strings, lazy sync functions, and lazy async functions.
 */
async function resolveFileEntry(entry: FileEntry): Promise<string> {
  if (typeof entry === 'string') {
    return entry;
  }
  return await entry();
}

/**
 * Strip common leading whitespace from a multi-line script string.
 * Preserves relative indentation. Useful for template literals.
 */
function normalizeScript(script: string): string {
  const lines = script.split('\n');

  // Remove leading empty line (common with template literals)
  if (lines.length > 0 && lines[0]!.trim() === '') {
    lines.shift();
  }
  // Remove trailing empty line
  if (lines.length > 0 && lines[lines.length - 1]!.trim() === '') {
    lines.pop();
  }

  if (lines.length === 0) return '';

  // Find minimum indentation across non-empty lines
  let minIndent = Infinity;
  for (const line of lines) {
    if (line.trim() === '') continue;
    const match = line.match(/^(\s*)/);
    if (match) {
      minIndent = Math.min(minIndent, match[1]!.length);
    }
  }

  if (minIndent === Infinity || minIndent === 0) {
    return lines.join('\n');
  }

  return lines.map((line) => line.slice(minIndent)).join('\n');
}

export class Bash {
  /** Direct VFS access proxy. */
  readonly fs: FileSystemProxy;

  private backend: BashBackend;
  private customCommands: CustomCommand[];

  constructor(backend: BashBackend, options?: BashOptions) {
    this.backend = backend;
    this.customCommands = options?.customCommands ?? [];

    // Build the FileSystemProxy that delegates to the backend
    this.fs = this.buildFsProxy();
  }

  /**
   * Create a Bash instance with resolved files and configured backend.
   * This is the primary factory — it handles lazy file resolution.
   */
  static async create(
    createBackend: (files: Record<string, string>, options?: BashOptions) => BashBackend,
    options?: BashOptions,
  ): Promise<Bash> {
    // Resolve lazy files
    const resolvedFiles: Record<string, string> = {};
    if (options?.files) {
      const entries = Object.entries(options.files);
      const results = await Promise.all(
        entries.map(async ([path, entry]) => ({
          path,
          content: await resolveFileEntry(entry),
        })),
      );
      for (const { path, content } of results) {
        resolvedFiles[path] = content;
      }
    }

    const backend = createBackend(resolvedFiles, options);

    // Register custom commands
    const bash = new Bash(backend, options);
    bash.registerCustomCommands();

    return bash;
  }

  /**
   * Execute a bash command string.
   *
   * The command is normalized (leading whitespace stripped from template
   * literals) unless `options.rawScript` is true.
   */
  async exec(command: string, options?: ExecOptions): Promise<ExecResult> {
    const script = options?.rawScript ? command : normalizeScript(command);

    const backendOptions: {
      env?: Record<string, string>;
      cwd?: string;
      stdin?: string;
    } = {};

    if (options?.env) {
      backendOptions.env = options.env;
      // When replaceEnv is true, we pass a flag so the backend clears
      // existing env vars before applying the new ones.
      // For now, the backend merges by default. The TypeScript layer
      // handles replaceEnv by getting all current env vars and unsetting
      // them, which would require additional backend support.
      // TODO: Wire replaceEnv through BackendExecOptions when backends support it.
    }
    if (options?.cwd) {
      backendOptions.cwd = options.cwd;
    }
    if (options?.stdin) {
      backendOptions.stdin = options.stdin;
    }

    const hasOptions = backendOptions.env || backendOptions.cwd || backendOptions.stdin;

    let result: ExecResult;
    if (hasOptions) {
      result = this.backend.execWithOptions(script, backendOptions);
    } else {
      result = this.backend.exec(script);
    }

    return result;
  }

  /** Write a file to the virtual filesystem. */
  writeFile(path: string, content: string): void {
    this.backend.writeFile(path, content);
  }

  /** Read a file from the virtual filesystem. */
  readFile(path: string): string {
    return this.backend.readFile(path);
  }

  /** Get the current working directory. */
  getCwd(): string {
    return this.backend.getCwd();
  }

  /** Get names of all registered commands. */
  getCommandNames(): string[] {
    return this.backend.getCommandNames();
  }

  private buildFsProxy(): FileSystemProxy {
    const backend = this.backend;
    return {
      readFileSync(path: string): string {
        return backend.readFile(path);
      },
      writeFileSync(path: string, content: string): void {
        backend.writeFile(path, content);
      },
      existsSync(path: string): boolean {
        return backend.exists(path);
      },
      mkdirSync(path: string, options?: { recursive?: boolean }): void {
        backend.mkdir(path, options?.recursive ?? false);
      },
      readdirSync(path: string): string[] {
        return backend.readdir(path);
      },
      statSync(path: string): FileStat {
        return backend.stat(path);
      },
      rmSync(path: string, options?: { recursive?: boolean }): void {
        backend.rm(path, options?.recursive ?? false);
      },
    };
  }

  private registerCustomCommands(): void {
    for (const cmd of this.customCommands) {
      // Wrap the async execute fn into a sync callback for the backend.
      // On WASM, custom commands must be synchronous — the async wrapper
      // is only useful for the native addon which uses ThreadsafeFunction.
      // Here we create a sync shim that executes the function and returns
      // a default result if it returns a Promise (WASM limitation).
      const bash = this;
      const execute = cmd.execute;

      this.backend.registerCommand(
        cmd.name,
        (args: string[], backendCtx: BackendCommandContext): ExecResult => {
          // Build the full CommandContext with exec capability
          const ctx: CommandContext = {
            fs: backendCtx.fs,
            cwd: backendCtx.cwd,
            env: backendCtx.env,
            stdin: backendCtx.stdin,
            exec: async (command: string, execOpts?: { cwd?: string; stdin?: string }) => {
              return bash.exec(command, execOpts);
            },
          };

          // Try to execute synchronously
          let result: ExecResult | Promise<ExecResult>;
          try {
            result = execute(args, ctx) as ExecResult | Promise<ExecResult>;
          } catch (err: unknown) {
            const message = err instanceof Error ? err.message : String(err);
            return {
              stdout: '',
              stderr: `${cmd.name}: ${message}\n`,
              exitCode: 1,
            };
          }

          // If the result is a Promise, we cannot await it in sync context (WASM).
          // Attach a catch handler to prevent unhandled rejections, then return an error.
          if (result && typeof (result as Promise<ExecResult>).then === 'function') {
            (result as Promise<ExecResult>).catch(() => {
              // Swallow — we already return an error below
            });
            return {
              stdout: '',
              stderr: `${cmd.name}: async custom commands are not supported in WASM backend\n`,
              exitCode: 1,
            };
          }

          return result as ExecResult;
        },
      );
    }
  }
}
