/**
 * Tests for the Bash class.
 *
 * These tests validate the TypeScript API layer using a mock backend.
 * Integration tests with real WASM/native backends are separate.
 */

import { describe, it, expect, vi } from 'vitest';
import { Bash } from '../src/bash.js';
import type {
  BashBackend,
  ExecResult,
  FileStat,
  BackendExecOptions,
  BackendCommandContext,
} from '../src/types.js';

/** Create a mock backend for unit testing. */
function createMockBackend(overrides?: Partial<BashBackend>): BashBackend {
  const files = new Map<string, string>();
  const commands = new Set<string>(['echo', 'cat', 'grep', 'ls']);

  return {
    exec: vi.fn((command: string): ExecResult => {
      // Simple mock: echo simulation
      if (command.startsWith('echo ')) {
        const msg = command.slice(5);
        return { stdout: msg + '\n', stderr: '', exitCode: 0 };
      }
      if (command.startsWith('cat ')) {
        const path = command.slice(4).trim();
        const content = files.get(path);
        if (content !== undefined) {
          return { stdout: content, stderr: '', exitCode: 0 };
        }
        return { stdout: '', stderr: `cat: ${path}: No such file or directory\n`, exitCode: 1 };
      }
      return { stdout: '', stderr: '', exitCode: 0 };
    }),
    execWithOptions: vi.fn(
      (command: string, _options: BackendExecOptions): ExecResult => {
        if (command.startsWith('echo ')) {
          const msg = command.slice(5);
          return { stdout: msg + '\n', stderr: '', exitCode: 0 };
        }
        return { stdout: '', stderr: '', exitCode: 0 };
      },
    ),
    writeFile: vi.fn((path: string, content: string): void => {
      files.set(path, content);
    }),
    readFile: vi.fn((path: string): string => {
      const content = files.get(path);
      if (content === undefined) {
        throw new Error(`No such file: ${path}`);
      }
      return content;
    }),
    mkdir: vi.fn(),
    exists: vi.fn((path: string): boolean => files.has(path)),
    readdir: vi.fn((): string[] => []),
    stat: vi.fn(
      (path: string): FileStat => ({
        isFile: files.has(path),
        isDirectory: !files.has(path),
        size: files.get(path)?.length ?? 0,
      }),
    ),
    rm: vi.fn(),
    getCwd: vi.fn((): string => '/'),
    getLastExitCode: vi.fn((): number => 0),
    getCommandNames: vi.fn((): string[] => [...commands]),
    registerCommand: vi.fn(),
    ...overrides,
  };
}

describe('Bash', () => {
  it('should execute a simple command', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    const result = await bash.exec('echo hello');
    expect(result.stdout).toBe('hello\n');
    expect(result.exitCode).toBe(0);
  });

  it('should return exit code for failed commands', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    const result = await bash.exec('cat /nonexistent');
    expect(result.exitCode).toBe(1);
    expect(result.stderr).toContain('No such file');
  });

  it('should pass per-exec options', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    await bash.exec('echo hello', { cwd: '/tmp', env: { FOO: 'bar' } });
    expect(backend.execWithOptions).toHaveBeenCalled();
  });

  it('should pass stdin via options', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    await bash.exec('cat', { stdin: 'hello' });
    expect(backend.execWithOptions).toHaveBeenCalled();
  });

  it('should normalize scripts by stripping leading whitespace', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    await bash.exec(`
      echo hello
      echo world
    `);

    // The normalized script should have leading whitespace stripped
    expect(backend.exec).toHaveBeenCalledWith('echo hello\necho world');
  });

  it('should skip normalization with rawScript option', async () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    await bash.exec('  echo hello', { rawScript: true });
    expect(backend.exec).toHaveBeenCalledWith('  echo hello');
  });

  it('should expose fs proxy', () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    bash.fs.writeFileSync('/test.txt', 'content');
    expect(backend.writeFile).toHaveBeenCalledWith('/test.txt', 'content');
  });

  it('should expose fs.readFileSync', () => {
    const backend = createMockBackend();
    (backend.readFile as ReturnType<typeof vi.fn>).mockReturnValue('hello');
    const bash = new Bash(backend);

    const content = bash.fs.readFileSync('/test.txt');
    expect(content).toBe('hello');
  });

  it('should expose fs.existsSync', () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    bash.fs.existsSync('/test.txt');
    expect(backend.exists).toHaveBeenCalledWith('/test.txt');
  });

  it('should write and read files via convenience methods', () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    bash.writeFile('/test.txt', 'hello');
    expect(backend.writeFile).toHaveBeenCalledWith('/test.txt', 'hello');

    (backend.readFile as ReturnType<typeof vi.fn>).mockReturnValue('hello');
    const content = bash.readFile('/test.txt');
    expect(content).toBe('hello');
  });

  it('should return cwd', () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    expect(bash.getCwd()).toBe('/');
  });

  it('should return command names', () => {
    const backend = createMockBackend();
    const bash = new Bash(backend);

    const names = bash.getCommandNames();
    expect(names).toContain('echo');
    expect(names).toContain('cat');
  });
});

describe('Bash.create', () => {
  it('should resolve eager files', async () => {
    const createdFiles: Record<string, string>[] = [];

    const createBackend = (files: Record<string, string>) => {
      createdFiles.push(files);
      return createMockBackend();
    };

    await Bash.create(createBackend, {
      files: { '/data.txt': 'hello' },
    });

    expect(createdFiles[0]).toEqual({ '/data.txt': 'hello' });
  });

  it('should resolve lazy sync files', async () => {
    const createdFiles: Record<string, string>[] = [];

    const createBackend = (files: Record<string, string>) => {
      createdFiles.push(files);
      return createMockBackend();
    };

    await Bash.create(createBackend, {
      files: { '/data.txt': () => 'lazy content' },
    });

    expect(createdFiles[0]).toEqual({ '/data.txt': 'lazy content' });
  });

  it('should resolve lazy async files', async () => {
    const createdFiles: Record<string, string>[] = [];

    const createBackend = (files: Record<string, string>) => {
      createdFiles.push(files);
      return createMockBackend();
    };

    await Bash.create(createBackend, {
      files: { '/data.txt': async () => 'async content' },
    });

    expect(createdFiles[0]).toEqual({ '/data.txt': 'async content' });
  });

  it('should handle mixed file types', async () => {
    const createdFiles: Record<string, string>[] = [];

    const createBackend = (files: Record<string, string>) => {
      createdFiles.push(files);
      return createMockBackend();
    };

    await Bash.create(createBackend, {
      files: {
        '/eager.txt': 'eager content',
        '/lazy.txt': () => 'lazy content',
        '/async.txt': async () => 'async content',
      },
    });

    expect(createdFiles[0]).toEqual({
      '/eager.txt': 'eager content',
      '/lazy.txt': 'lazy content',
      '/async.txt': 'async content',
    });
  });
});
