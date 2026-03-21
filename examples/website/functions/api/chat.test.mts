import assert from 'node:assert/strict';
import test from 'node:test';
import {
  buildUpstreamRequestBody,
  getChatCompletionsUrl,
  isAllowedOrigin,
  resolveUpstreamConfig,
} from './chat.ts';

test('allows the deployed Pages origin', () => {
  assert.equal(
    isAllowedOrigin(
      'https://9dc0e729.rustbashweb.pages.dev',
      'https://9dc0e729.rustbashweb.pages.dev/api/chat/completions',
    ),
    true,
  );
});

test('allows localhost dev requests when allowLocalhost is true', () => {
  assert.equal(
    isAllowedOrigin(
      'http://localhost:5173',
      'http://localhost:8788/api/chat/completions',
      true,
    ),
    true,
  );
});

test('rejects localhost requests when allowLocalhost is false (default)', () => {
  assert.equal(
    isAllowedOrigin(
      'http://localhost:5173',
      'https://rustbash.dev/api/chat/completions',
    ),
    false,
  );
});

test('rejects 127.0.0.1 requests when allowLocalhost is false', () => {
  assert.equal(
    isAllowedOrigin(
      'http://127.0.0.1:8080',
      'https://rustbash.dev/api/chat/completions',
    ),
    false,
  );
});

test('rejects cross-origin requests', () => {
  assert.equal(
    isAllowedOrigin(
      'https://evil.example',
      'https://9dc0e729.rustbashweb.pages.dev/api/chat/completions',
    ),
    false,
  );
});

test('rejects invalid origin headers', () => {
  assert.equal(
    isAllowedOrigin(
      'not a url',
      'https://9dc0e729.rustbashweb.pages.dev/api/chat/completions',
    ),
    false,
  );
});

test('builds the default Gemini chat completions URL', () => {
  assert.equal(
    getChatCompletionsUrl('https://generativelanguage.googleapis.com/v1beta/openai/'),
    'https://generativelanguage.googleapis.com/v1beta/openai/chat/completions',
  );
});

test('resolves default Gemini settings when only GEMINI_API_KEY is set', () => {
  assert.deepEqual(
    resolveUpstreamConfig({ GEMINI_API_KEY: 'gemini-secret' }),
    {
      apiKey: 'gemini-secret',
      chatCompletionsUrl:
        'https://generativelanguage.googleapis.com/v1beta/openai/chat/completions',
      model: 'gemini-2.5-flash',
    },
  );
});

test('prefers LLM_* overrides when provided', () => {
  assert.deepEqual(
    resolveUpstreamConfig({
      GEMINI_API_KEY: 'gemini-secret',
      LLM_API_KEY: 'provider-secret',
      LLM_BASE_URL: 'https://api.openai.example/v1',
      LLM_MODEL: 'provider-model',
    }),
    {
      apiKey: 'provider-secret',
      chatCompletionsUrl: 'https://api.openai.example/v1/chat/completions',
      model: 'provider-model',
    },
  );
});

test('throws when no API key is configured', () => {
  assert.throws(
    () => resolveUpstreamConfig({}),
    /LLM_API_KEY or GEMINI_API_KEY is not configured/,
  );
});

test('preserves non-streaming requests for SDK tool runners', () => {
  assert.deepEqual(
    buildUpstreamRequestBody(
      {
        messages: [{ role: 'user', content: 'hello' }],
        stream: false,
      },
      'gemini-2.5-flash',
    ),
    {
      model: 'gemini-2.5-flash',
      messages: [{ role: 'user', content: 'hello' }],
      tools: [
        {
          type: 'function',
          function: {
            name: 'bash',
            description: 'Execute bash commands in the sandboxed rust-bash environment.',
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
          },
        },
      ],
      stream: false,
    },
  );
});

test('allows explicitly streamed requests when requested', () => {
  assert.equal(
    buildUpstreamRequestBody(
      {
        messages: [],
        stream: true,
      },
      'gemini-2.5-flash',
    ).stream,
    true,
  );
});
