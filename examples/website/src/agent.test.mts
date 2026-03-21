import assert from 'node:assert/strict';
import test from 'node:test';
import { runAgentLoop } from './agent.ts';

function createMockClient(run) {
  return {
    runTools(params) {
      const contentListeners = [];

      return {
        on(event, listener) {
          if (event === 'content') {
            contentListeners.push(listener);
          }
          return this;
        },
        async done() {
          await run({
            params,
            emitContent(content) {
              for (const listener of contentListeners) {
                listener(content);
              }
            },
            async invokeTool(rawArgs) {
              const tool = params.tools[0];
              const parsed = tool.function.parse(rawArgs);
              return await tool.function.function(parsed);
            },
          });
        },
      };
    },
  };
}

async function collectEvents(userMessage, bash, client) {
  const events = [];

  for await (const event of runAgentLoop(userMessage, bash, client)) {
    events.push(event);
  }

  return events;
}

test('uses the SDK tool runner to execute bash tool calls and emit assistant content', async () => {
  const executedCommands = [];
  const bash = {
    async exec(command) {
      executedCommands.push(command);
      return { stdout: 'README.md\n', stderr: '', exitCode: 0 };
    },
  };

  const client = createMockClient(async ({ emitContent, invokeTool }) => {
    await invokeTool('{"command":"ls"}');
    emitContent('Listed the files for you.');
  });

  const events = await collectEvents('can you list files', bash, client);

  assert.deepEqual(executedCommands, ['ls']);
  assert.deepEqual(events, [
    {
      type: 'tool_call',
      command: 'ls',
      result: { stdout: 'README.md\n', stderr: '', exitCode: 0 },
    },
    {
      type: 'text',
      content: 'Listed the files for you.',
    },
  ]);
});

test('accepts raw string tool arguments from partially compatible providers', async () => {
  const executedCommands = [];
  const bash = {
    async exec(command) {
      executedCommands.push(command);
      return { stdout: 'ok\n', stderr: '', exitCode: 0 };
    },
  };

  const client = createMockClient(async ({ invokeTool }) => {
    await invokeTool('pwd');
  });

  const events = await collectEvents('where am i', bash, client);

  assert.deepEqual(executedCommands, ['pwd']);
  assert.deepEqual(events, [
    {
      type: 'tool_call',
      command: 'pwd',
      result: { stdout: 'ok\n', stderr: '', exitCode: 0 },
    },
  ]);
});

test('warns when the runner produces no visible content or tool calls', async () => {
  const bash = {
    exec: async () => ({ stdout: '', stderr: '', exitCode: 0 }),
  };

  const client = createMockClient(async ({ emitContent }) => {
    emitContent('\n\n');
  });

  const events = await collectEvents('list files', bash, client);

  assert.deepEqual(events, [
    {
      type: 'text',
      content:
        '\n⚠️ Agent returned an empty response. Try again, or use the shell directly.\n',
    },
  ]);
});

test('formats rate limit errors as a busy message', async () => {
  const bash = {
    exec: async () => ({ stdout: '', stderr: '', exitCode: 0 }),
  };

  const client = createMockClient(async () => {
    throw new Error('429 rate limit exceeded');
  });

  const events = await collectEvents('hello', bash, client);

  assert.deepEqual(events, [
    {
      type: 'text',
      content:
        '\n⚠️ The demo is currently busy. Try again in a moment, or explore the shell directly — all commands work without the AI agent.\n',
    },
  ]);
});
