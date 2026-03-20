/**
 * Cloudflare Pages Function — LLM proxy.
 *
 * Proxies requests to Google Gemini 2.5 Flash (OpenAI-compatible endpoint).
 * API key stored as Worker secret. Rate limited to 10 req/min/IP.
 */

/// <reference types="@cloudflare/workers-types" />

interface Env {
  GEMINI_API_KEY: string;
}

// Simple in-memory rate limiter (resets on Worker cold start)
const rateLimitMap = new Map<string, { count: number; resetAt: number }>();
const RATE_LIMIT = 10;
const RATE_WINDOW_MS = 60_000;

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

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const ip = request.headers.get('CF-Connecting-IP') ?? 'unknown';

  if (isRateLimited(ip)) {
    return new Response(
      JSON.stringify({ error: 'Rate limit exceeded. Try again in a minute.' }),
      { status: 429, headers: { 'Content-Type': 'application/json' } },
    );
  }

  // Origin check: only allow requests from our site or localhost
  const origin = request.headers.get('Origin') ?? '';
  if (
    !origin.includes('rustbash.dev') &&
    !origin.includes('localhost') &&
    !origin.includes('127.0.0.1')
  ) {
    return new Response(
      JSON.stringify({ error: 'Forbidden' }),
      { status: 403, headers: { 'Content-Type': 'application/json' } },
    );
  }

  if (!env.GEMINI_API_KEY) {
    return new Response(
      JSON.stringify({ error: 'API key not configured' }),
      { status: 500, headers: { 'Content-Type': 'application/json' } },
    );
  }

  // Parse and reconstruct the request body — never forward raw user input
  let parsed: { messages?: unknown };
  try {
    parsed = JSON.parse(await request.text()) as { messages?: unknown };
  } catch {
    return new Response(
      JSON.stringify({ error: 'Invalid JSON' }),
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    );
  }

  const MAX_MESSAGES = 20;
  const messages = Array.isArray(parsed.messages)
    ? parsed.messages.slice(-MAX_MESSAGES)
    : [];

  const sanitizedBody = JSON.stringify({
    model: 'gemini-2.5-flash',
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
    stream: true,
  });

  const geminiUrl =
    'https://generativelanguage.googleapis.com/v1beta/openai/chat/completions';

  const response = await fetch(geminiUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${env.GEMINI_API_KEY}`,
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
