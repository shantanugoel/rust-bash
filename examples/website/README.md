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
|  | Rate limiter   |--->| Proxy to LLM     |                      |
|  | (10 req/min/IP)|    | API (streaming)  |                      |
|  +----------------+    +------------------+                      |
+------------------------------------------------------------------+
                               |
                               v
+------------------------------------------------------------------+
|       OPENAI-COMPATIBLE LLM (Gemini default, configurable)       |
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
| `functions/api/chat.ts` | Shared Cloudflare Pages Function logic for the LLM proxy |
| `functions/api/chat/completions.ts` | Pages route entrypoint for the OpenAI-compatible `chat/completions` API |

## Development

```bash
cd examples/website
npm install
npm run build
npm run dev
```

Use a second terminal to run the Pages Functions locally:

```bash
bunx wrangler pages dev dist --binding GEMINI_API_KEY=your-key-here
```

To test another provider locally, add `LLM_API_KEY` and optionally `LLM_BASE_URL`
and `LLM_MODEL` bindings to the same command.

### Local Testing

The local setup uses both Vite and Wrangler:

- `npm run dev` serves the frontend on `http://localhost:5173`
- `wrangler pages dev dist` serves the built Pages site and Functions, usually on `http://localhost:8788`
- Vite proxies `/api` requests to Wrangler, so the browser can talk to the local Pages Function while still using the Vite dev server

Recommended workflow:

```bash
# terminal 1
cd examples/website
npm run dev

# terminal 2
cd examples/website
bunx wrangler pages dev dist --binding GEMINI_API_KEY=your-key-here
```

Then open `http://localhost:5173`.

Use Wrangler by itself when you want a more production-like local check:

```bash
cd examples/website
npm run build
bunx wrangler pages dev dist --binding GEMINI_API_KEY=your-key-here
```

Then open the Wrangler URL directly, typically `http://localhost:8788`.

Notes:

- `wrangler pages dev dist` serves the built `dist/` output, not Vite's live bundle
- rerun `npm run build` before testing through Wrangler alone after frontend changes
- if you are using the Vite dev server plus Wrangler together, Vite handles the frontend and proxies `/api` to the Wrangler server configured in `vite.config.ts`

## Build

```bash
npm run build      # outputs to dist/
npm run preview    # preview production build
```

## Deployment

### Automated (GitHub Actions)

A workflow at `.github/workflows/deploy-website.yml` automatically builds the
WASM binary and deploys to Cloudflare Pages on **any `v*` tag push**.

**One-time setup:**

1. Create a Cloudflare Pages project:
   ```bash
   npx wrangler pages project create rust-bash-website --production-branch main
   ```
2. Add these GitHub repository secrets (`Settings → Secrets and variables → Actions`):
   | Secret | Where to get it |
   |--------|----------------|
   | `CLOUDFLARE_API_TOKEN` | [Cloudflare dashboard → API Tokens](https://dash.cloudflare.com/profile/api-tokens) — create a token with **Cloudflare Pages: Edit** permission |
   | `CLOUDFLARE_ACCOUNT_ID` | Cloudflare dashboard → any domain → **Overview** sidebar |
   | `GEMINI_API_KEY` | [Google AI Studio](https://aistudio.google.com/apikey) |
3. Set the `GEMINI_API_KEY` secret on the Pages project for the default Gemini setup:
   ```bash
   npx wrangler pages secret put GEMINI_API_KEY --project-name rust-bash-website
   ```
   To switch providers later without code changes, set `LLM_API_KEY` instead and
   optionally configure `LLM_BASE_URL` and `LLM_MODEL` in the Pages dashboard.
4. Deploy by pushing any tag:
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

### Manual deploy

```bash
# From repo root — build WASM first
./scripts/build-wasm.sh

# Then build and deploy the website
cd examples/website
npm install && npm run build
npx wrangler pages deploy dist/ --project-name rust-bash-website
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `GEMINI_API_KEY` | Yes, by default | Default Google Gemini API key. Used when `LLM_API_KEY` is not set. |
| `LLM_API_KEY` | Optional | API key for another OpenAI-compatible provider. Overrides `GEMINI_API_KEY` when set. |
| `LLM_BASE_URL` | Optional | OpenAI-compatible API base URL. Defaults to `https://generativelanguage.googleapis.com/v1beta/openai/`. |
| `LLM_MODEL` | Optional | Upstream model name. Defaults to `gemini-2.5-flash`. |

`LLM_BASE_URL` should be the provider's OpenAI-compatible base URL; the function
appends `chat/completions` to it.

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

- `agent "query"` sends the query to the configured LLM API via the CF Worker
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
- **OpenAI SDK** — Chat completions client (pointed at the configured proxy)
- **Cloudflare Pages** — Hosting + Functions (Workers)
- **Google Gemini 2.5 Flash** — default LLM (free tier)
