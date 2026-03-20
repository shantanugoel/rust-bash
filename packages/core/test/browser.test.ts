/**
 * Tests for the browser entry point.
 *
 * Validates that the browser export surface is correct:
 * - Exports all browser-safe APIs (Bash, defineCommand, tool helpers, WASM loader)
 * - Does NOT export native-loader functions (tryLoadNative, isNativeAvailable, createNativeBackend)
 * - Exports the expected types
 */

import { describe, it, expect } from 'vitest';
import * as browser from '../src/browser.js';

describe('browser entry point exports', () => {
  it('should export the Bash class', () => {
    expect(browser.Bash).toBeDefined();
    expect(typeof browser.Bash).toBe('function');
  });

  it('should export defineCommand', () => {
    expect(browser.defineCommand).toBeDefined();
    expect(typeof browser.defineCommand).toBe('function');
  });

  it('should export tool definition helpers', () => {
    expect(browser.bashToolDefinition).toBeDefined();
    expect(browser.createBashToolHandler).toBeDefined();
    expect(browser.formatToolForProvider).toBeDefined();
    expect(browser.handleToolCall).toBeDefined();
    expect(browser.writeFileToolDefinition).toBeDefined();
    expect(browser.readFileToolDefinition).toBeDefined();
    expect(browser.listDirectoryToolDefinition).toBeDefined();
  });

  it('should export WASM loader functions', () => {
    expect(browser.initWasm).toBeDefined();
    expect(typeof browser.initWasm).toBe('function');

    expect(browser.isWasmInitialized).toBeDefined();
    expect(typeof browser.isWasmInitialized).toBe('function');

    expect(browser.createWasmBackend).toBeDefined();
    expect(typeof browser.createWasmBackend).toBe('function');
  });

  it('should NOT export native-loader functions', () => {
    const browserAny = browser as Record<string, unknown>;
    expect(browserAny.tryLoadNative).toBeUndefined();
    expect(browserAny.isNativeAvailable).toBeUndefined();
    expect(browserAny.createNativeBackend).toBeUndefined();
  });
});

describe('browser WASM loader state', () => {
  it('isWasmInitialized should return false before initialization', () => {
    expect(browser.isWasmInitialized()).toBe(false);
  });

  it('createWasmBackend should throw before initialization', () => {
    expect(() => browser.createWasmBackend({} as Record<string, string>)).toThrow(
      'WASM module not initialized',
    );
  });
});

describe('browser tool definitions', () => {
  it('bashToolDefinition should have correct structure', () => {
    const def = browser.bashToolDefinition;
    expect(def.name).toBe('bash');
    expect(def.inputSchema.type).toBe('object');
    expect(def.inputSchema.properties.command).toBeDefined();
    expect(def.inputSchema.required).toContain('command');
  });

  it('defineCommand should create a valid custom command', () => {
    const cmd = browser.defineCommand('test', async () => ({
      stdout: 'ok\n',
      stderr: '',
      exitCode: 0,
    }));

    expect(cmd.name).toBe('test');
    expect(typeof cmd.execute).toBe('function');
  });
});
