# rust-bash FFI — Go Example

A complete example of embedding rust-bash in a Go application using cgo.

## Prerequisites

- Go 1.21+
- Rust toolchain (for building the shared library)
- C compiler (gcc or clang — required by cgo)

## Build

From the repository root:

```bash
cargo build --features ffi --release
```

This produces `target/release/librust_bash.so` (Linux) or `target/release/librust_bash.dylib` (macOS).

## Run

```bash
cd examples/ffi/go
CGO_ENABLED=1 go run main.go
```

The `#cgo` directives in `main.go` use `${SRCDIR}` relative paths, so library and header paths are resolved automatically — no extra environment variables needed.

If you get linker errors at runtime, set `LD_LIBRARY_PATH`:

```bash
LD_LIBRARY_PATH=../../../target/release go run main.go
```

On macOS, use `DYLD_LIBRARY_PATH` instead:

```bash
DYLD_LIBRARY_PATH=../../../target/release go run main.go
```

## Expected Output

```
rust-bash version: 0.1.0

--- cat /hello.txt ---
stdout: "Hello from Go!"
exit code: 0

--- echo $GREETING ---
stdout: "Hi there\n"

--- text processing pipeline ---
stdout: "Alice,30\nBob,25\n"

--- state persistence ---
MY_VAR = "42\n"

--- stderr capture ---
stderr: "this is stderr\n"

--- non-zero exit ---
exit code: 42

--- error handling ---
Expected error: JSON parse error: ...

All examples completed successfully!
```

(Version number and error message details may vary.)

## Troubleshooting

### `cgo: not enabled`

Set `CGO_ENABLED=1` and ensure a C compiler is available:

```bash
CGO_ENABLED=1 go run main.go
```

### Linker errors: `cannot find -lrust_bash`

The shared library hasn't been built yet. Run from the repository root:

```bash
cargo build --features ffi --release
```

### Runtime error: `librust_bash.so: cannot open shared object file`

The dynamic linker can't find the library. Set the library path:

```bash
LD_LIBRARY_PATH=../../../target/release go run main.go
```

### `undefined reference to rust_bash_*`

Ensure you built with `--features ffi`. Without this feature flag, the FFI symbols are not exported.
