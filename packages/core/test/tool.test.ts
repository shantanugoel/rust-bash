/**
 * Tests for the tool definitions API.
 */

import { describe, it, expect, vi } from 'vitest';
import {
  bashToolDefinition,
  createBashToolHandler,
  formatToolForProvider,
  handleToolCall,
  writeFileToolDefinition,
  readFileToolDefinition,
  listDirectoryToolDefinition,
} from '../src/tool.js';
import { Bash } from '../src/bash.js';
import type {
  BashBackend,
  ExecResult,
  FileStat,
  BackendExecOptions,
  BackendCommandContext,
} from '../src/types.js';

function createMockBackend(): BashBackend {
  const files: Record<string, string> = {
    '/data.txt': 'hello world\n',
  };
  const dirs: Record<string, string[]> = {
    '/': ['data.txt', 'docs'],
  };

  return {
    exec: vi.fn((command: string): ExecResult => {
      if (command.startsWith('echo ')) {
        return { stdout: command.slice(5) + '\n', stderr: '', exitCode: 0 };
      }
      if (command.startsWith('cat ')) {
        const path = command.slice(4).trim();
        if (path === '/data.txt') {
          return { stdout: 'hello world\n', stderr: '', exitCode: 0 };
        }
        return { stdout: '', stderr: `cat: ${path}: No such file\n`, exitCode: 1 };
      }
      return { stdout: '', stderr: '', exitCode: 0 };
    }),
    execWithOptions: vi.fn(
      (_cmd: string, _opts: BackendExecOptions): ExecResult => ({
        stdout: '',
        stderr: '',
        exitCode: 0,
      }),
    ),
    writeFile: vi.fn((path: string, content: string) => {
      files[path] = content;
    }),
    readFile: vi.fn((path: string): string => {
      if (path in files) return files[path]!;
      throw new Error(`No such file: ${path}`);
    }),
    mkdir: vi.fn(),
    exists: vi.fn((path: string): boolean => path in files || path in dirs),
    readdir: vi.fn((path: string): string[] => {
      if (path in dirs) return dirs[path]!;
      throw new Error(`No such directory: ${path}`);
    }),
    stat: vi.fn((): FileStat => ({ isFile: true, isDirectory: false, size: 12 })),
    rm: vi.fn(),
    getCwd: vi.fn((): string => '/'),
    getLastExitCode: vi.fn((): number => 0),
    getCommandNames: vi.fn((): string[] => ['echo', 'cat']),
    registerCommand: vi.fn(),
  };
}

describe('bashToolDefinition', () => {
  it('should have correct name', () => {
    expect(bashToolDefinition.name).toBe('bash');
  });

  it('should have a description', () => {
    expect(bashToolDefinition.description).toBeTruthy();
    expect(typeof bashToolDefinition.description).toBe('string');
  });

  it('should have valid JSON Schema input', () => {
    expect(bashToolDefinition.inputSchema.type).toBe('object');
    expect(bashToolDefinition.inputSchema.properties.command).toBeDefined();
    expect(bashToolDefinition.inputSchema.properties.command.type).toBe('string');
    expect(bashToolDefinition.inputSchema.required).toContain('command');
  });

  it('should be compatible with OpenAI tool format', () => {
    const { inputSchema } = bashToolDefinition;
    expect(inputSchema.type).toBe('object');
    expect(inputSchema.properties).toBeDefined();
    expect(inputSchema.required).toBeDefined();
    expect(Array.isArray(inputSchema.required)).toBe(true);
  });
});

describe('createBashToolHandler', () => {
  it('should create a handler and definition', () => {
    const { handler, definition, bash } = createBashToolHandler(
      () => createMockBackend(),
    );

    expect(typeof handler).toBe('function');
    expect(definition).toBe(bashToolDefinition);
    expect(bash).toBeDefined();
  });

  it('should execute commands through the handler', async () => {
    const { handler } = createBashToolHandler(
      () => createMockBackend(),
    );

    const result = await handler({ command: 'echo hello' });
    expect(result.stdout).toBe('hello\n');
    expect(result.exitCode).toBe(0);
  });

  it('should pass files to the backend', () => {
    const createdFiles: Record<string, string>[] = [];

    createBashToolHandler(
      (files) => {
        createdFiles.push(files);
        return createMockBackend();
      },
      { files: { '/data.txt': 'content' } },
    );

    expect(createdFiles[0]).toEqual({ '/data.txt': 'content' });
  });

  it('should throw on lazy file entries', () => {
    expect(() =>
      createBashToolHandler(
        () => createMockBackend(),
        { files: { '/data.txt': (() => 'lazy') as unknown as string } },
      ),
    ).toThrow('lazy file');
  });

  it('should truncate long output', async () => {
    const backend = createMockBackend();
    (backend.exec as ReturnType<typeof vi.fn>).mockReturnValue({
      stdout: 'x'.repeat(200),
      stderr: '',
      exitCode: 0,
    });

    const { handler } = createBashToolHandler(
      () => backend,
      { maxOutputLength: 100 },
    );

    const result = await handler({ command: 'generate-long-output' });
    expect(result.stdout.length).toBeLessThan(300);
    expect(result.stdout).toContain('truncated');
  });

  it('should not truncate short output', async () => {
    const { handler } = createBashToolHandler(
      () => createMockBackend(),
    );

    const result = await handler({ command: 'echo short' });
    expect(result.stdout).toBe('short\n');
    expect(result.stdout).not.toContain('truncated');
  });
});

