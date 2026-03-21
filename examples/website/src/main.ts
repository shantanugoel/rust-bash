/**
 * Entry point — "Wake up, Neo..." intro + CRT frame + boot sequence.
 *
 * Loading sequence:
 * T=0.0s  Page load. CRT frame visible. Black screen, cursor blinks.
 * T=1.0s  Types "Wake up, Neo..." one character at a time.
 * T=1.0s after typing finishes  Brief pause. Then reverse-deletes the text.
 * T=3.5s  Intro fades out, terminal fades in.
 * T=4.0s  Welcome screen + typing animation begins.
 */

import { TerminalUI } from './terminal.js';

// ── Intro Sequence ───────────────────────────────────────────────────

async function playIntro(): Promise<void> {
  const introEl = document.getElementById('intro')!;
  const textEl = document.getElementById('intro-text')!;
  const message = 'Wake up, Neo...';

  // Cursor blinks alone for a beat
  await sleep(1200);

  // Type the message
  for (const char of message) {
    textEl.textContent += char;
    await sleep(80 + Math.random() * 60);
  }

  // Brief pause before reverse-deleting the message
  await sleep(1000);

  // Reverse-delete one character at a time
  for (let i = message.length; i > 0; i--) {
    textEl.textContent = message.slice(0, i - 1);
    await sleep(35);
  }

  await sleep(300);

  // Fade out the intro
  introEl.classList.add('fade-out');
  await sleep(400);
  introEl.remove();
}

// ── Boot Sequence ────────────────────────────────────────────────────

async function boot() {
  const app = document.getElementById('app')!;
  const terminalContainer = document.getElementById('terminal')!;

  // Start WASM loading in parallel with intro animation
  const terminalUI = new TerminalUI(terminalContainer);
  const introPromise = playIntro();

  // Wait for intro to finish (WASM loads in background during this)
  await introPromise;

  // Show the app
  app.classList.remove('hidden');
  app.classList.add('visible');

  // Boot the terminal (welcome screen + auto-type)
  terminalUI.fit();
  await terminalUI.boot();
  terminalUI.focus();

  // Check for ?agent= URL parameter for deep-linking
  const params = new URLSearchParams(window.location.search);
  const agentQuery = params.get('agent');
  if (agentQuery) {
    // Sanitize: limit length, strip control characters, and escape for shell quoting
    const MAX_AGENT_QUERY_LENGTH = 500;
    const sanitized = agentQuery
      .slice(0, MAX_AGENT_QUERY_LENGTH)
      .replace(/[\x00-\x1f\x7f]/g, '') // strip control chars (ANSI escapes, etc.)
      .replace(/\\/g, '\\\\')           // escape backslashes first
      .replace(/"/g, '\\"');             // then escape double quotes
    if (sanitized.trim()) {
      await terminalUI.executeCommand(`agent "${sanitized}"`);
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Start boot on DOM ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
