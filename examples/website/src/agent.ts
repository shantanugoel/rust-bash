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

const tools: OpenAI.ChatCompletionTool[] = [
  {
    type: 'function',
    function: {
      name: 'bash',
      description:
        'Execute bash commands in the sandboxed rust-bash environment. ' +
        'Supports pipes, redirects, and 80+ commands.',
      parameters: {
        type: 'object' as const,
        properties: {
          command: {
            type: 'string',
            description: 'The bash command to execute',
          },
        },
        required: ['command'],
      },
    },
  },
];

const MAX_TURNS = 8;
const MAX_STDOUT = 5000;
const MAX_STDERR = 2000;
const MODEL = 'gemini-2.5-flash';

type PendingToolCall = {
  id: string;
  function: { name: string; arguments: string };
};

type TurnData = {
  content: string;
  textChunks: string[];
  toolCalls: PendingToolCall[];
};

type AgentClient = {
  createStreamingCompletion(
    params: Omit<OpenAI.ChatCompletionCreateParamsStreaming, 'stream'>,
  ): Promise<AsyncIterable<OpenAI.ChatCompletionChunk>>;
  createCompletion(
    params: OpenAI.ChatCompletionCreateParamsNonStreaming,
  ): Promise<OpenAI.ChatCompletion>;
};

function createAgentApi(client: OpenAI): AgentClient {
  return {
    createStreamingCompletion(params) {
      return client.chat.completions.create({
        ...params,
        stream: true,
      });
    },
    createCompletion(params) {
      return client.chat.completions.create({
        ...params,
        stream: false,
      });
    },
  };
}

function appendToolCallDelta(
  toolCalls: PendingToolCall[],
  deltaToolCalls: OpenAI.ChatCompletionChunk.Choice.Delta.ToolCall[],
): void {
  for (const tc of deltaToolCalls) {
    toolCalls[tc.index] = toolCalls[tc.index] ?? {
      id: '',
      function: { name: '', arguments: '' },
    };
    if (tc.id) toolCalls[tc.index]!.id = tc.id;
    if (tc.function?.name) {
      toolCalls[tc.index]!.function.name = tc.function.name;
    }
    if (tc.function?.arguments) {
      toolCalls[tc.index]!.function.arguments += tc.function.arguments;
    }
  }
}

function collectMessageContent(
  content: unknown,
): string {
  if (typeof content === 'string') {
    return content;
  }

  if (!Array.isArray(content)) {
    return '';
  }

  return content
    .map((part) => {
      if (
        part &&
        typeof part === 'object' &&
        'type' in part &&
        part.type === 'text' &&
        'text' in part &&
        typeof part.text === 'string'
      ) {
        return part.text;
      }

      return '';
    })
    .join('');
}

function collectMessageToolCalls(
  toolCalls: OpenAI.ChatCompletionMessage['tool_calls'],
): PendingToolCall[] {
  if (!toolCalls) {
    return [];
  }

  return toolCalls
    .filter((toolCall) => toolCall.type === 'function')
    .map((toolCall) => ({
      id: toolCall.id,
      function: {
        name: toolCall.function.name,
        arguments: toolCall.function.arguments,
      },
    }));
}

async function collectStreamedTurn(
  stream: AsyncIterable<OpenAI.ChatCompletionChunk>,
): Promise<TurnData> {
  let content = '';
  const textChunks: string[] = [];
  const toolCalls: PendingToolCall[] = [];

  for await (const chunk of stream) {
    const delta = chunk.choices[0]?.delta;
    if (delta?.content) {
      content += delta.content;
      textChunks.push(delta.content);
    }
    if (delta?.tool_calls) {
      appendToolCallDelta(toolCalls, delta.tool_calls);
    }
  }

  return { content, textChunks, toolCalls };
}

function collectCompletionTurn(
  completion: OpenAI.ChatCompletion,
): TurnData {
  const message = completion.choices[0]?.message;

  return {
    content: collectMessageContent(message?.content ?? null),
    textChunks: [],
    toolCalls: collectMessageToolCalls(message?.tool_calls),
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

  for (let turn = 0; turn < MAX_TURNS; turn++) {
    const request = {
      model: MODEL,
      messages,
      tools,
    } satisfies Omit<OpenAI.ChatCompletionCreateParamsStreaming, 'stream'>;

    let turnData: TurnData;
    try {
      const stream = await agentClient.createStreamingCompletion(request);
      turnData = await collectStreamedTurn(stream);
    } catch (err: unknown) {
      const message =
        err instanceof Error ? err.message : 'Unknown error';
      if (message.includes('429') || message.includes('rate')) {
        yield {
          type: 'text',
          content:
            '\n⚠️ The demo is currently busy. Try again in a moment, or explore the shell directly — all commands work without the AI agent.\n',
        };
      } else {
        yield {
          type: 'text',
          content: `\n⚠️ Agent error: ${message}\n`,
        };
      }
      return;
    }

    for (const chunk of turnData.textChunks) {
      yield { type: 'text', content: chunk };
    }

    if (!turnData.content && turnData.toolCalls.length === 0) {
      try {
        const completion = await agentClient.createCompletion(request);
        turnData = collectCompletionTurn(completion);
      } catch (err: unknown) {
        const message =
          err instanceof Error ? err.message : 'Unknown error';
        yield {
          type: 'text',
          content: `\n⚠️ Agent error: ${message}\n`,
        };
        return;
      }

      if (turnData.content) {
        yield { type: 'text', content: turnData.content };
      }
    }

    if (turnData.toolCalls.length > 0) {
      messages.push({
        role: 'assistant',
        content: turnData.content || null,
        tool_calls: turnData.toolCalls.map((tc) => ({
          id: tc.id,
          type: 'function' as const,
          function: tc.function,
        })),
      });

      for (const tc of turnData.toolCalls) {
        let args: { command: string };
        try {
          args = JSON.parse(tc.function.arguments) as {
            command: string;
          };
        } catch {
          args = { command: tc.function.arguments };
        }

        const result = await bash.exec(args.command);
        yield { type: 'tool_call', command: args.command, result };

        messages.push({
          role: 'tool',
          tool_call_id: tc.id,
          content: JSON.stringify({
            stdout: result.stdout.slice(0, MAX_STDOUT),
            stderr: result.stderr.slice(0, MAX_STDERR),
            exitCode: result.exitCode,
          }),
        });
      }
    } else {
      break;
    }
  }
}
