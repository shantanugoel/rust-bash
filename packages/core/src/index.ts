/**
 * Main entry point for @rust-bash/core (Node.js).
 *
 * Auto-detects backend: tries native addon first, falls back to WASM.
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
export { tryLoadNative, isNativeAvailable, createNativeBackend } from './native-loader.js';

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
