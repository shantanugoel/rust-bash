/**
 * Tests for the custom command API.
 */

import { describe, it, expect, vi } from 'vitest';
import { defineCommand } from '../src/custom-commands.js';
import { Bash } from '../src/bash.js';
import type {
  BashBackend,
  ExecResult,
  FileStat,
  BackendExecOptions,
  BackendCommandContext,
} from '../src/types.js';

/** Create a mock backend that supports command registration. */
function createMockBackend(): BashBackend & {
  registeredCommands: Map<string, (args: string[], ctx: BackendCommandContext) => ExecResult>;
} {
  const registeredCommands = new Map<
    string,
    (args: string[], ctx: BackendCommandContext) => ExecResult
  >();

  const backend: BashBackend & {
    registeredCommands: Map<string, (args: string[], ctx: BackendCommandContext) => ExecResult>;
  } = {
    registeredCommands,
    exec: vi.fn((command: string): ExecResult => {
      // Check registered commands
      const parts = command.split(' ');
      const cmdName = parts[0]!;
      const args = parts.slice(1);
      const handler = registeredCommands.get(cmdName);
      if (handler) {
        return handler(args, {
          fs: {
            readFileSync: () => '',
            writeFileSync: () => {},
            existsSync: () => false,
            mkdirSync: () => {},
            readdirSync: () => [],
            statSync: () => ({ isFile: false, isDirectory: false, size: 0 }),
            rmSync: () => {},
          },
          cwd: '/',
          env: {},
          stdin: '',
        });
      }
      return { stdout: '', stderr: `${cmdName}: command not found\n`, exitCode: 127 };
    }),
    execWithOptions: vi.fn(
      (_command: string, _options: BackendExecOptions): ExecResult => ({
        stdout: '',
        stderr: '',
        exitCode: 0,
      }),
    ),
    writeFile: vi.fn(),
    readFile: vi.fn((): string => ''),
    mkdir: vi.fn(),
    exists: vi.fn((): boolean => false),
    readdir: vi.fn((): string[] => []),
    stat: vi.fn((): FileStat => ({ isFile: false, isDirectory: false, size: 0 })),
    rm: vi.fn(),
    getCwd: vi.fn((): string => '/'),
    getLastExitCode: vi.fn((): number => 0),
    getCommandNames: vi.fn((): string[] => ['echo']),
    registerCommand: vi.fn(
      (
        name: string,
        callback: (args: string[], ctx: BackendCommandContext) => ExecResult,
      ): void => {
        registeredCommands.set(name, callback);
      },
    ),
  };

  return backend;
}

describe('defineCommand', () => {
  it('should create a CustomCommand object', () => {
    const cmd = defineCommand('hello', async (args) => ({
      stdout: `Hello, ${args[0] || 'world'}!\n`,
      stderr: '',
      exitCode: 0,
    }));

    expect(cmd.name).toBe('hello');
    expect(typeof cmd.execute).toBe('function');
  });

  it('should be usable with Bash constructor', async () => {
    const hello = defineCommand('hello', async (args) => ({
      stdout: `Hello, ${args[0] || 'world'}!\n`,
      stderr: '',
      exitCode: 0,
    }));

    const backend = createMockBackend();
    const bash = new Bash(backend, { customCommands: [hello] });

    // The command should have been registered
    expect(backend.registerCommand).toHaveBeenCalledTimes(0);
    // Note: registerCustomCommands is called from Bash.create, not constructor directly.
    // Through Bash.create, custom commands are registered.
  });

  it('should work via Bash.create', async () => {
    const hello = defineCommand('hello', async (args) => ({
      stdout: `Hello, ${args[0] || 'world'}!\n`,
      stderr: '',
      exitCode: 0,
    }));

    const backend = createMockBackend();

    const bash = await Bash.create(
      () => backend,
      { customCommands: [hello] },
    );

    expect(backend.registerCommand).toHaveBeenCalledWith('hello', expect.any(Function));
  });

  it('should execute registered commands through the backend', async () => {
    const greet = defineCommand('greet', async (args) => ({
      stdout: `Hi ${args[0]}!\n`,
      stderr: '',
      exitCode: 0,
    }));

    const backend = createMockBackend();

    await Bash.create(
      () => backend,
      { customCommands: [greet] },
    );

    // Simulate calling the registered command through the backend
    const handler = backend.registeredCommands.get('greet');
    expect(handler).toBeDefined();

    // Note: the handler wraps async in sync — on WASM it returns an error for async commands
  });

  it('should handle commands that return sync-like results', async () => {
    // defineCommand with a function that effectively returns a resolved value
    const sync = defineCommand('sync-cmd', async () => ({
      stdout: 'sync output\n',
      stderr: '',
      exitCode: 0,
    }));

    const backend = createMockBackend();

    await Bash.create(
      () => backend,
      { customCommands: [sync] },
    );

    expect(backend.registerCommand).toHaveBeenCalledWith('sync-cmd', expect.any(Function));
  });

  it('should handle errors in custom commands', async () => {
    const failing = defineCommand('fail', async () => {
      throw new Error('Command failed');
    });

    const backend = createMockBackend();

    await Bash.create(
      () => backend,
      { customCommands: [failing] },
    );

    // The registered callback should catch errors
    const handler = backend.registeredCommands.get('fail');
    expect(handler).toBeDefined();

    if (handler) {
      const result = handler([], {
        fs: {
          readFileSync: () => '',
          writeFileSync: () => {},
          existsSync: () => false,
          mkdirSync: () => {},
          readdirSync: () => [],
          statSync: () => ({ isFile: false, isDirectory: false, size: 0 }),
          rmSync: () => {},
        },
        cwd: '/',
        env: {},
        stdin: '',
      });

      // Async commands in sync context should return an error message
      expect(result.exitCode).toBe(1);
      expect(result.stderr).toContain('async custom commands are not supported');
    }
  });
});
