/**
 * Hand-crafted cached response for the initial "is this the matrix?" query.
 *
 * Same AgentEvent[] type as live agent events — zero branching in rendering.
 * Tool calls are executed for real against the bash instance during replay
 * so VFS state stays consistent with what the agent "said".
 */

import type { AgentEvent } from './agent.js';
import type { BashInstance } from './bash-loader.js';

export const CACHED_INITIAL_RESPONSE: AgentEvent[] = [
  { type: 'text', content: 'Close, but no. You\'re inside ' },
  { type: 'text', content: 'rust-bash — and it\'s all around you.\n\n' },
  { type: 'text', content: 'Let me show you...\n\n' },
  {
    type: 'tool_call',
    command: 'ls',
    result: {
      stdout: 'README.md  Cargo.toml  src  docs  examples\n',
      stderr: '',
      exitCode: 0,
    },
  },
  {
    type: 'tool_call',
    command: 'echo "hello from WASM" | sed s/WASM/the\\ matrix/',
    result: {
      stdout: 'hello from the matrix\n',
      stderr: '',
      exitCode: 0,
    },
  },
  {
    type: 'text',
    content:
      'This is a full bash interpreter built in Rust, compiled to WASM, ',
  },
  {
    type: 'text',
    content: 'running entirely in your browser. 80+ commands — ',
  },
  {
    type: 'text',
    content:
      'pipes, redirects, awk, sed, jq, grep, find — all real, all local.\n\n',
  },
  {
    type: 'text',
    content:
      'Try `cat README.md` to explore, or ask me to write a script!',
  },
];

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export type ReplayCacheOptions = {
  animate?: boolean;
};

/**
 * Replay a cached AgentEvent[] with typewriter animation.
 *
 * @param events - The cached events to replay
 * @param bash - Optional bash instance to execute tool calls for real
 * @returns An async generator yielding events + an interrupt callback
 */
export async function* replayCache(
  events: AgentEvent[],
  bash?: BashInstance,
  options: ReplayCacheOptions = {},
): AsyncGenerator<AgentEvent & { interrupt?: () => void }> {
  const animate = options.animate ?? true;
  let interrupted = false;
  const onInterrupt = () => {
    interrupted = true;
  };

  // Yield the interrupt handler as metadata on the first event
  let first = true;

  for (const event of events) {
    if (!animate) {
      if (event.type === 'tool_call' && bash) {
        const realResult = await bash.exec(event.command);
        const realEvent: AgentEvent & { interrupt?: () => void } = {
          type: 'tool_call',
          command: event.command,
          result: realResult,
        };
        if (first) {
          realEvent.interrupt = onInterrupt;
          first = false;
        }
        yield realEvent;
      } else if (first) {
        yield { ...event, interrupt: onInterrupt };
        first = false;
      } else {
        yield event;
      }
      continue;
    }

    if (event.type === 'text') {
      if (interrupted) {
        // Skip animation, dump rest instantly
        if (first) {
          yield { ...event, interrupt: onInterrupt };
          first = false;
        } else {
          yield event;
        }
        continue;
      }

      // Stream character by character with typewriter effect
      for (let i = 0; i < event.content.length; i++) {
        if (interrupted) {
          // Dump remaining text instantly
          const remaining = event.content.slice(i);
          yield { type: 'text', content: remaining };
          break;
        }
        const charEvent: AgentEvent & { interrupt?: () => void } = {
          type: 'text',
          content: event.content[i]!,
        };
        if (first) {
          charEvent.interrupt = onInterrupt;
          first = false;
        }
        yield charEvent;
        await sleep(12 + Math.random() * 8);
      }
    } else if (event.type === 'tool_call') {
      if (!interrupted) await sleep(100);

      // Execute the command for real so VFS state is consistent
      if (bash) {
        const realResult = await bash.exec(event.command);
        const realEvent: AgentEvent & { interrupt?: () => void } = {
          type: 'tool_call',
          command: event.command,
          result: realResult,
        };
        if (first) {
          realEvent.interrupt = onInterrupt;
          first = false;
        }
        yield realEvent;
      } else if (first) {
        yield { ...event, interrupt: onInterrupt };
        first = false;
      } else {
        yield event;
      }

      if (!interrupted) await sleep(300);
    }
  }
}
