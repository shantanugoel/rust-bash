# rust-bash Showcase Website

Interactive demo of rust-bash running in the browser via WASM.

## Architecture

```
+------------------------------------------------------------------+
|                          BROWSER                                 |
|  +-----------+    +------------+    +------------------+         |
|  | xterm.js  |-->| rust-bash  |-->| Virtual FS       |         |
|  | Terminal  |   | (WASM)     |   | (in-memory)      |         |
|  +-----------+    +------------+    +------------------+         |
|       |                  ^                                       |
|       | `agent` cmd      | tool_call → exec locally via WASM    |
|       v                  |                                       |
|  +--------------------------------------------------+           |
|  |        Client-side agent loop                     |           |
|  |  1. POST messages to CF Worker                    |           |
|  |  2. Parse SSE stream                              |           |
|  |  3. On tool_call → exec via WASM → send result    |           |
|  |  4. Repeat until final text response              |           |
|  +--------------------------------------------------+           |
+------------------------------|-----------------------------------+
                               | HTTPS (SSE stream)
                               v
+------------------------------------------------------------------+
|                    CLOUDFLARE WORKER (~30 lines)                  |
|  +----------------+    +------------------+                      |
|  | Rate limiter   |--->| Proxy to Gemini  |                      |
|  | (10 req/min/IP)|    | API (streaming)  |                      |
|  +----------------+    +------------------+                      |
+------------------------------------------------------------------+
                               |
                               v
+------------------------------------------------------------------+
|              GOOGLE GEMINI 2.5 FLASH (free tier)                 |
+------------------------------------------------------------------+
```

## Key Files

| File | Purpose |
|------|---------|
| `src/main.ts` | Entry point — matrix rain transition, boot sequence |
| `src/terminal.ts` | xterm.js integration, input handling, agent rendering |
| `src/agent.ts` | Client-side agent loop (async generator) |
| `src/cached-initial-response.ts` | Hand-crafted first demo response |
| `src/wasm-mock.ts` | Development mock for bash (used until WASM binary is built) |
| `src/content.ts` | Preloaded VFS file content |
| `functions/api/chat.ts` | Cloudflare Pages Function — Gemini proxy |

## Development

```bash
cd examples/website
npm install
npm run dev
```

The dev server runs on `http://localhost:5173`. The agent proxy requires
a Cloudflare Worker running locally:

```bash
npx wrangler pages dev dist/ --binding GEMINI_API_KEY=your-key-here
```

## Build

```bash
npm run build      # outputs to dist/
npm run preview    # preview production build
```

## Deployment

### Cloudflare Pages

1. Connect GitHub repo to Cloudflare Pages
2. Set build command: `cd examples/website && npm install && npm run build`
3. Set build output directory: `examples/website/dist`
4. Add secret: `GEMINI_API_KEY` (Google AI Studio API key)

Or deploy manually:

```bash
npx wrangler pages deploy dist/
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `GEMINI_API_KEY` | Yes | Google Gemini API key (set as Worker secret) |

## How It Works

### Loading Sequence

1. **Matrix rain** fills the screen (canvas-based, katakana + code characters)
2. After 1.5s, matrix fades out, terminal fades in
3. Welcome screen with ASCII art and example commands
4. Auto-types `agent "is this the matrix?"` with typewriter effect
5. Plays cached response (no API call needed)
6. User takes control

### Agent

The `agent` command intercepts input before it reaches the bash interpreter:

- `agent "query"` sends the query to the Gemini API via the CF Worker
- The LLM can request tool calls (bash commands)
- Tool calls are executed **locally** via the WASM bash instance
- Results are sent back to the LLM for the next turn
- The user and agent share the same VFS — files created by the agent are visible to the user

### Cached Initial Response

The first `agent` query uses a hand-crafted `AgentEvent[]` array baked into
the bundle. This ensures:

- Zero API cost on first load
- Perfect, deterministic first impression
- No latency or loading state
- Tool calls still execute against the real WASM/mock bash

## Tech Stack

- **Vite** — Static site bundler
- **xterm.js** — Terminal emulator (`@xterm/xterm`, `@xterm/addon-fit`, `@xterm/addon-web-links`)
- **Tailwind CSS** — Utility styling
- **OpenAI SDK** — Chat completions client (pointed at Gemini via proxy)
- **Cloudflare Pages** — Hosting + Functions (Workers)
- **Google Gemini 2.5 Flash** — LLM (free tier)
