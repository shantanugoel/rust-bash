# rust-bash FFI — Python Example

A complete example of embedding rust-bash in Python using `ctypes`.

## Prerequisites

- Python 3.x
- Rust toolchain (for building the shared library)

## Build

From the repository root:

```bash
cargo build --features ffi --release
```

This produces `target/release/librust_bash.so` (Linux) or `target/release/librust_bash.dylib` (macOS).

## Run

```bash
cd examples/ffi/python
LD_LIBRARY_PATH=../../../target/release python3 rust_bash_example.py
```

On macOS, use `DYLD_LIBRARY_PATH` instead:

```bash
DYLD_LIBRARY_PATH=../../../target/release python3 rust_bash_example.py
```

Alternatively, set the `RUST_BASH_LIB` environment variable to the full path of the shared library:

```bash
RUST_BASH_LIB=/path/to/librust_bash.so python3 rust_bash_example.py
```

## Expected Output

```
rust-bash version: 0.1.0

--- cat /hello.txt ---
stdout: 'Hello from Python!'
exit code: 0

--- echo $GREETING ---
stdout: 'Hi there\n'

--- text processing pipeline ---
stdout: 'Alice,30\nBob,25\n'

--- state persistence ---
MY_VAR = '42'

--- stderr capture ---
stderr: 'this is stderr\n'

--- non-zero exit ---
exit code: 42

--- error handling ---
Expected error: JSON parse error: ...

All examples completed successfully!
```

(Version number and error message details may vary.)

## What This Example Demonstrates

1. **Loading the shared library** with `ctypes.CDLL`
2. **Defining the ExecResult struct** matching the C header
3. **Declaring function signatures** (argtypes/restype)
4. **Creating a sandbox** with JSON configuration (files, env vars, cwd)
5. **Executing commands** and reading stdout/stderr (pointer+length, not null-terminated)
6. **State persistence** across multiple `exec` calls
7. **Error handling** — checking for NULL returns and reading `rust_bash_last_error()`
8. **Resource cleanup** with `try/finally`
