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

const client = new OpenAI({
  baseURL: '/api',
  apiKey: 'unused', // Worker handles auth
  dangerouslyAllowBrowser: true,
});

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

export async function* runAgentLoop(
  userMessage: string,
  bash: BashInstance,
): AsyncGenerator<AgentEvent> {
  const messages: OpenAI.ChatCompletionMessageParam[] = [
    { role: 'system', content: SYSTEM_INSTRUCTIONS },
    { role: 'user', content: userMessage },
  ];

  for (let turn = 0; turn < MAX_TURNS; turn++) {
    let stream: AsyncIterable<OpenAI.ChatCompletionChunk>;
    try {
      stream = await client.chat.completions.create({
        model: 'gemini-2.5-flash',
        messages,
        tools,
        stream: true,
      });
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

    let fullResponse = '';
    const toolCalls: {
      id: string;
      function: { name: string; arguments: string };
    }[] = [];

    for await (const chunk of stream) {
      const delta = chunk.choices[0]?.delta;
      if (delta?.content) {
        fullResponse += delta.content;
        yield { type: 'text', content: delta.content };
      }
      if (delta?.tool_calls) {
        for (const tc of delta.tool_calls) {
          toolCalls[tc.index] = toolCalls[tc.index] ?? {
            id: '',
            function: { name: '', arguments: '' },
          };
          if (tc.id) toolCalls[tc.index]!.id = tc.id;
          if (tc.function?.name)
            toolCalls[tc.index]!.function.name = tc.function.name;
          if (tc.function?.arguments)
            toolCalls[tc.index]!.function.arguments +=
              tc.function.arguments;
        }
      }
    }

    if (toolCalls.length > 0) {
      // Add assistant message with tool calls
      messages.push({
        role: 'assistant',
        content: fullResponse || null,
        tool_calls: toolCalls.map((tc) => ({
          id: tc.id,
          type: 'function' as const,
          function: tc.function,
        })),
      });

      // Execute each tool call locally
      for (const tc of toolCalls) {
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
      // Loop continues — send results back to LLM
    } else {
      // No tool calls — final text response
      break;
    }
  }
}
