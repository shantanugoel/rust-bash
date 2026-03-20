/* tslint:disable */
/* eslint-disable */

/**
 * A sandboxed bash interpreter for use from JavaScript.
 */
export class WasmBash {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Get the names of all registered commands.
     */
    command_names(): string[];
    /**
     * Get the current working directory.
     */
    cwd(): string;
    /**
     * Execute a shell command string.
     *
     * Returns `{ stdout: string, stderr: string, exitCode: number }`.
     */
    exec(command: string): any;
    /**
     * Execute a shell command with per-exec options.
     *
     * `options` is a JS object with optional fields:
     * - `env`: `Record<string, string>` — per-exec environment overrides
     * - `cwd`: `string` — per-exec working directory
     * - `stdin`: `string` — standard input content
     */
    exec_with_options(command: string, options: any): any;
    /**
     * Check whether a path exists in the virtual filesystem.
     */
    exists(path: string): boolean;
    /**
     * Get the exit code of the last executed command.
     */
    last_exit_code(): number;
    /**
     * Create a directory in the virtual filesystem.
     */
    mkdir(path: string, recursive: boolean): void;
    /**
     * Create a new WasmBash instance.
     *
     * `config` is a JS object with optional fields:
     * - `files`: `Record<string, string>` — seed virtual filesystem
     * - `env`: `Record<string, string>` — environment variables
     * - `cwd`: `string` — working directory (default: "/")
     * - `executionLimits`: partial execution limits
     */
    constructor(config: any);
    /**
     * Read a file from the virtual filesystem.
     */
    read_file(path: string): string;
    /**
     * List directory entries.
     *
     * Returns a JS array of `{ name: string, isDirectory: boolean }` objects.
     */
    readdir(path: string): any;
    /**
     * Register a custom command backed by a JavaScript callback.
     *
     * The callback receives `(args: string[], ctx: object)` and must return
     * `{ stdout: string, stderr: string, exitCode: number }` synchronously.
     *
     * The `ctx` object provides:
     * - `cwd: string` — current working directory
     * - `stdin: string` — piped input from the previous pipeline stage
     * - `env: Record<string, string>` — environment variables
     * - `fs` — virtual filesystem proxy (readFileSync, writeFileSync, …)
     * - `exec(command: string) → { stdout, stderr, exitCode }` — execute a
     *   sub-command through the shell interpreter.  **Must only be called
     *   synchronously** within the callback; do **not** store or defer it.
     */
    register_command(name: string, callback: Function): void;
    /**
     * Recursively remove a directory and its contents.
     */
    remove_dir_all(path: string): void;
    /**
     * Remove a file from the virtual filesystem.
     */
    remove_file(path: string): void;
    /**
     * Get metadata for a path.
     *
     * Returns `{ size: number, isDirectory: boolean, isFile: boolean, isSymlink: boolean }`.
     */
    stat(path: string): any;
    /**
     * Write a file to the virtual filesystem.
     */
    write_file(path: string, content: string): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmbash_free: (a: number, b: number) => void;
    readonly wasmbash_command_names: (a: number) => [number, number];
    readonly wasmbash_cwd: (a: number) => [number, number];
    readonly wasmbash_exec: (a: number, b: number, c: number) => [number, number, number];
    readonly wasmbash_exec_with_options: (a: number, b: number, c: number, d: any) => [number, number, number];
    readonly wasmbash_exists: (a: number, b: number, c: number) => number;
    readonly wasmbash_last_exit_code: (a: number) => number;
    readonly wasmbash_mkdir: (a: number, b: number, c: number, d: number) => [number, number];
    readonly wasmbash_new: (a: any) => [number, number, number];
    readonly wasmbash_read_file: (a: number, b: number, c: number) => [number, number, number, number];
    readonly wasmbash_readdir: (a: number, b: number, c: number) => [number, number, number];
    readonly wasmbash_register_command: (a: number, b: number, c: number, d: any) => [number, number];
    readonly wasmbash_remove_dir_all: (a: number, b: number, c: number) => [number, number];
    readonly wasmbash_remove_file: (a: number, b: number, c: number) => [number, number];
    readonly wasmbash_stat: (a: number, b: number, c: number) => [number, number, number];
    readonly wasmbash_write_file: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly wasm_bindgen__closure__destroy__h01f82033f57c3cd2: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h7a631768e569dc43: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h888e266ba1af605a: (a: number, b: number, c: number, d: number, e: any) => [number, number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h556d52e4eb39cbb5: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly wasm_bindgen__convert__closures_____invoke__ha3337c50045b27fb: (a: number, b: number, c: number, d: number) => any;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_drop_slice: (a: number, b: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
