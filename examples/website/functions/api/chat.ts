/**
 * Cloudflare Pages Function — LLM proxy.
 *
 * Proxies requests to a configurable OpenAI-compatible endpoint.
 * Defaults to Google Gemini 3.1 Flash Lite Preview. Rate limited to 10 req/min/IP.
 */

/// <reference types="@cloudflare/workers-types" />

interface Env {
  GEMINI_API_KEY?: string;
  LLM_API_KEY?: string;
  LLM_BASE_URL?: string;
  LLM_MODEL?: string;
  ALLOW_LOCALHOST?: string;
}

// Simple in-memory rate limiter (resets on Worker cold start)
const rateLimitMap = new Map<string, { count: number; resetAt: number }>();
const RATE_LIMIT = 10;
const RATE_WINDOW_MS = 60_000;
const LOCALHOST_HOSTNAMES = new Set(['localhost', '127.0.0.1']);
const DEFAULT_LLM_BASE_URL =
  'https://generativelanguage.googleapis.com/v1beta/openai/';
const DEFAULT_LLM_MODEL = 'gemini-3.1-flash-lite-preview';

function isRateLimited(ip: string): boolean {
  const now = Date.now();
  const entry = rateLimitMap.get(ip);

  if (!entry || now > entry.resetAt) {
    rateLimitMap.set(ip, { count: 1, resetAt: now + RATE_WINDOW_MS });
    return false;
  }

  entry.count++;
  return entry.count > RATE_LIMIT;
}

export function isAllowedOrigin(
  originHeader: string | null,
  requestUrl: string,
  allowLocalhost = false,
): boolean {
  if (!originHeader) {
    return false;
  }

  let origin: URL;
  let requestOrigin: URL;
  try {
    origin = new URL(originHeader);
    requestOrigin = new URL(requestUrl);
  } catch {
    return false;
  }

  if (allowLocalhost && LOCALHOST_HOSTNAMES.has(origin.hostname)) {
    return true;
  }

  return (
    origin.protocol === requestOrigin.protocol &&
    origin.host === requestOrigin.host
  );
}

export function getChatCompletionsUrl(baseUrl: string): string {
  const normalizedBaseUrl = baseUrl.endsWith('/')
    ? baseUrl
    : `${baseUrl}/`;

  return new URL('chat/completions', normalizedBaseUrl).toString();
}

export function resolveUpstreamConfig(env: Env): {
  apiKey: string;
  chatCompletionsUrl: string;
  model: string;
} {
  const apiKey = env.LLM_API_KEY ?? env.GEMINI_API_KEY;
  if (!apiKey) {
    throw new Error('LLM_API_KEY or GEMINI_API_KEY is not configured');
  }

  const baseUrl = env.LLM_BASE_URL ?? DEFAULT_LLM_BASE_URL;
  const model = env.LLM_MODEL ?? DEFAULT_LLM_MODEL;

  return {
    apiKey,
    chatCompletionsUrl: getChatCompletionsUrl(baseUrl),
    model,
  };
}

export function buildUpstreamRequestBody(
  parsed: { messages?: unknown; stream?: unknown },
  model: string,
): {
  model: string;
  messages: unknown[];
  tools: Array<{
    type: 'function';
    function: {
      name: 'bash';
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
    };
  }>;
  stream: boolean;
} {
  const MAX_MESSAGES = 20;
  const messages = Array.isArray(parsed.messages)
    ? parsed.messages.slice(-MAX_MESSAGES)
    : [];

  return {
    model,
    messages,
    tools: [
      {
        type: 'function',
        function: {
          name: 'bash',
          description: 'Execute bash commands in the sandboxed rust-bash environment.',
          parameters: {
            type: 'object',
            properties: {
              command: { type: 'string', description: 'The bash command to execute' },
            },
            required: ['command'],
          },
        },
      },
    ],
    stream: parsed.stream === true,
  };
}

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const ip = request.headers.get('CF-Connecting-IP') ?? 'unknown';

  if (isRateLimited(ip)) {
    return new Response(
      JSON.stringify({ error: 'Rate limit exceeded. Try again in a minute.' }),
      { status: 429, headers: { 'Content-Type': 'application/json' } },
    );
  }

  // Origin check: only allow requests from our site (localhost only if ALLOW_LOCALHOST is set)
  const allowLocalhost = env.ALLOW_LOCALHOST === 'true';
  if (!isAllowedOrigin(request.headers.get('Origin'), request.url, allowLocalhost)) {
    return new Response(
      JSON.stringify({ error: 'Forbidden' }),
      { status: 403, headers: { 'Content-Type': 'application/json' } },
    );
  }

  let upstreamConfig:
    | {
        apiKey: string;
        chatCompletionsUrl: string;
        model: string;
      }
    | undefined;
  try {
    upstreamConfig = resolveUpstreamConfig(env);
  } catch (error) {
    const message =
      error instanceof Error ? error.message : 'Invalid upstream configuration';

    return new Response(
      JSON.stringify({ error: message }),
      { status: 500, headers: { 'Content-Type': 'application/json' } },
    );
  }

  // Parse and reconstruct the request body — never forward raw user input
  let parsed: { messages?: unknown; stream?: unknown };
  try {
    parsed = JSON.parse(await request.text()) as {
      messages?: unknown;
      stream?: unknown;
    };
  } catch {
    return new Response(
      JSON.stringify({ error: 'Invalid JSON' }),
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    );
  }

  const sanitizedBody = JSON.stringify(
    buildUpstreamRequestBody(parsed, upstreamConfig.model),
  );

  const response = await fetch(upstreamConfig.chatCompletionsUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${upstreamConfig.apiKey}`,
    },
    body: sanitizedBody,
  });

  // Stream the response through to the client
  return new Response(response.body, {
    status: response.status,
    headers: {
      'Content-Type': response.headers.get('Content-Type') ?? 'text/event-stream',
      'Cache-Control': 'no-cache',
    },
  });
};
