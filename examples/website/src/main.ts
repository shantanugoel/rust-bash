/**
 * Entry point — matrix rain loading transition + boot sequence.
 *
 * Loading sequence:
 * T=0.0s  Page load. Matrix rain fills screen. WASM + xterm.js load in background.
 * T=1.5s  Ready. Matrix rain fades out, terminal fades in.
 * T=2.0s  Clean terminal visible. Welcome screen. Typing animation begins.
 */

import { TerminalUI } from './terminal.js';

// ── Matrix Rain ──────────────────────────────────────────────────────

function startMatrixRain(canvas: HTMLCanvasElement): () => void {
  const ctx = canvas.getContext('2d')!;
  let w: number, h: number, columns: number;
  let drops: number[];

  const chars =
    'アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン' +
    '0123456789abcdef{}[]|/<>$#@!~';

  function resize() {
    w = canvas.width = window.innerWidth;
    h = canvas.height = window.innerHeight;
    const fontSize = 14;
    columns = Math.floor(w / fontSize);
    drops = Array.from({ length: columns }, () =>
      Math.random() * -100,
    );
  }

  resize();
  window.addEventListener('resize', resize);

  let animationId: number;

  function draw() {
    ctx.fillStyle = 'rgba(10, 10, 10, 0.06)';
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = '#00ff41';
    ctx.font = '14px monospace';

    for (let i = 0; i < columns; i++) {
      const char = chars[Math.floor(Math.random() * chars.length)]!;
      ctx.fillText(char, i * 14, drops[i]! * 14);
      if (drops[i]! * 14 > h && Math.random() > 0.975) {
        drops[i] = 0;
      }
      drops[i]!++;
    }

    animationId = requestAnimationFrame(draw);
  }

  animationId = requestAnimationFrame(draw);

  // Return cleanup function
  return () => {
    cancelAnimationFrame(animationId);
    window.removeEventListener('resize', resize);
  };
}

// ── Boot Sequence ────────────────────────────────────────────────────

async function boot() {
  const canvas = document.getElementById(
    'matrix-canvas',
  ) as HTMLCanvasElement;
  const app = document.getElementById('app')!;
  const terminalContainer = document.getElementById('terminal')!;

  // Start matrix rain immediately
  const stopRain = startMatrixRain(canvas);

  // Minimum display time for matrix rain (dramatic entry)
  const minRainTime = sleep(1500);

  // Initialize terminal (xterm.js loads here)
  const terminalUI = new TerminalUI(terminalContainer);

  // Wait for minimum rain time
  await minRainTime;

  // Transition: fade out matrix, fade in app
  canvas.classList.add('fade-out');
  app.classList.remove('hidden');
  app.classList.add('visible');

  // Wait for CSS transition to complete
  await sleep(600);

  // Clean up matrix rain (free memory)
  stopRain();
  canvas.remove();

  // Boot the terminal (welcome screen + auto-type)
  terminalUI.fit();
  await terminalUI.boot();
  terminalUI.focus();

  // Check for ?agent= URL parameter for deep-linking
  const params = new URLSearchParams(window.location.search);
  const agentQuery = params.get('agent');
  if (agentQuery) {
    // The boot already ran the initial demo; for a custom query,
    // we'd need to queue it. For now, the URL param is noted for
    // future implementation when the live agent is connected.
    console.info(`Deep-link agent query: ${agentQuery}`);
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
