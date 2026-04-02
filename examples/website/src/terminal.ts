/**
 * Terminal integration — xterm.js + rust-bash bridge.
 *
 * The terminal IS the entire UI. xterm.js handles rendering; this module
 * bridges keystrokes to the bash instance (mock or WASM) and the agent.
 */

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';

import { createBash, type BashInstance } from './bash-loader.js';
import { VFS_FILES_URL, VFS_PLACEHOLDER_FILES } from './content.js';
import { runAgentLoop, type AgentEvent } from './agent.js';
import {
  CACHED_INITIAL_RESPONSE,
  replayCache,
} from './cached-initial-response.js';

const PROMPT = '\x1b[32m🦀  rust-bash\x1b[0m:\x1b[36m~\x1b[0m$ ';
const GITHUB_REPO_URL = 'https://github.com/shantanugoel/rust-bash';
const INITIAL_AGENT_COMMAND = 'agent "is this the matrix?"';

// Font size breakpoints (match the CSS @media (max-width: 480px) query)
const NARROW_VIEWPORT_THRESHOLD = 480;
const EXTRA_NARROW_THRESHOLD = 360;
const FONT_SIZE_DEFAULT = 14;
const FONT_SIZE_NARROW = 12;
const FONT_SIZE_EXTRA_NARROW = 11;
// Minimum terminal column count before the wide ASCII art wraps on mobile
const MIN_COLS_FOR_WIDE_WELCOME = 55;
// Commands that produce correct output against the placeholder VFS (which has
// the full file tree but empty content for deferred files).  `cat` is
// intentionally excluded: although `cat README.md` would work (inline), `cat
// src/lib.rs` would silently return empty, which is more confusing than the
// brief one-time deferred-load delay.
const PLACEHOLDER_SAFE_COMMANDS = new Set([
  'cd',
  'clear',
  'date',
  'echo',
  // `find` is safe for path-based predicates (`-name`, `-type`), but predicates
  // that inspect content or size (`-size +0`, `-newer`) can return wrong results
  // against zero-length placeholders.  This is an acceptable edge case for a demo.
  'find',
  'hostname',
  'ls',
  'printf',
  'pwd',
  'realpath',
  'seq',
  'type',
  'uname',
  'which',
  'whoami',
]);
// Conservative: triggers deferred loading for any line containing shell
// metacharacters, even if they appear inside quotes (e.g. `echo "a | b"`).
// Parsing quoting correctly would be complex and the false-positive cost is a
// one-time deferred fetch, which is acceptable.
const SHELL_META_CHARS = /[|&;<>()`]/;

function hyperlink(label: string, url: string): string {
  return `\x1b]8;;${url}\x1b\\${label}\x1b]8;;\x1b\\`;
}

const WELCOME_WIDE = `\x1b[38;2;247;76;0m
                    __  __               __
   _______  _______/ /_/ /_  ____ ______/ /_
  / ___/ / / / ___/ __/ __ \\/ __ \`/ ___/ __ \\
 / /  / /_/ (__  ) /_/ /_/ / /_/ (__  ) / / /
/_/   \\__,_/____/\\__/_.___/\\__,_/____/_/ /_/
\x1b[0m
${hyperlink(
  ' 🦀  A sandboxed bash interpreter for AI Agents. Built in Rust, to run anywhere.',
  GITHUB_REPO_URL,
)}

 \x1b[33m80+ commands\x1b[0m · \x1b[33mVirtual filesystem\x1b[0m · \x1b[33mExecution limits\x1b[0m · \x1b[33mNetwork sandboxing\x1b[0m

 Try:  \x1b[36mls\x1b[0m              \x1b[36mcat README.md\x1b[0m         \x1b[36mecho '{"a":1}' | grep a\x1b[0m
       \x1b[36mgrep -r bash .\x1b[0m  \x1b[36mfind / -name "*.md"\x1b[0m   \x1b[36mseq 1 10 | awk '{s+=$1} END{print s}'\x1b[0m
       \x1b[36magent "<ask anything>"\x1b[0m

`;

function buildWelcomeNarrow(): string {
  return `
\x1b[38;2;247;76;0m🦀 rust-bash\x1b[0m  ${hyperlink('github', GITHUB_REPO_URL)}

\x1b[33m80+ cmds\x1b[0m · \x1b[33mVirtual FS\x1b[0m · \x1b[33mSandboxed\x1b[0m

 Try: \x1b[36mls\x1b[0m  \x1b[36mcat README.md\x1b[0m
      \x1b[36magent "is this the matrix?"\x1b[0m

`;
}

export class TerminalUI {
  private term: Terminal;
  private fitAddon: FitAddon;
  private bash: BashInstance | null = null;
  private bashPromise: Promise<BashInstance> | null = null;
  private lineBuffer = '';
  private history: string[] = [];
  private historyIndex = -1;
  private savedBuffer = '';
  private isProcessing = false;
  private interruptFn: (() => void) | null = null;
  private deferredRepoFilesLoaded = false;
  private deferredRepoFilesPromise: Promise<void> | null = null;

  constructor(container: HTMLElement) {
    const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const isNarrowViewport = window.innerWidth <= NARROW_VIEWPORT_THRESHOLD;
    const fontSize = isNarrowViewport
      ? (window.innerWidth <= EXTRA_NARROW_THRESHOLD ? FONT_SIZE_EXTRA_NARROW : FONT_SIZE_NARROW)
      : FONT_SIZE_DEFAULT;

    this.term = new Terminal({
      fontFamily:
        "'JetBrains Mono', 'Cascadia Code', 'SF Mono', Consolas, monospace",
      fontSize,
      theme: isDark
        ? {
            background: '#0a0a0a',
            foreground: '#b0ffb0',
            cursor: '#00ff41',
            cursorAccent: '#0a0a0a',
            selectionBackground: '#00ff4133',
            green: '#00ff41',
            cyan: '#00d4ff',
            red: '#ff4444',
            yellow: '#ffcc00',
            blue: '#5577ff',
          }
        : {
            background: '#f5f5f0',
            foreground: '#1a1a2e',
            cursor: '#16a34a',
            cursorAccent: '#f5f5f0',
            selectionBackground: '#16a34a33',
            green: '#16a34a',
            cyan: '#0891b2',
            red: '#dc2626',
            yellow: '#ca8a04',
            blue: '#2563eb',
          },
      cursorBlink: true,
      convertEol: true,
      scrollback: 5000,
    });

    this.fitAddon = new FitAddon();
    this.term.loadAddon(this.fitAddon);
    this.term.loadAddon(new WebLinksAddon());

    this.term.open(container);
    this.fitAddon.fit();

    this.setupInput();
    this.setupResize();
  }

  /** Start warming the shell before the intro finishes. */
  prepareShell(): void {
    if (!this.bashPromise) {
      this.bashPromise = createBash({
        files: VFS_PLACEHOLDER_FILES,
        cwd: '/home/user',
      });
    }
  }

  /** Boot sequence: show terminal immediately, then reveal the prompt once the shell is ready. */
  async boot(): Promise<void> {
    this.prepareShell();
    const bashPromise = this.bashPromise!;

    // Write welcome screen immediately — doesn't need WASM
    const welcome = this.term.cols < MIN_COLS_FOR_WIDE_WELCOME ? buildWelcomeNarrow() : WELCOME_WIDE;
    this.term.write(welcome);
    this.term.writeln('\x1b[2mLoading shell...\x1b[0m');

    // Now await WASM — likely already ready since it loaded during the intro animation
    try {
      this.bash = await bashPromise;
      this.term.write('\x1b[1A\x1b[2K\r');
      this.showPrompt();
      await this.typeText(INITIAL_AGENT_COMMAND, 50);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      this.term.writeln(
        `\x1b[31m⚠ Failed to load WASM: ${msg}\x1b[0m\r\n` +
        'Make sure you have run: ./scripts/build-wasm.sh\r\n',
      );
      this.showPrompt();
    }
  }

  /** Focus the terminal. */
  focus(): void {
    this.term.focus();
  }

  /** Refit terminal to container. */
  fit(): void {
    this.fitAddon.fit();
  }

  /** Simulate typing and executing a command programmatically. */
  async executeCommand(
    cmd: string,
    options: { animate?: boolean } = {},
  ): Promise<void> {
    this.lineBuffer = '';
    if (options.animate) {
      await this.typeText(cmd, 32);
      await sleep(120);
    } else {
      this.lineBuffer = cmd;
      this.term.write(cmd);
    }
    this.term.write('\r\n');
    const line = this.lineBuffer.trim();
    if (line) {
      this.history.push(line);
    }
    await this.handleInput(line);
  }

  private showPrompt(): void {
    this.term.write(PROMPT);
    this.lineBuffer = '';
    this.historyIndex = -1;
    this.savedBuffer = '';
  }

  private async ensureDeferredRepoFilesLoaded(): Promise<void> {
    if (this.deferredRepoFilesLoaded) {
      return;
    }

    if (!this.bash) {
      throw new Error('Shell not loaded');
    }

    if (!this.deferredRepoFilesPromise) {
      this.term.writeln('\x1b[2mLoading tracked project files...\x1b[0m');
      this.deferredRepoFilesPromise = fetch(VFS_FILES_URL, { cache: 'force-cache' })
        .then(async (response) => {
          if (!response.ok) {
            throw new Error(`Unable to load tracked project files (${response.status})`);
          }

          return await response.json() as Record<string, string>;
        })
        .then((files) => {
          // Note: this overwrites any user edits to placeholder files made
          // before deferred loading was triggered (unlikely in a demo).
          for (const [path, content] of Object.entries(files)) {
            this.bash!.writeFile(path, content);
          }

          this.deferredRepoFilesLoaded = true;
          // Erase the "Loading tracked project files..." status line
          this.term.write('\x1b[1A\x1b[2K\r');
        })
        .catch((err: unknown) => {
          // Erase the "Loading tracked project files..." status line
          this.term.write('\x1b[1A\x1b[2K\r');
          this.deferredRepoFilesPromise = null;
          throw err;
        });
    }

    await this.deferredRepoFilesPromise;
  }

  private commandNeedsDeferredRepoFiles(line: string): boolean {
    const trimmed = line.trim();
    if (SHELL_META_CHARS.test(trimmed)) {
      return true;
    }

    const tokens = trimmed.split(/\s+/);
    const command = tokens[0]!;
    if (command === 'ls' && tokens.slice(1).some((arg) => arg.startsWith('-'))) {
      return true;
    }

    return !PLACEHOLDER_SAFE_COMMANDS.has(command);
  }

  private setupInput(): void {
    this.term.onKey(({ key, domEvent }) => {
      // If agent is animating, interrupt it on any keypress
      if (this.interruptFn) {
        this.interruptFn();
        this.interruptFn = null;
        return;
      }

      if (this.isProcessing) return;

      const code = domEvent.keyCode;

      if (code === 13) {
        // Enter
        this.term.write('\r\n');
        const line = this.lineBuffer.trim();
        if (line) {
          this.history.push(line);
        }
        this.handleInput(line);
        return;
      }

      if (code === 8) {
        // Backspace
        if (this.lineBuffer.length > 0) {
          this.lineBuffer = this.lineBuffer.slice(0, -1);
          this.term.write('\b \b');
        }
        return;
      }

      if (code === 38) {
        // Up arrow — history
        if (this.history.length === 0) return;
        if (this.historyIndex === -1) {
          this.savedBuffer = this.lineBuffer;
          this.historyIndex = this.history.length - 1;
        } else if (this.historyIndex > 0) {
          this.historyIndex--;
        }
        this.replaceLineBuffer(this.history[this.historyIndex]!);
        return;
      }

      if (code === 40) {
        // Down arrow — history
        if (this.historyIndex === -1) return;
        if (this.historyIndex < this.history.length - 1) {
          this.historyIndex++;
          this.replaceLineBuffer(this.history[this.historyIndex]!);
        } else {
          this.historyIndex = -1;
          this.replaceLineBuffer(this.savedBuffer);
        }
        return;
      }

      if (code === 9) {
        // Tab — completion
        domEvent.preventDefault();
        this.handleTabCompletion();
        return;
      }

      // Ctrl+C — cancel current input
      if (domEvent.ctrlKey && (code === 67 /* C */ || domEvent.key === 'c')) {
        this.term.write('^C\r\n');
        this.lineBuffer = '';
        this.isProcessing = false;
        this.showPrompt();
        return;
      }

      // Ctrl+L — clear screen
      if (domEvent.ctrlKey && (code === 76 /* L */ || domEvent.key === 'l')) {
        this.term.clear();
        this.showPrompt();
        this.term.write(this.lineBuffer);
        return;
      }

      // Ignore other control keys
      if (domEvent.ctrlKey || domEvent.altKey || domEvent.metaKey) return;

      // Regular character
      if (key.length === 1 && !domEvent.ctrlKey) {
        this.lineBuffer += key;
        this.term.write(key);
      }
    });
  }

  private replaceLineBuffer(newContent: string): void {
    // Erase current line buffer from display
    const eraseCount = this.lineBuffer.length;
    this.term.write('\b'.repeat(eraseCount) + ' '.repeat(eraseCount) + '\b'.repeat(eraseCount));
    this.lineBuffer = newContent;
    this.term.write(newContent);
  }

  private handleTabCompletion(): void {
    const input = this.lineBuffer;
    if (!input) return;
    if (!this.bash) {
      // The prompt is shown before WASM finishes loading, so completion must
      // tolerate a brief pre-shell window instead of crashing on undefined bash.
      return;
    }

    // If no space yet, complete command names
    const spaceIdx = input.indexOf(' ');
    if (spaceIdx === -1) {
      const partial = input;
      const commands = [...this.bash.getCommandNames(), 'agent', 'clear'];
      const matches = commands.filter((c) => c.startsWith(partial));

      if (matches.length === 1) {
        const completion = matches[0]!.slice(partial.length) + ' ';
        this.lineBuffer += completion;
        this.term.write(completion);
      } else if (matches.length > 1) {
        this.term.write('\r\n');
        this.term.writeln(matches.join('  '));
        this.showPrompt();
        this.lineBuffer = partial;
        this.term.write(partial);
      }
    } else {
      // Complete filenames for arguments.
      // Uses the placeholder VFS, which has the full directory tree — filenames
      // are correct even before deferred files are loaded.  We intentionally
      // skip triggering deferred loading here to keep tab completion synchronous.
      const lastSpace = input.lastIndexOf(' ');
      const partial = input.slice(lastSpace + 1);
      const dir = partial.includes('/')
        ? partial.slice(0, partial.lastIndexOf('/') + 1) || '/'
        : '.';
      const prefix = partial.includes('/')
        ? partial.slice(partial.lastIndexOf('/') + 1)
        : partial;

      const allFiles = this.bash.listDir(dir);
      const matches = allFiles.filter((f) => f.replace(/\/$/, '').startsWith(prefix));

      if (matches.length === 1) {
        const completion = matches[0]!.replace(/\/$/, '').slice(prefix.length);
        this.lineBuffer += completion;
        this.term.write(completion);
      } else if (matches.length > 1) {
        this.term.write('\r\n');
        this.term.writeln(matches.map(f => f.replace(/\/$/, '')).join('  '));
        this.showPrompt();
        this.lineBuffer = input;
        this.term.write(input);
      }
    }
  }

  private async handleInput(line: string): Promise<void> {
    if (!line) {
      this.showPrompt();
      return;
    }

    if (!this.bash) {
      this.term.writeln(
        '\x1b[31m⚠ Shell not loaded. Run ./scripts/build-wasm.sh and reload.\x1b[0m',
      );
      this.showPrompt();
      return;
    }

    this.isProcessing = true;

    try {
      if (line === INITIAL_AGENT_COMMAND) {
        await this.runCachedInitial({ animate: false });
      } else if (line.startsWith('agent ') || line === 'agent') {
        const query = line.slice(6).trim().replace(/^["']|["']$/g, '');
        if (!query) {
          this.term.writeln(
            'Usage: agent "your question"',
          );
          this.term.writeln(
            'Example: agent "is this the matrix?"',
          );
        } else {
          try {
            await this.ensureDeferredRepoFilesLoaded();
          } catch {
            this.term.writeln(
              '\x1b[33m⚠ Could not load full project files — running with partial data\x1b[0m',
            );
          }
          await this.handleAgentQuery(query);
        }
      } else if (line === 'clear') {
        this.term.clear();
      } else {
        if (this.commandNeedsDeferredRepoFiles(line)) {
          try {
            await this.ensureDeferredRepoFilesLoaded();
          } catch {
            this.term.writeln(
              '\x1b[33m⚠ Could not load full project files — running with partial data\x1b[0m',
            );
          }
        }

        const result = await this.bash.exec(line);
        if (result.stdout) this.term.write(result.stdout);
        if (result.stderr) {
          this.term.write(`\x1b[31m${result.stderr}\x1b[0m`);
        }
      }
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      this.term.writeln(`\x1b[31mError: ${message}\x1b[0m`);
    }

    this.isProcessing = false;
    this.showPrompt();
  }

  private async handleAgentQuery(query: string): Promise<void> {
    this.term.writeln('');
    try {
      for await (const event of runAgentLoop(query, this.bash!)) {
        this.renderAgentEvent(event);
      }
    } catch {
      this.term.writeln(
        '\x1b[31m⚠️ Agent unavailable. Try exploring the shell directly!\x1b[0m',
      );
    }
    this.term.writeln('');
  }

  private renderAgentEvent(event: AgentEvent): void {
    switch (event.type) {
      case 'text':
        this.term.write(event.content);
        break;
      case 'tool_call': {
        this.term.writeln('');
        this.term.writeln(
          `  \x1b[2m$\x1b[0m \x1b[36m${event.command}\x1b[0m`,
        );
        if (event.result.stdout) {
          const lines = event.result.stdout.split('\n');
          const shown = lines.slice(0, 50);
          for (const line of shown) {
            if (line) this.term.writeln(`  ${line}`);
          }
          if (lines.length > 50) {
            this.term.writeln(
              `  \x1b[2m... (${lines.length - 50} more lines)\x1b[0m`,
            );
          }
        }
        if (event.result.stderr) {
          this.term.writeln(
            `  \x1b[31m${event.result.stderr}\x1b[0m`,
          );
        }
        this.term.writeln('');
        break;
      }
    }
  }

  private async runCachedInitial(options: { animate: boolean }): Promise<void> {
    this.term.writeln('');

    for await (const event of replayCache(
      CACHED_INITIAL_RESPONSE,
      this.bash ?? undefined,
      options,
    )) {
      // Capture the interrupt handler
      if ('interrupt' in event && event.interrupt) {
        this.interruptFn = event.interrupt;
      }
      this.renderAgentEvent(event);
    }

    this.interruptFn = null;
    this.term.writeln('');
  }

  private async typeText(
    text: string,
    baseDelay: number,
  ): Promise<void> {
    for (const char of text) {
      this.term.write(char);
      this.lineBuffer += char;
      await sleep(baseDelay + (Math.random() - 0.5) * 24);
    }
  }

  private setupResize(): void {
    const observer = new ResizeObserver(() => {
      this.fitAddon.fit();
    });
    observer.observe(this.term.element!.parentElement!);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
