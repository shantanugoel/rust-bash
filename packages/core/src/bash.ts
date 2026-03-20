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
  TransformPlugin,
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
 * Shell-escape a string for safe inclusion as a single argument.
 * Wraps in single quotes and escapes internal single quotes.
 */
function shellEscape(arg: string): string {
  return "'" + arg.replace(/'/g, "'\\''") + "'";
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
  private allowedCommands: string[] | null;
  private transformPlugins: TransformPlugin[] = [];
  private pendingLazyFiles: Map<string, Exclude<FileEntry, string>> = new Map();
  private lazyResolved = false;

  constructor(backend: BashBackend, options?: BashOptions) {
    this.backend = backend;
    this.customCommands = options?.customCommands ?? [];
    this.allowedCommands = options?.commands ?? null;

    // Build the FileSystemProxy that delegates to the backend
    // with lazy file interception
    this.fs = this.buildFsProxy();
  }

  /**
   * Create a Bash instance with resolved files and configured backend.
   * This is the primary factory — it handles lazy file resolution.
   *
   * Eager (string) files are written to the backend immediately.
   * Lazy files are deferred until first `exec()` or `readFile()` call.
   */
  static async create(
    createBackend: (files: Record<string, string>, options?: BashOptions) => BashBackend,
    options?: BashOptions,
  ): Promise<Bash> {
    // Separate eager files from lazy files
    const eagerFiles: Record<string, string> = {};
    const lazyFiles = new Map<string, Exclude<FileEntry, string>>();

    if (options?.files) {
      for (const [path, entry] of Object.entries(options.files)) {
        if (typeof entry === 'string') {
          eagerFiles[path] = entry;
        } else {
          lazyFiles.set(path, entry);
        }
      }
    }

    // Create backend with only eager files
    const backend = createBackend(eagerFiles, options);

    const bash = new Bash(backend, options);
    bash.pendingLazyFiles = lazyFiles;
    bash.registerCustomCommands();

    return bash;
  }

  /**
   * Register a transform plugin that processes scripts before execution.
   * Plugins are applied in registration order, after script normalization.
   */
  registerTransformPlugin(plugin: TransformPlugin): void {
    this.transformPlugins.push(plugin);
  }

  /**
   * Execute a bash command string.
   *
   * The command is normalized (leading whitespace stripped from template
   * literals) unless `options.rawScript` is true. Transform plugins are
   * applied after normalization.
   */
  async exec(command: string, options?: ExecOptions): Promise<ExecResult> {
    // Materialize any pending lazy files before first exec
    await this.materializeLazyFiles();

    let script = options?.rawScript ? command : normalizeScript(command);

    // Apply transform plugins
    for (const plugin of this.transformPlugins) {
      script = plugin.transform(script);
    }

    // Append safe args if provided (bypass shell parsing)
    if (options?.args && options.args.length > 0) {
      const escaped = options.args.map(shellEscape).join(' ');
      script = `${script} ${escaped}`;
    }

    // Check command allow-list (first-word heuristic — not a security boundary).
    // Compound commands (e.g., "echo ok; rm foo") only check the first word.
    // Use execution limits for real sandboxing.
    if (this.allowedCommands) {
      const firstWord = script.trimStart().split(/[\s;|&<>()]/)[0];
      if (firstWord && !this.allowedCommands.includes(firstWord)) {
        return {
          stdout: '',
          stderr: `${firstWord}: command not allowed\n`,
          exitCode: 127,
        };
      }
    }

    const backendOptions: {
      env?: Record<string, string>;
      replaceEnv?: boolean;
      cwd?: string;
      stdin?: string;
    } = {};

    if (options?.env) {
      backendOptions.env = options.env;
    }
    if (options?.replaceEnv) {
      backendOptions.replaceEnv = true;
    }
    if (options?.cwd) {
      backendOptions.cwd = options.cwd;
    }
    if (options?.stdin) {
      backendOptions.stdin = options.stdin;
    }

    const hasOptions = backendOptions.env || backendOptions.cwd || backendOptions.stdin || backendOptions.replaceEnv;

    let result: ExecResult;
    if (hasOptions) {
      result = this.backend.execWithOptions(script, backendOptions);
    } else {
      result = this.backend.exec(script);
    }

    return result;
  }

  /** Write a file to the virtual filesystem. Cancels lazy loading for this path. */
  writeFile(path: string, content: string): void {
    // Write-before-read: if the file was lazy, the callback is never invoked
    this.pendingLazyFiles.delete(path);
    this.backend.writeFile(path, content);
  }

  /** Read a file from the virtual filesystem. Resolves lazy files on demand. */
  readFile(path: string): string {
    // If this path has a pending lazy file, resolve it synchronously if possible
    const lazyEntry = this.pendingLazyFiles.get(path);
    if (lazyEntry) {
      // Try synchronous resolution
      const result = lazyEntry();
      if (typeof result === 'string') {
        this.backend.writeFile(path, result);
        this.pendingLazyFiles.delete(path);
      } else {
        // Async lazy file — can't resolve synchronously from readFile
        throw new Error(
          `readFile("${path}"): file has an async lazy loader. ` +
            'Use exec() (which materializes async files) or await the file manually.',
        );
      }
    }
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
    // eslint-disable-next-line @typescript-eslint/no-this-alias
    const bash = this;
    const backend = this.backend;
    return {
      readFileSync(path: string): string {
        return bash.readFile(path);
      },
      writeFileSync(path: string, content: string): void {
        bash.writeFile(path, content);
      },
      existsSync(path: string): boolean {
        if (bash.pendingLazyFiles.has(path)) return true;
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
        bash.pendingLazyFiles.delete(path);
        backend.rm(path, options?.recursive ?? false);
      },
    };
  }

  /**
   * Resolve all pending lazy files and write them to the backend.
   * Called once before the first exec().
   */
  private async materializeLazyFiles(): Promise<void> {
    if (this.lazyResolved || this.pendingLazyFiles.size === 0) {
      return;
    }
    this.lazyResolved = true;

    const entries = Array.from(this.pendingLazyFiles.entries());
    const results = await Promise.all(
      entries.map(async ([path, loader]) => ({
        path,
        content: await resolveFileEntry(loader),
      })),
    );

    for (const { path, content } of results) {
      // Only write if not already removed by a writeFile call
      if (this.pendingLazyFiles.has(path)) {
        this.backend.writeFile(path, content);
        this.pendingLazyFiles.delete(path);
      }
    }
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
