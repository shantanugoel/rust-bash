import assert from 'node:assert/strict';
import test from 'node:test';
import { replayCache } from './cached-initial-response.ts';

test('replayCache instant mode yields full events and executes tool calls', async () => {
  const executedCommands = [];
  const bash = {
    async exec(command) {
      executedCommands.push(command);
      return { stdout: 'README.md\n', stderr: '', exitCode: 0 };
    },
  };

  const events = [];
  for await (const event of replayCache(
    [
      { type: 'text', content: 'hello there' },
      {
        type: 'tool_call',
        command: 'ls',
        result: { stdout: '', stderr: '', exitCode: 0 },
      },
      { type: 'text', content: 'done' },
    ],
    bash,
    { animate: false },
  )) {
    events.push(event);
  }

  assert.deepEqual(executedCommands, ['ls']);
  assert.deepEqual(events, [
    { type: 'text', content: 'hello there', interrupt: events[0].interrupt },
    {
      type: 'tool_call',
      command: 'ls',
      result: { stdout: 'README.md\n', stderr: '', exitCode: 0 },
    },
    { type: 'text', content: 'done' },
  ]);
  assert.equal(typeof events[0].interrupt, 'function');
});
