# Convenience API

## Goal

Use the high-level features of the `Bash` class to control command execution: filter allowed commands, isolate per-exec state, pass arguments safely, normalize scripts, register transform plugins, and access the virtual filesystem directly.

## Command Filtering

Restrict which commands a script can execute with the `commands` allow-list:

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  commands: ['echo', 'cat', 'grep', 'jq'],
  files: { '/data.json': '{"status": "ok"}' },
});

// Allowed commands work normally
const result = await bash.exec('cat /data.json | jq .status');
console.log(result.stdout); // '"ok"\n'

// Commands not in the allow-list are rejected
const blocked = await bash.exec('rm /data.json');
console.log(blocked.stderr);   // "rm: command not allowed\n"
console.log(blocked.exitCode); // 127
```

This is useful for least-privilege execution — give AI agents or untrusted scripts access to only the commands they need.

## Per-Exec Environment and CWD Isolation

Override environment variables and working directory for a single `exec()` call without affecting the shell's persistent state:

```typescript
const bash = await Bash.create(createWasmBackend, {
  env: { USER: 'default', HOME: '/home/default' },
  cwd: '/',
});

// Per-exec overrides are temporary
const result = await bash.exec('echo $USER in $PWD', {
  env: { USER: 'override' },
  cwd: '/tmp',
});
console.log(result.stdout); // "override in /tmp\n"

// Shell state is unchanged after exec
const check = await bash.exec('echo $USER in $PWD');
console.log(check.stdout); // "default in /\n"
```

### Replacing the Entire Environment

By default, per-exec `env` values are merged with the shell's environment. Use `replaceEnv` to start with a clean slate:

```typescript
const result = await bash.exec('env | sort', {
  env: { ONLY_THIS: 'variable' },
  replaceEnv: true,
});
console.log(result.stdout); // "ONLY_THIS=variable\n"
```

## Safe Argument Passing

The `args` option passes arguments to a script without shell expansion, preventing injection attacks:

```typescript
// UNSAFE: user input is interpreted by the shell
const userInput = '$(rm -rf /)';
await bash.exec(`echo ${userInput}`); // shell expansion happens!

// SAFE: args are shell-escaped automatically
const result = await bash.exec('echo', {
  args: [userInput],
});
console.log(result.stdout); // "$(rm -rf /)\n" — treated as literal text
```

Arguments are escaped by wrapping in single quotes with internal single quotes handled:

```typescript
// Multiple arguments
const result = await bash.exec('printf "%s\\n"', {
  args: ['hello world', "it's safe", 'path/to/file'],
});
// stdout: "hello world\nit's safe\npath/to/file\n"
```

## Script Normalization

When using template literals, leading whitespace from indentation is automatically stripped:

```typescript
// Without normalization, this script would have unwanted leading spaces
const result = await bash.exec(`
  echo "line one"
  echo "line two"
  if true; then
    echo "indented"
  fi
`);
console.log(result.stdout);
// "line one\nline two\nindented\n"
```

Normalization:
1. Removes leading and trailing empty lines (common with template literals)
2. Finds the minimum indentation across non-empty lines
3. Strips that indentation from all lines

This lets you write inline scripts with natural indentation in your code.

### Disabling Normalization with rawScript

If your script depends on exact whitespace (e.g., Makefile-style tabs), disable normalization:

```typescript
const result = await bash.exec(`
\techo "tab-indented"
\techo "must keep tabs"
`, { rawScript: true });
```

Heredoc content is preserved regardless of the `rawScript` setting — normalization only affects the script lines themselves, not heredoc bodies.

## Transform Plugins

Register plugins that modify scripts before execution. Plugins run after normalization and before the backend executes the command.

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend);

// A plugin that logs every command to a file
bash.registerTransformPlugin({
  name: 'audit-logger',
  transform(script: string): string {
    const timestamp = new Date().toISOString();
    return `echo "[${timestamp}] ${script.split('\n')[0]}" >> /var/log/audit.log\n${script}`;
  },
});

await bash.exec('echo hello');
const log = await bash.exec('cat /var/log/audit.log');
// log.stdout contains the timestamped audit entry
```

