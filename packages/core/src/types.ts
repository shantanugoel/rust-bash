/**
 * TypeScript types for @rust-bash/core.
 */

// ── File System Types ────────────────────────────────────────────────

/** A file value that is written immediately. */
export type EagerFile = string;

/** A file value resolved synchronously on first read. */
export type LazySyncFile = () => string;

/** A file value resolved asynchronously on first read. */
export type LazyAsyncFile = () => Promise<string>;

/** A file entry in the `files` option — eager string, lazy sync, or lazy async. */
export type FileEntry = EagerFile | LazySyncFile | LazyAsyncFile;

/** File stat information. */
export interface FileStat {
  isFile: boolean;
  isDirectory: boolean;
  size: number;
}

/** Proxy for direct VFS access from TypeScript. */
export interface FileSystemProxy {
  readFileSync(path: string): string;
  writeFileSync(path: string, content: string): void;
  existsSync(path: string): boolean;
  mkdirSync(path: string, options?: { recursive?: boolean }): void;
  readdirSync(path: string): string[];
  statSync(path: string): FileStat;
  rmSync(path: string, options?: { recursive?: boolean }): void;
}

// ── Execution Types ──────────────────────────────────────────────────

/** Execution limits for the bash interpreter. */
export interface ExecutionLimits {
  maxCommandCount: number;
  maxExecutionTimeSecs: number;
  maxLoopIterations: number;
  maxOutputSize: number;
  maxCallDepth: number;
  maxStringLength: number;
  maxGlobResults: number;
  maxSubstitutionDepth: number;
  maxHeredocSize: number;
  maxBraceExpansion: number;
}

/** Network configuration. */
export interface NetworkConfig {
  enabled: boolean;
  allowedUrlPrefixes?: string[];
  allowedMethods?: string[];
  maxResponseSize?: number;
  maxRedirects?: number;
  timeoutSecs?: number;
}

/** Result of executing a bash command. */
export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  env?: Record<string, string>;
}

/** Options for `Bash.exec()`. */
export interface ExecOptions {
  /** Per-exec environment variable overrides (merged with instance env). */
  env?: Record<string, string>;
  /** Per-exec working directory override. */
  cwd?: string;
  /** Standard input content. */
  stdin?: string;
  /** If true, skip script normalization (leading whitespace stripping). */
  rawScript?: boolean;
}

/** Options for constructing a `Bash` instance. */
export interface BashOptions {
  /** Seed the virtual filesystem with files. Values can be eager or lazy. */
  files?: Record<string, FileEntry>;
  /** Environment variables. */
  env?: Record<string, string>;
  /** Initial working directory (default: "/"). */
  cwd?: string;
  /** Execution limits. */
  executionLimits?: Partial<ExecutionLimits>;
  /** Custom commands. */
  customCommands?: CustomCommand[];
  /** Network configuration. */
  network?: NetworkConfig;
}

// ── Custom Command Types ─────────────────────────────────────────────

/** Context passed to custom command execute functions. */
export interface CommandContext {
  fs: FileSystemProxy;
  cwd: string;
  env: Record<string, string>;
  stdin: string;
  exec: (command: string, options?: { cwd?: string; stdin?: string }) => Promise<ExecResult>;
}

/** A custom command definition. */
export interface CustomCommand {
  name: string;
  execute: (args: string[], ctx: CommandContext) => Promise<ExecResult>;
}

// ── Backend Interface ────────────────────────────────────────────────

/**
 * Internal interface for backend implementations (WASM or native addon).
 * Not exported to consumers.
 */
export interface BashBackend {
  exec(command: string): ExecResult;
  execWithOptions(command: string, options: BackendExecOptions): ExecResult;
  writeFile(path: string, content: string): void;
  readFile(path: string): string;
  mkdir(path: string, recursive: boolean): void;
  exists(path: string): boolean;
  readdir(path: string): string[];
  stat(path: string): FileStat;
  rm(path: string, recursive: boolean): void;
  getCwd(): string;
  getLastExitCode(): number;
  getCommandNames(): string[];
  registerCommand(
    name: string,
    callback: (args: string[], ctx: BackendCommandContext) => ExecResult,
  ): void;
}

/** Backend-level exec options (after TS-layer normalization). */
export interface BackendExecOptions {
  env?: Record<string, string>;
  cwd?: string;
  stdin?: string;
}

/** Backend-level command context (simplified, sync-only). */
export interface BackendCommandContext {
  fs: FileSystemProxy;
  cwd: string;
  env: Record<string, string>;
  stdin: string;
}

// ── Tool Types ───────────────────────────────────────────────────────

/** JSON Schema tool definition for AI SDK integration. */
export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: {
    type: 'object';
    properties: Record<string, { type: string; description: string }>;
    required: string[];
  };
}

/** Options for creating a bash tool handler. */
export interface BashToolOptions extends BashOptions {
  /** Maximum output length before truncation. */
  maxOutputLength?: number;
}

/** Result of creating a bash tool handler. */
export interface BashToolHandler {
  handler: (args: { command: string }) => Promise<ExecResult>;
  definition: ToolDefinition;
  bash: import('./bash.js').Bash;
}

/** Supported AI provider formats. */
export type ToolProvider = 'openai' | 'anthropic' | 'mcp';
