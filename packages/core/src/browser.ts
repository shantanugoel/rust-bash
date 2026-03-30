/**
 * Browser entry point for rust-bash.
 *
 * WASM-only — does not attempt to load the native addon.
 */

export { Bash } from './bash.js';
export { defineCommand } from './custom-commands.js';
export {
  bashToolDefinition,
  createBashToolHandler,
  formatToolForProvider,
  handleToolCall,
  writeFileToolDefinition,
  readFileToolDefinition,
  listDirectoryToolDefinition,
} from './tool.js';
export { initWasm, isWasmInitialized, createWasmBackend } from './wasm-loader.js';

export type {
  BashOptions,
  ExecOptions,
  ExecResult,
  ExecutionLimits,
  NetworkConfig,
  FileEntry,
  FileSystemProxy,
  FileStat,
  CustomCommand,
  CommandContext,
  ToolDefinition,
  BashToolOptions,
  BashToolHandler,
  ToolProvider,
  TransformPlugin,
} from './types.js';
