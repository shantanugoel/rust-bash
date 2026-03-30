/**
 * AI tool definitions for @shantanugoel/rust-bash.
 *
 * Exports framework-agnostic JSON Schema tool definitions and a handler
 * factory. Works with OpenAI, Anthropic, Vercel AI SDK, LangChain, or
 * any framework that accepts JSON Schema tool definitions.
 */

import { Bash } from './bash.js';
import type {
  BashToolOptions,
  BashToolHandler,
  ToolDefinition,
  ToolProvider,
  ExecResult,
} from './types.js';

/** JSON Schema tool definition for the bash tool. */
export const bashToolDefinition: ToolDefinition = {
  name: 'bash',
  description:
    'Execute bash commands in a sandboxed environment with an in-memory virtual filesystem. ' +
    'Supports standard Unix utilities including grep, sed, awk, jq, cat, echo, and more. ' +
    'All file operations are isolated within the sandbox.',
  inputSchema: {
    type: 'object',
    properties: {
      command: {
        type: 'string',
        description: 'The bash command to execute',
      },
    },
    required: ['command'],
  },
};

/**
 * Create a bash tool handler for use with any AI framework.
 *
 * @example
 * ```ts
 * const { handler, definition, bash } = createBashToolHandler({
 *   files: { '/data.txt': 'hello world' },
 * });
 *
 * // Use with OpenAI
 * const result = await handler({ command: 'grep hello /data.txt' });
 * ```
 */
export function createBashToolHandler(
  createBackend: (
    files: Record<string, string>,
    options?: BashToolOptions,
  ) => import('./types.js').BashBackend,
  options?: BashToolOptions,
): BashToolHandler {
  const maxOutputLength = options?.maxOutputLength ?? 100_000;

  // Create backend synchronously with pre-resolved files.
  // For the tool handler, files are expected to be eager strings.
  const files: Record<string, string> = {};
  if (options?.files) {
    for (const [path, entry] of Object.entries(options.files)) {
      if (typeof entry === 'string') {
        files[path] = entry;
      } else {
        throw new Error(
          `createBashToolHandler: lazy file at "${path}" is not supported. ` +
            'Use eager string values for tool handler files.',
        );
      }
    }
  }

  const backend = createBackend(files, options);
  const bash = new Bash(backend, options);

  const handler = async (args: { command: string }): Promise<ExecResult> => {
    const result = await bash.exec(args.command);

    // Truncate output if needed
    if (maxOutputLength > 0) {
      if (result.stdout.length > maxOutputLength) {
        result.stdout =
          result.stdout.slice(0, maxOutputLength) + `\n... (truncated, ${result.stdout.length} total chars)`;
      }
      if (result.stderr.length > maxOutputLength) {
        result.stderr =
          result.stderr.slice(0, maxOutputLength) + `\n... (truncated, ${result.stderr.length} total chars)`;
      }
    }

    return result;
  };

  return { handler, definition: bashToolDefinition, bash };
}

/**
 * Format a tool definition for a specific AI provider's wire format.
 *
 * These are thin wrappers — no external dependencies required.
 *
 * @example
 * ```ts
 * const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
 * // { type: "function", function: { name, description, parameters } }
 *
 * const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');
 * // { name, description, input_schema }
 * ```
 */
export function formatToolForProvider(
  definition: ToolDefinition,
  provider: ToolProvider,
): Record<string, unknown> {
  switch (provider) {
    case 'openai':
      return {
        type: 'function',
        function: {
          name: definition.name,
          description: definition.description,
          parameters: definition.inputSchema,
        },
      };
    case 'anthropic':
      return {
        name: definition.name,
        description: definition.description,
        input_schema: definition.inputSchema,
      };
    case 'mcp':
      return {
        name: definition.name,
        description: definition.description,
        inputSchema: definition.inputSchema,
      };
    default:
      throw new Error(`Unknown provider: ${provider as string}`);
  }
}

/** Tool definitions for auxiliary file operations. */
export const writeFileToolDefinition: ToolDefinition = {
  name: 'writeFile',
  description: 'Write content to a file in the sandboxed virtual filesystem.',
  inputSchema: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'The absolute path to write to',
      },
      content: {
        type: 'string',
        description: 'The content to write',
      },
    },
    required: ['path', 'content'],
  },
};

export const readFileToolDefinition: ToolDefinition = {
  name: 'readFile',
  description: 'Read the contents of a file from the sandboxed virtual filesystem.',
  inputSchema: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'The absolute path to read',
      },
    },
    required: ['path'],
  },
};

export const listDirectoryToolDefinition: ToolDefinition = {
  name: 'listDirectory',
  description: 'List the contents of a directory in the sandboxed virtual filesystem.',
  inputSchema: {
    type: 'object',
    properties: {
      path: {
        type: 'string',
        description: 'The absolute path of the directory to list',
      },
    },
    required: ['path'],
  },
};

/**
 * Dispatch a tool call to the appropriate handler on a Bash instance.
 *
 * Supports tool names in both camelCase and snake_case:
 * `bash`, `readFile`/`read_file`, `writeFile`/`write_file`, `listDirectory`/`list_directory`.
 *
 * **Return shapes by tool:**
 * - `bash` → `{ stdout: string, stderr: string, exitCode: number }`
 * - `readFile` / `read_file` → `{ content: string }`
 * - `writeFile` / `write_file` → `{ success: true }`
 * - `listDirectory` / `list_directory` → `{ entries: string[] }`
 * - On error (any tool) → `{ error: string }`
 *
 * @example
 * ```ts
 * const result = await handleToolCall(bash, 'bash', { command: 'echo hi' });
 * const file = await handleToolCall(bash, 'readFile', { path: '/data.txt' });
 * ```
 */
export async function handleToolCall(
  bash: Bash,
  toolName: string,
  args: Record<string, string>,
): Promise<Record<string, unknown>> {
  switch (toolName) {
    case 'bash': {
      if (!args.command) {
        return { error: "Missing required argument: 'command'" };
      }
      try {
        const result = await bash.exec(args.command);
        return {
          stdout: result.stdout,
          stderr: result.stderr,
          exitCode: result.exitCode,
        };
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : String(err);
        return { error: message };
      }
    }
    case 'readFile':
    case 'read_file': {
      if (!args.path) {
        return { error: "Missing required argument: 'path'" };
      }
      try {
        const content = bash.readFile(args.path);
        return { content };
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : String(err);
        return { error: message };
      }
    }
    case 'writeFile':
    case 'write_file': {
      if (!args.path) {
        return { error: "Missing required argument: 'path'" };
      }
      try {
        bash.writeFile(args.path, args.content ?? '');
        return { success: true };
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : String(err);
        return { error: message };
      }
    }
    case 'listDirectory':
    case 'list_directory': {
      if (!args.path) {
        return { error: "Missing required argument: 'path'" };
      }
      try {
        const entries = bash.fs.readdirSync(args.path);
        return { entries };
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : String(err);
        return { error: message };
      }
    }
    default:
      throw new Error(`Unknown tool: ${toolName}`);
  }
}
