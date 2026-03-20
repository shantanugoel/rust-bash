/**
 * Preloaded file content for the virtual filesystem.
 *
 * These files are available in the VFS at boot time so the user
 * and agent can explore project files immediately.
 */

export const VFS_FILES: Record<string, string> = {
  '/home/user/README.md': `# rust-bash

A sandboxed bash interpreter, built in Rust.

## Features

- **80+ commands** — echo, grep, awk, sed, jq, curl, find, sort, uniq, wc, and more
- **Virtual filesystem** — in-memory, fully isolated
- **Full bash syntax** — pipes, redirects, subshells, functions, globs, arithmetic
- **Execution limits** — 10 configurable safety bounds
- **Network sandboxing** — URL allow-lists for curl
- **Embeddable** — use as a crate, CLI, WASM, or C FFI

## Quick Start

\`\`\`bash
cargo install rust-bash
rust-bash -c 'echo "Hello from rust-bash!"'
\`\`\`

## Usage as a Library

\`\`\`rust
use rust_bash::interpreter::Interpreter;
use rust_bash::virtual_fs::VirtualFs;

let mut fs = VirtualFs::new();
fs.write_file("/hello.txt", "Hello, world!").unwrap();

let mut interp = Interpreter::with_virtual_fs(fs);
let result = interp.run("cat /hello.txt | tr a-z A-Z");
assert_eq!(result.stdout, "HELLO, WORLD!\\n");
\`\`\`

## License

MIT
`,

  '/home/user/Cargo.toml': `[package]
name = "rust-bash"
version = "0.5.0"
edition = "2024"
description = "A sandboxed bash interpreter with virtual filesystem"
license = "MIT"
repository = "https://github.com/shantanugoel/rust-bash"

[features]
default = ["cli"]
cli = []
wasm = ["wasm-bindgen", "js-sys"]

[dependencies]
glob-match = "0.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
`,

  '/home/user/src/main.rs': `use rust_bash::interpreter::Interpreter;
use std::io::{self, BufRead, Write};

fn main() {
    let mut interp = Interpreter::new();
    let stdin = io::stdin();

    loop {
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap() == 0 {
            break;
        }

        let result = interp.run(line.trim());
        if !result.stdout.is_empty() {
            print!("{}", result.stdout);
        }
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
    }
}
`,

  '/home/user/examples/fibonacci.sh': `#!/bin/bash
# Fibonacci sequence generator

n=\${1:-10}
a=0
b=1

echo "Fibonacci sequence (first $n numbers):"
for ((i = 0; i < n; i++)); do
  echo "  F($i) = $a"
  temp=$((a + b))
  a=$b
  b=$temp
done
`,

  '/home/user/examples/word-count.sh': `#!/bin/bash
# Count words in all files under a directory

dir=\${1:-.}
total=0

for file in $(find "$dir" -type f -name "*.md" 2>/dev/null); do
  count=$(wc -w < "$file")
  total=$((total + count))
  echo "  $file: $count words"
done

echo "Total: $total words"
`,

  '/home/user/docs/architecture.md': `# Architecture Overview

rust-bash is organized into layered subsystems:

1. **Lexer/Parser** — Tokenizes bash input into an AST
2. **Interpreter** — Walks the AST, executing commands
3. **Virtual Filesystem** — In-memory file storage with path resolution
4. **Command System** — 80+ built-in commands (echo, grep, awk, etc.)
5. **Execution Safety** — Configurable limits on loops, output, time
6. **Integration Layer** — CLI, WASM, C FFI, and npm package

All file I/O goes through VirtualFs. No \`std::fs\` in command code.
All process execution is in-process. No \`std::process::Command\`.
`,
};