### Multiple Plugins

Plugins execute in registration order:

```typescript
bash.registerTransformPlugin({
  name: 'env-injector',
  transform(script) {
    return `export DEBUG=1\n${script}`;
  },
});

bash.registerTransformPlugin({
  name: 'error-handler',
  transform(script) {
    return `set -e\n${script}`;
  },
});

// Execution order: env-injector → error-handler → execute
// Final script: "set -e\nexport DEBUG=1\n<original script>"
```

### Use Cases

- **Audit logging** — prepend logging commands to track what scripts are run
- **Error mode injection** — automatically add `set -e` or `set -o pipefail`
- **Variable injection** — inject environment setup before every script
- **Command instrumentation** — wrap commands for timing or output capture

## FileSystemProxy

The `bash.fs` property provides direct synchronous access to the virtual filesystem, bypassing shell execution:

```typescript
const bash = await Bash.create(createWasmBackend, {
  files: { '/data.txt': 'initial content' },
});

// Write files
bash.fs.writeFileSync('/output.txt', 'generated content');

// Read files
const content = bash.fs.readFileSync('/data.txt');
console.log(content); // "initial content"

// Check existence
if (bash.fs.existsSync('/output.txt')) {
  console.log('file exists');
}

// Create directories
bash.fs.mkdirSync('/dir/subdir', { recursive: true });

// List directory contents
const entries = bash.fs.readdirSync('/');
console.log(entries); // ["data.txt", "output.txt", "dir"]

// File metadata
const stat = bash.fs.statSync('/data.txt');
console.log(stat.isFile, stat.size); // true, 15

// Remove files and directories
bash.fs.rmSync('/output.txt');
bash.fs.rmSync('/dir', { recursive: true });
```

### FileSystemProxy API

| Method | Description |
|--------|-------------|
| `readFileSync(path)` | Read file contents as string |
| `writeFileSync(path, content)` | Write string content to a file |
| `existsSync(path)` | Check if a path exists |
| `mkdirSync(path, options?)` | Create directory (`{ recursive: true }` for nested) |
| `readdirSync(path)` | List directory entries |
| `statSync(path)` | Get file metadata (`isFile`, `isDirectory`, `size`) |
| `rmSync(path, options?)` | Remove file or directory (`{ recursive: true }` for trees) |

The proxy is useful for setting up test fixtures, reading output files, and programmatic file manipulation without constructing shell commands.

## Putting It All Together

```typescript
import { Bash, defineCommand, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();

const validate = defineCommand('validate-json', async (args, ctx) => {
  try {
    JSON.parse(ctx.stdin);
    return { stdout: 'valid\n', stderr: '', exitCode: 0 };
  } catch (e) {
    return { stdout: '', stderr: `invalid JSON: ${e}\n`, exitCode: 1 };
  }
});

const bash = await Bash.create(createWasmBackend, {
  commands: ['cat', 'echo', 'validate-json', 'jq'],
  customCommands: [validate],
  files: { '/config.json': '{"port": 8080}' },
  env: { APP_ENV: 'test' },
});

// Add error handling to every script
bash.registerTransformPlugin({
  name: 'strict-mode',
  transform: (script) => `set -eo pipefail\n${script}`,
});

// Use safe args and per-exec isolation
const result = await bash.exec('cat /config.json | validate-json', {
  env: { VERBOSE: '1' },
  cwd: '/tmp',
});

// Read results via filesystem proxy
bash.fs.writeFileSync('/result.txt', result.stdout);
const saved = bash.fs.readFileSync('/result.txt');
console.log(saved); // "valid\n"
```

## Next Steps

- [Getting Started](getting-started.md) — basic Bash class setup and usage
- [Custom Commands](custom-commands.md) — create domain-specific commands
- [npm Package](npm-package.md) — installation and package exports