describe('formatToolForProvider', () => {
  it('should format for OpenAI', () => {
    const formatted = formatToolForProvider(bashToolDefinition, 'openai');

    expect(formatted.type).toBe('function');
    const fn = formatted.function as Record<string, unknown>;
    expect(fn.name).toBe('bash');
    expect(fn.description).toBe(bashToolDefinition.description);
    expect(fn.parameters).toEqual(bashToolDefinition.inputSchema);
  });

  it('should format for Anthropic', () => {
    const formatted = formatToolForProvider(bashToolDefinition, 'anthropic');

    expect(formatted.name).toBe('bash');
    expect(formatted.description).toBe(bashToolDefinition.description);
    expect(formatted.input_schema).toEqual(bashToolDefinition.inputSchema);
  });

  it('should format for MCP', () => {
    const formatted = formatToolForProvider(bashToolDefinition, 'mcp');

    expect(formatted.name).toBe('bash');
    expect(formatted.description).toBe(bashToolDefinition.description);
    expect(formatted.inputSchema).toEqual(bashToolDefinition.inputSchema);
  });

  it('should throw for unknown provider', () => {
    expect(() => {
      formatToolForProvider(bashToolDefinition, 'unknown' as 'openai');
    }).toThrow('Unknown provider');
  });
});

describe('auxiliary tool definitions', () => {
  it('writeFileToolDefinition has correct schema', () => {
    expect(writeFileToolDefinition.name).toBe('writeFile');
    expect(writeFileToolDefinition.inputSchema.required).toContain('path');
    expect(writeFileToolDefinition.inputSchema.required).toContain('content');
  });

  it('readFileToolDefinition has correct schema', () => {
    expect(readFileToolDefinition.name).toBe('readFile');
    expect(readFileToolDefinition.inputSchema.required).toContain('path');
  });

  it('listDirectoryToolDefinition has correct schema', () => {
    expect(listDirectoryToolDefinition.name).toBe('listDirectory');
    expect(listDirectoryToolDefinition.inputSchema.required).toContain('path');
  });
});

describe('handleToolCall', () => {
  function createBashWithMock(): Bash {
    return new Bash(createMockBackend());
  }

  it('should handle bash tool calls', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'bash', { command: 'echo hello' });
    expect(result.stdout).toBe('hello\n');
    expect(result.exitCode).toBe(0);
  });

  it('should handle readFile tool calls', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'readFile', { path: '/data.txt' });
    expect(result.content).toBe('hello world\n');
  });

  it('should handle readFile errors gracefully', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'readFile', { path: '/nonexistent' });
    expect(result.error).toBeDefined();
  });

  it('should handle writeFile tool calls', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'writeFile', {
      path: '/new.txt',
      content: 'new content',
    });
    expect(result.success).toBe(true);
  });

  it('should handle listDirectory tool calls', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'listDirectory', { path: '/' });
    expect(Array.isArray(result.entries)).toBe(true);
    expect(result.entries).toContain('data.txt');
  });

  it('should handle listDirectory errors gracefully', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'listDirectory', { path: '/nonexistent' });
    expect(result.error).toBeDefined();
  });

  it('should throw for unknown tool names', async () => {
    const bash = createBashWithMock();
    await expect(
      handleToolCall(bash, 'unknown', {}),
    ).rejects.toThrow('Unknown tool');
  });

  it('should accept snake_case tool names', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'read_file', { path: '/data.txt' });
    expect(result.content).toBe('hello world\n');
  });

  it('should accept write_file snake_case', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'write_file', {
      path: '/new.txt',
      content: 'hello',
    });
    expect(result.success).toBe(true);
  });

  it('should accept list_directory snake_case', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'list_directory', { path: '/' });
    expect(Array.isArray(result.entries)).toBe(true);
  });

  it('should return error for missing command arg', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'bash', {});
    expect(result.error).toBeDefined();
  });

  it('should return error for missing path arg on readFile', async () => {
    const bash = createBashWithMock();
    const result = await handleToolCall(bash, 'readFile', {});
    expect(result.error).toBeDefined();
  });
});
