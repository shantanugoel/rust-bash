import assert from 'node:assert/strict';
import test from 'node:test';
import { runAgentLoop } from './agent.ts';

function makeStream(chunks) {
  return (async function* () {
    for (const chunk of chunks) {
      yield chunk;
    }
  })();
}

test('falls back to non-streaming text when a streamed turn is empty', async () => {
  const bash = {
    exec: async () => ({ stdout: '', stderr: '', exitCode: 0 }),
  };

  let fallbackCalls = 0;
  const client = {
    async createStreamingCompletion() {
      return makeStream([{ choices: [{ delta: {} }] }]);
    },
    async createCompletion() {
      fallbackCalls += 1;
      return {
        choices: [
          {
            message: {
              content: 'Recovered final response',
            },
          },
        ],
      };
    },
  };

  const events = [];
  for await (const event of runAgentLoop('hello', bash, client)) {
    events.push(event);
  }

  assert.equal(fallbackCalls, 1);
  assert.deepEqual(events, [
    { type: 'text', content: 'Recovered final response' },
  ]);
});

test('falls back to non-streaming tool calls when a streamed turn is empty', async () => {
  const executedCommands = [];
  const bash = {
    async exec(command) {
      executedCommands.push(command);
      return { stdout: 'README.md\n', stderr: '', exitCode: 0 };
    },
  };

  let streamingCalls = 0;
  const client = {
    async createStreamingCompletion() {
      streamingCalls += 1;
      if (streamingCalls === 1) {
        return makeStream([{ choices: [{ delta: {} }] }]);
      }

      return makeStream([
        {
          choices: [
            {
              delta: {
                content: 'Listed the files for you.',
              },
            },
          ],
        },
      ]);
    },
    async createCompletion() {
      return {
        choices: [
          {
            message: {
              content: null,
              tool_calls: [
                {
                  id: 'call_1',
                  type: 'function',
                  function: {
                    name: 'bash',
                    arguments: '{"command":"ls"}',
                  },
                },
              ],
            },
          },
        ],
      };
    },
  };

  const events = [];
  for await (const event of runAgentLoop('can you list files', bash, client)) {
    events.push(event);
  }

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
