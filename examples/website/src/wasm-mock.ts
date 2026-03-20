/**
 * Mock bash interpreter for development.
 *
 * Provides a minimal in-memory bash simulation when the real
 * rust-bash WASM binary is not available. Supports enough commands
 * to drive the showcase demo.
 */

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export class MockBash {
  private files: Map<string, string>;
  private cwd: string;
  private env: Map<string, string>;

  constructor(options?: { files?: Record<string, string>; cwd?: string }) {
    this.files = new Map();
    this.cwd = options?.cwd ?? '/home/user';
    this.env = new Map([
      ['HOME', '/home/user'],
      ['USER', 'user'],
      ['SHELL', '/bin/bash'],
      ['PWD', this.cwd],
      ['PATH', '/usr/local/bin:/usr/bin:/bin'],
    ]);

    // Seed VFS files
    if (options?.files) {
      for (const [path, content] of Object.entries(options.files)) {
        this.files.set(path, content);
      }
    }
  }

  async exec(command: string): Promise<ExecResult> {
    const trimmed = command.trim();
    if (!trimmed) return { stdout: '', stderr: '', exitCode: 0 };

    // Handle pipes (respecting quotes)
    const pipeSegments = this.splitPipeline(trimmed);
    if (pipeSegments.length > 1) {
      return this.execPipeline(pipeSegments);
    }

    return this.execSingle(trimmed);
  }

  writeFile(path: string, content: string): void {
    const resolved = this.resolvePath(path);
    this.files.set(resolved, content);
  }

  readFile(path: string): string {
    const resolved = this.resolvePath(path);
    const content = this.files.get(resolved);
    if (content === undefined) {
      throw new Error(`No such file: ${path}`);
    }
    return content;
  }

  getCwd(): string {
    return this.cwd;
  }

  getCommandNames(): string[] {
    return [
      'echo', 'cat', 'ls', 'pwd', 'cd', 'grep', 'find', 'wc', 'head',
      'tail', 'sort', 'uniq', 'tr', 'sed', 'awk', 'seq', 'date',
      'env', 'touch', 'mkdir', 'rev', 'help', 'true', 'false', 'clear',
    ];
  }

  private resolvePath(path: string): string {
    if (path.startsWith('/')) return path;
    if (path.startsWith('~/')) return `/home/user${path.slice(1)}`;
    return `${this.cwd}/${path}`.replace(/\/+/g, '/');
  }

  private splitPipeline(command: string): string[] {
    const segments: string[] = [];
    let current = '';
    let inSingle = false;
    let inDouble = false;

    for (let i = 0; i < command.length; i++) {
      const ch = command[i]!;
      if (ch === "'" && !inDouble) {
        inSingle = !inSingle;
        current += ch;
      } else if (ch === '"' && !inSingle) {
        inDouble = !inDouble;
        current += ch;
      } else if (ch === '\\' && inDouble && i + 1 < command.length) {
        current += ch + command[i + 1]!;
        i++;
      } else if (ch === '|' && !inSingle && !inDouble) {
        segments.push(current.trim());
        current = '';
      } else {
        current += ch;
      }
    }
    if (current.trim()) segments.push(current.trim());
    return segments;
  }

  private getFilesInDir(dir: string): string[] {
    const entries = new Set<string>();
    const prefix = dir.endsWith('/') ? dir : `${dir}/`;
    for (const path of this.files.keys()) {
      if (path.startsWith(prefix)) {
        const rest = path.slice(prefix.length);
        const firstPart = rest.split('/')[0]!;
        if (rest.includes('/')) {
          entries.add(`${firstPart}/`);
        } else {
          entries.add(firstPart);
        }
      }
    }
    return [...entries].sort();
  }

  private execPipeline(parts: string[]): ExecResult {
    let input = '';
    let lastResult: ExecResult = { stdout: '', stderr: '', exitCode: 0 };

    for (const part of parts) {
      lastResult = this.execSingle(part, input);
      input = lastResult.stdout;
      if (lastResult.exitCode !== 0) break;
    }

    return lastResult;
  }

  private execSingle(command: string, stdin = ''): ExecResult {
    // Parse command and args (simplified)
    const tokens = this.tokenize(command);
    if (tokens.length === 0) return { stdout: '', stderr: '', exitCode: 0 };

    const cmd = tokens[0]!;
    const args = tokens.slice(1);

    switch (cmd) {
      case 'echo': return this.cmdEcho(args);
      case 'cat': return this.cmdCat(args, stdin);
      case 'ls': return this.cmdLs(args);
      case 'pwd': return { stdout: `${this.cwd}\n`, stderr: '', exitCode: 0 };
      case 'cd': return this.cmdCd(args);
      case 'grep': return this.cmdGrep(args, stdin);
      case 'find': return this.cmdFind(args);
      case 'wc': return this.cmdWc(args, stdin);
      case 'head': return this.cmdHead(args, stdin);
      case 'tail': return this.cmdTail(args, stdin);
      case 'sort': return this.cmdSort(stdin);
      case 'uniq': return this.cmdUniq(stdin);
      case 'tr': return this.cmdTr(args, stdin);
      case 'sed': return this.cmdSed(args, stdin);
      case 'seq': return this.cmdSeq(args);
      case 'awk': return this.cmdAwk(args, stdin);
      case 'rev': return this.cmdRev(stdin);
      case 'date': return { stdout: `${new Date().toUTCString()}\n`, stderr: '', exitCode: 0 };
      case 'env': return this.cmdEnv();
      case 'touch': return this.cmdTouch(args);
      case 'mkdir': return this.cmdMkdir(args);
      case 'help': return this.cmdHelp();
      case 'true': return { stdout: '', stderr: '', exitCode: 0 };
      case 'false': return { stdout: '', stderr: '', exitCode: 1 };
      default:
        return { stdout: '', stderr: `${cmd}: command not found\n`, exitCode: 127 };
    }
  }

  private tokenize(command: string): string[] {
    const tokens: string[] = [];
    let i = 0;
    while (i < command.length) {
      // Skip whitespace
      while (i < command.length && command[i] === ' ') i++;
      if (i >= command.length) break;

      let token = '';
      const quote = command[i];
      if (quote === '"' || quote === "'") {
        i++; // skip opening quote
        while (i < command.length && command[i] !== quote) {
          if (command[i] === '\\' && quote === '"' && i + 1 < command.length) {
            i++;
            token += command[i]!;
          } else {
            token += command[i]!;
          }
          i++;
        }
        i++; // skip closing quote
        tokens.push(token);
      } else {
        while (i < command.length && command[i] !== ' ') {
          token += command[i]!;
          i++;
        }
        tokens.push(token);
      }
    }
    return tokens;
  }

  private cmdEcho(args: string[]): ExecResult {
    const output = args.join(' ');
    return { stdout: `${output}\n`, stderr: '', exitCode: 0 };
  }

  private cmdCat(args: string[], stdin: string): ExecResult {
    if (args.length === 0 && stdin) {
      return { stdout: stdin, stderr: '', exitCode: 0 };
    }
    let output = '';
    for (const arg of args) {
      const resolved = this.resolvePath(arg);
      const content = this.files.get(resolved);
      if (content === undefined) {
        return { stdout: output, stderr: `cat: ${arg}: No such file or directory\n`, exitCode: 1 };
      }
      output += content;
    }
    return { stdout: output, stderr: '', exitCode: 0 };
  }

  private cmdLs(args: string[]): ExecResult {
    const dir = args.filter(a => !a.startsWith('-'))[0] ?? '.';
    const resolved = this.resolvePath(dir);
    const entries = this.getFilesInDir(resolved);
    if (entries.length === 0) {
      return { stdout: '', stderr: '', exitCode: 0 };
    }
    const names = entries.map(e => e.replace(/\/$/, ''));
    return { stdout: names.join('  ') + '\n', stderr: '', exitCode: 0 };
  }

  private cmdCd(args: string[]): ExecResult {
    const target = args[0] ?? '/home/user';
    const resolved = this.resolvePath(target);
    this.cwd = resolved;
    this.env.set('PWD', resolved);
    return { stdout: '', stderr: '', exitCode: 0 };
  }

  private cmdGrep(args: string[], stdin: string): ExecResult {
    const flags = args.filter(a => a.startsWith('-'));
    const nonFlags = args.filter(a => !a.startsWith('-'));
    const pattern = nonFlags[0] ?? '';
    const recursive = flags.includes('-r') || flags.includes('-R');
    const caseInsensitive = flags.includes('-i');

    let re: RegExp;
    try {
      re = new RegExp(pattern, caseInsensitive ? 'i' : '');
    } catch {
      return { stdout: '', stderr: `grep: Invalid regular expression: ${pattern}\n`, exitCode: 2 };
    }

    let input = stdin;
    if (nonFlags.length > 1) {
      const filePath = this.resolvePath(nonFlags[1]!);
      const content = this.files.get(filePath);
      if (content === undefined) {
        return { stdout: '', stderr: `grep: ${nonFlags[1]}: No such file or directory\n`, exitCode: 2 };
      }
      input = content;
    } else if (recursive && nonFlags.length === 1) {
      let output = '';
      const cwdPrefix = this.cwd + '/';
      for (const [path, content] of this.files) {
        if (path.startsWith(cwdPrefix) || path === this.cwd) {
          const relPath = path.slice(this.cwd.length + 1);
          for (const line of content.split('\n')) {
            if (re.test(line)) {
              output += `${relPath}:${line}\n`;
            }
          }
        }
      }
      return { stdout: output, stderr: '', exitCode: output ? 0 : 1 };
    }

    const lines = input.split('\n').filter(line => re.test(line));
    const output = lines.length > 0 ? lines.join('\n') + '\n' : '';
    return { stdout: output, stderr: '', exitCode: lines.length > 0 ? 0 : 1 };
  }

  private cmdFind(args: string[]): ExecResult {
    const nameIdx = args.indexOf('-name');
    const typeIdx = args.indexOf('-type');
    const startDir = args.find(a => a.startsWith('/') || a === '.') ?? '.';
    const resolved = this.resolvePath(startDir);
    const resolvedPrefix = (resolved === '.' ? this.cwd : resolved) + '/';

    let pattern: RegExp | null = null;
    if (nameIdx >= 0 && args[nameIdx + 1]) {
      const glob = args[nameIdx + 1]!.replace(/\*/g, '.*').replace(/\?/g, '.');
      try {
        pattern = new RegExp(`^${glob}$`);
      } catch {
        return { stdout: '', stderr: `find: Invalid pattern: ${args[nameIdx + 1]}\n`, exitCode: 1 };
      }
    }

    const results: string[] = [];
    for (const path of this.files.keys()) {
      if (!path.startsWith(resolvedPrefix)) continue;
      const basename = path.split('/').pop()!;
      if (pattern && !pattern.test(basename)) continue;
      if (typeIdx >= 0 && args[typeIdx + 1] === 'd') continue;
      results.push(path);
    }

    return {
      stdout: results.length > 0 ? results.join('\n') + '\n' : '',
      stderr: '',
      exitCode: 0,
    };
  }

  private cmdWc(args: string[], stdin: string): ExecResult {
    let input = stdin;
    if (args.length > 0 && !args[0]!.startsWith('-')) {
      const resolved = this.resolvePath(args[0]!);
      input = this.files.get(resolved) ?? '';
    }
    const lines = input.split('\n').length - (input.endsWith('\n') ? 1 : 0);
    const words = input.split(/\s+/).filter(Boolean).length;
    const chars = input.length;

    if (args.includes('-l')) return { stdout: `${lines}\n`, stderr: '', exitCode: 0 };
    if (args.includes('-w')) return { stdout: `${words}\n`, stderr: '', exitCode: 0 };
    if (args.includes('-c')) return { stdout: `${chars}\n`, stderr: '', exitCode: 0 };
    return { stdout: `  ${lines}  ${words} ${chars}\n`, stderr: '', exitCode: 0 };
  }

  private cmdHead(args: string[], stdin: string): ExecResult {
    let n = 10;
    const nIdx = args.indexOf('-n');
    if (nIdx >= 0 && args[nIdx + 1]) n = parseInt(args[nIdx + 1]!, 10);

    let input = stdin;
    const fileArgs = args.filter(a => !a.startsWith('-') && (nIdx < 0 || args.indexOf(a) !== nIdx + 1));
    if (fileArgs.length > 0) {
      const resolved = this.resolvePath(fileArgs[0]!);
      const content = this.files.get(resolved);
      if (content === undefined) {
        return { stdout: '', stderr: `head: ${fileArgs[0]}: No such file or directory\n`, exitCode: 1 };
      }
      input = content;
    }

    const lines = input.split('\n').slice(0, n);
    return { stdout: lines.join('\n') + (input.endsWith('\n') ? '\n' : ''), stderr: '', exitCode: 0 };
  }

  private cmdTail(args: string[], stdin: string): ExecResult {
    let n = 10;
    const nIdx = args.indexOf('-n');
    if (nIdx >= 0 && args[nIdx + 1]) n = parseInt(args[nIdx + 1]!, 10);

    let input = stdin;
    const fileArgs = args.filter(a => !a.startsWith('-') && (nIdx < 0 || args.indexOf(a) !== nIdx + 1));
    if (fileArgs.length > 0) {
      const resolved = this.resolvePath(fileArgs[0]!);
      const content = this.files.get(resolved);
      if (content === undefined) {
        return { stdout: '', stderr: `tail: ${fileArgs[0]}: No such file or directory\n`, exitCode: 1 };
      }
      input = content;
    }

    const allLines = input.split('\n');
    if (allLines[allLines.length - 1] === '') allLines.pop();
    const lines = allLines.slice(-n);
    return { stdout: lines.join('\n') + '\n', stderr: '', exitCode: 0 };
  }

  private cmdSort(stdin: string): ExecResult {
    const lines = stdin.split('\n').filter(Boolean).sort();
    return { stdout: lines.join('\n') + '\n', stderr: '', exitCode: 0 };
  }

  private cmdUniq(stdin: string): ExecResult {
    const lines = stdin.split('\n');
    const result = lines.filter((line, i) => i === 0 || line !== lines[i - 1]);
    return { stdout: result.join('\n'), stderr: '', exitCode: 0 };
  }

  private cmdTr(args: string[], stdin: string): ExecResult {
    if (args.length < 2) {
      return { stdout: stdin, stderr: 'tr: missing operand\n', exitCode: 1 };
    }
    const set1 = args[0]!;
    const set2 = args[1]!;

    // Handle common case: a-z A-Z
    if (set1 === 'a-z' && set2 === 'A-Z') {
      return { stdout: stdin.toUpperCase(), stderr: '', exitCode: 0 };
    }
    if (set1 === 'A-Z' && set2 === 'a-z') {
      return { stdout: stdin.toLowerCase(), stderr: '', exitCode: 0 };
    }

    let output = stdin;
    for (let i = 0; i < Math.min(set1.length, set2.length); i++) {
      output = output.replaceAll(set1[i]!, set2[i]!);
    }
    return { stdout: output, stderr: '', exitCode: 0 };
  }

  private cmdSed(args: string[], stdin: string): ExecResult {
    // Handle s/pattern/replacement/flags
    const expr = args.find(a => a.startsWith('s')) ?? args[0] ?? '';
    const match = expr.match(/^s(.)(.*?)\1(.*?)\1([g]?)$/);
    if (!match) {
      return { stdout: stdin, stderr: `sed: invalid expression: ${expr}\n`, exitCode: 1 };
    }
    const [, , pattern, replacement, flags] = match;
    const re = new RegExp(pattern!, flags === 'g' ? 'g' : '');
    const lines = stdin.split('\n');
    const result = lines.map(line => line.replace(re, replacement!));
    return { stdout: result.join('\n'), stderr: '', exitCode: 0 };
  }

  private cmdSeq(args: string[]): ExecResult {
    const nums = args.map(Number);
    let start = 1, end = 1, step = 1;
    if (nums.length === 1) { end = nums[0]!; }
    else if (nums.length === 2) { start = nums[0]!; end = nums[1]!; }
    else if (nums.length >= 3) { start = nums[0]!; step = nums[1]!; end = nums[2]!; }

    const result: number[] = [];
    for (let i = start; step > 0 ? i <= end : i >= end; i += step) {
      result.push(i);
    }
    return { stdout: result.join('\n') + '\n', stderr: '', exitCode: 0 };
  }

  private cmdAwk(args: string[], stdin: string): ExecResult {
    const program = args[0] ?? '';
    // Handle simple summation: {s+=$1} END{print s}
    const sumMatch = program.match(/\{s\+?=\$(\d+)\}\s*END\s*\{print\s+s\}/);
    if (sumMatch) {
      const field = parseInt(sumMatch[1]!, 10) - 1;
      const lines = stdin.split('\n').filter(Boolean);
      let sum = 0;
      for (const line of lines) {
        const fields = line.trim().split(/\s+/);
        sum += parseFloat(fields[field] ?? '0');
      }
      return { stdout: `${sum}\n`, stderr: '', exitCode: 0 };
    }

    // Handle {print $N}
    const printMatch = program.match(/\{print \$(\d+)\}/);
    if (printMatch) {
      const field = parseInt(printMatch[1]!, 10) - 1;
      const lines = stdin.split('\n').filter(Boolean);
      const result = lines.map(line => {
        const fields = line.trim().split(/\s+/);
        return fields[field] ?? '';
      });
      return { stdout: result.join('\n') + '\n', stderr: '', exitCode: 0 };
    }

    // Handle simple {print} or {print $0}
    if (program === '{print}' || program === '{print $0}') {
      return { stdout: stdin, stderr: '', exitCode: 0 };
    }

    return { stdout: stdin, stderr: '', exitCode: 0 };
  }

  private cmdRev(stdin: string): ExecResult {
    const lines = stdin.split('\n');
    const result = lines.map(line => [...line].reverse().join(''));
    return { stdout: result.join('\n'), stderr: '', exitCode: 0 };
  }

  private cmdEnv(): ExecResult {
    const lines = [...this.env.entries()].map(([k, v]) => `${k}=${v}`);
    return { stdout: lines.join('\n') + '\n', stderr: '', exitCode: 0 };
  }

  private cmdTouch(args: string[]): ExecResult {
    for (const arg of args) {
      const resolved = this.resolvePath(arg);
      if (!this.files.has(resolved)) {
        this.files.set(resolved, '');
      }
    }
    return { stdout: '', stderr: '', exitCode: 0 };
  }

  private cmdMkdir(_args: string[]): ExecResult {
    // Directories are implicit in the VFS (created by file paths)
    return { stdout: '', stderr: '', exitCode: 0 };
  }

  private cmdHelp(): ExecResult {
    const cmds = this.getCommandNames();
    return {
      stdout: `Built-in commands: ${cmds.join(', ')}\n\nType \`agent "your question"\` to talk to the AI assistant.\n`,
      stderr: '',
      exitCode: 0,
    };
  }
}
