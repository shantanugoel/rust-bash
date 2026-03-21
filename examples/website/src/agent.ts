/**
 * Client-side agent loop.
 *
 * Uses the OpenAI npm package to talk to the Cloudflare Worker proxy
 * at `/api`, which forwards to Google Gemini 2.5 Flash.
 *
 * The agent loop is a pure async generator that yields AgentEvents.
 * Terminal rendering is handled by the caller — this module has no
 * UI dependencies.
 *
 * Tool calls are executed locally via the mock bash (or WASM when available).
 */

import OpenAI from 'openai';
import type { BashInstance, ExecResult } from './bash-loader.js';

export type AgentEvent =
  | { type: 'text'; content: string }
  | { type: 'tool_call'; command: string; result: ExecResult };

const SYSTEM_INSTRUCTIONS = `You are an AI assistant embedded in rust-bash, a sandboxed bash interpreter built in Rust and compiled to WASM, running in the user's browser.

You have access to a "bash" tool that executes commands in the sandboxed environment. The virtual filesystem contains project files like README.md, Cargo.toml, source code, and example scripts.

Available commands include: echo, cat, grep, sed, awk, jq, find, sort, uniq, wc, head, tail, tr, seq, ls, pwd, cd, and 60+ more.

Guidelines:
- Use the bash tool to demonstrate commands and explore the filesystem
- Keep responses concise and engaging — this is a demo
- Show off interesting command combinations (pipes, redirects)
- When asked about rust-bash, explore the actual files in the VFS
- Be creative with demonstrations`;

export function getApiBaseUrl(origin = globalThis.location?.origin): string {
  if (!origin) {
    throw new Error('Browser location is unavailable');
  }

  return new URL('/api/', origin).toString();
}

function createClient(): OpenAI {
  return new OpenAI({
    baseURL: getApiBaseUrl(),
    apiKey: 'unused', // Worker handles auth
    dangerouslyAllowBrowser: true,
  });
}

const MAX_TURNS = 8;
const MAX_STDOUT = 5000;
const MAX_STDERR = 2000;
const MODEL = 'gemini-2.5-flash';

type BashToolArgs = {
  command: string;
};

type BashTool = {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: {
      type: 'object';
      properties: {
        command: {
          type: 'string';
          description: string;
        };
      };
      required: ['command'];
    };
    parse: (input: string) => BashToolArgs;
    function: (args: BashToolArgs) => Promise<string>;
  };
};

type AgentClient = {
  runTools(
    params: {
      model: string;
      messages: OpenAI.ChatCompletionMessageParam[];
      tools: [BashTool];
    },
    options: { maxChatCompletions: number },
  ): {
    on(event: 'content', listener: (content: string) => void): unknown;
    done(): Promise<void>;
  };
};

function createAgentApi(client: OpenAI): AgentClient {
  return {
    runTools(params, options) {
      return client.chat.completions.runTools(params, options);
    },
  };
}

function hasVisibleContent(content: string): boolean {
  return content.trim().length > 0;
}

class AsyncEventQueue<T> implements AsyncIterable<T>, AsyncIterator<T> {
  private items: T[] = [];
  private waiters: Array<(value: IteratorResult<T>) => void> = [];
  private closed = false;

  push(item: T): void {
    if (this.closed) {
      return;
    }

    const waiter = this.waiters.shift();
    if (waiter) {
      waiter({ value: item, done: false });
      return;
    }

    this.items.push(item);
  }

  close(): void {
    if (this.closed) {
      return;
    }

    this.closed = true;
    while (this.waiters.length > 0) {
      const waiter = this.waiters.shift();
      waiter?.({ value: undefined as T, done: true });
    }
  }

  async next(): Promise<IteratorResult<T>> {
    const item = this.items.shift();
    if (item !== undefined) {
      return { value: item, done: false };
    }

    if (this.closed) {
      return { value: undefined as T, done: true };
    }

    return await new Promise<IteratorResult<T>>((resolve) => {
      this.waiters.push(resolve);
    });
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return this;
  }
}

function parseBashToolArgs(input: string): BashToolArgs {
  try {
    const parsed = JSON.parse(input) as unknown;
    if (typeof parsed === 'string') {
      return { command: parsed };
    }

    if (
      parsed &&
      typeof parsed === 'object' &&
      'command' in parsed &&
      typeof parsed.command === 'string'
    ) {
      return { command: parsed.command };
    }
  } catch {
    // Fall back to the raw input for partially compatible providers.
  }

  return { command: input };
}

function formatToolResult(result: ExecResult): string {
  return JSON.stringify({
    stdout: result.stdout.slice(0, MAX_STDOUT),
    stderr: result.stderr.slice(0, MAX_STDERR),
    exitCode: result.exitCode,
  });
}

function formatAgentError(err: unknown): AgentEvent {
  const message = err instanceof Error ? err.message : 'Unknown error';

  if (message.includes('429') || message.includes('rate')) {
    return {
      type: 'text',
      content:
        '\n⚠️ The demo is currently busy. Try again in a moment, or explore the shell directly — all commands work without the AI agent.\n',
    };
  }

  return {
    type: 'text',
    content: `\n⚠️ Agent error: ${message}\n`,
  };
}

function createBashTool(
  bash: BashInstance,
  pushEvent: (event: AgentEvent) => void,
): BashTool {
  return {
    type: 'function',
    function: {
      name: 'bash',
      description:
        'Execute bash commands in the sandboxed rust-bash environment. ' +
        'Supports pipes, redirects, and 80+ commands.',
      parameters: {
        type: 'object',
        properties: {
          command: {
            type: 'string',
            description: 'The bash command to execute',
          },
        },
        required: ['command'],
      },
      parse: parseBashToolArgs,
      async function(args) {
        const result = await bash.exec(args.command);
        pushEvent({ type: 'tool_call', command: args.command, result });
        return formatToolResult(result);
      },
    },
  };
}

export async function* runAgentLoop(
  userMessage: string,
  bash: BashInstance,
  agentClient = createAgentApi(createClient()),
): AsyncGenerator<AgentEvent> {
  const messages: OpenAI.ChatCompletionMessageParam[] = [
    { role: 'system', content: SYSTEM_INSTRUCTIONS },
    { role: 'user', content: userMessage },
  ];
  const events = new AsyncEventQueue<AgentEvent>();
  let emittedEvents = 0;
  const pushEvent = (event: AgentEvent): void => {
    emittedEvents += 1;
    events.push(event);
  };

  let runner: ReturnType<AgentClient['runTools']>;
  try {
    runner = agentClient.runTools(
      {
        model: MODEL,
        messages,
        tools: [createBashTool(bash, pushEvent)],
      },
      { maxChatCompletions: MAX_TURNS },
    );
  } catch (err: unknown) {
    yield formatAgentError(err);
    return;
  }

  runner.on('content', (content) => {
    if (hasVisibleContent(content)) {
      pushEvent({ type: 'text', content });
    }
  });

  void (async () => {
    try {
      await runner.done();
      if (emittedEvents === 0) {
        pushEvent({
          type: 'text',
          content:
            '\n⚠️ Agent returned an empty response. Try again, or use the shell directly.\n',
        });
      }
    } catch (err: unknown) {
      pushEvent(formatAgentError(err));
    } finally {
      events.close();
    }
  })();

  for await (const event of events) {
    yield event;
  }
}
