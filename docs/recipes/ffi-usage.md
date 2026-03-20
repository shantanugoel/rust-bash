# FFI Usage

Embed rust-bash in Python, Go, or any C-compatible language via the shared library.

## Building the Shared Library

```bash
# From the repository root
cargo build --features ffi --release
```

This produces:

| Platform | Library path |
|----------|-------------|
| Linux    | `target/release/librust_bash.so` |
| macOS    | `target/release/librust_bash.dylib` |
| Windows  | `target/release/rust_bash.dll` |

The C header is at `include/rust_bash.h`.

## The C API at a Glance

| Function | Description |
|----------|-------------|
| `rust_bash_create(config_json)` | Create a sandboxed shell. Pass `NULL` for defaults or a JSON config string. Returns `RustBash*`. |
| `rust_bash_exec(sb, command)` | Execute a shell command string. Returns `ExecResult*`. |
| `rust_bash_result_free(result)` | Free an `ExecResult*` returned by `rust_bash_exec`. |
| `rust_bash_free(sb)` | Free a `RustBash*` handle. |
| `rust_bash_last_error()` | Get the last error message for the current thread (or `NULL` if none). |
| `rust_bash_version()` | Get the library version as a static string. |

## JSON Configuration Reference

Pass a JSON string to `rust_bash_create` to configure the sandbox. All fields are optional — `"{}"` gives you a default sandbox.

```json
{
  "files": {
    "/data.txt": "file content",
    "/config.json": "{\"key\": \"value\"}"
  },
  "env": {
    "USER": "agent",
    "HOME": "/home/agent"
  },
  "cwd": "/",
  "limits": {
    "max_command_count": 10000,
    "max_execution_time_secs": 30,
    "max_loop_iterations": 10000,
    "max_output_size": 10485760,
    "max_call_depth": 100,
    "max_string_length": 10485760,
    "max_glob_results": 100000,
    "max_substitution_depth": 50,
    "max_heredoc_size": 10485760,
    "max_brace_expansion": 10000
  },
  "network": {
    "enabled": true,
    "allowed_url_prefixes": ["https://api.example.com/"],
    "allowed_methods": ["GET", "POST"],
    "max_response_size": 10485760,
    "max_redirects": 5,
    "timeout_secs": 30
  }
}
```

### Field Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `files` | `{path: content}` | `{}` | Pre-seed the virtual filesystem with files (text content only). |
| `env` | `{name: value}` | `{}` | Set environment variables in the sandbox. |
| `cwd` | `string` | `"/"` | Initial working directory. Created automatically if it doesn't exist. |
| `limits.max_command_count` | `integer` | `10000` | Maximum number of simple commands to execute. |
| `limits.max_execution_time_secs` | `integer` | `30` | Wall-clock timeout in seconds. |
| `limits.max_loop_iterations` | `integer` | `10000` | Maximum iterations across all loops. |
| `limits.max_output_size` | `integer` | `10485760` | Maximum combined stdout+stderr bytes. |
| `limits.max_call_depth` | `integer` | `100` | Maximum function/subshell call depth. |
| `limits.max_string_length` | `integer` | `10485760` | Maximum length of any single string value. |
| `limits.max_glob_results` | `integer` | `100000` | Maximum number of glob expansion results. |
| `limits.max_substitution_depth` | `integer` | `50` | Maximum nesting depth for command substitutions. |
| `limits.max_heredoc_size` | `integer` | `10485760` | Maximum size of a here-document. |
| `limits.max_brace_expansion` | `integer` | `10000` | Maximum number of brace expansion results. |
| `network.enabled` | `boolean` | `false` | Whether `curl` can make HTTP requests. |
| `network.allowed_url_prefixes` | `[string]` | `[]` | URL prefixes that `curl` is allowed to access. |
| `network.allowed_methods` | `[string]` | `["GET", "POST"]` | Allowed HTTP methods. |
| `network.max_response_size` | `integer` | `10485760` | Maximum HTTP response body size in bytes. |
| `network.max_redirects` | `integer` | `5` | Maximum number of HTTP redirects to follow. |
| `network.timeout_secs` | `integer` | `30` | HTTP request timeout in seconds. |

## Memory Management Rules

| Pointer | Owner | How to free |
|---------|-------|-------------|
| `RustBash*` from `rust_bash_create` | Caller | `rust_bash_free(sb)` |
| `ExecResult*` from `rust_bash_exec` | Caller | `rust_bash_result_free(result)` |
| `const char*` from `rust_bash_version` | Library (static) | **Do not free** |
| `const char*` from `rust_bash_last_error` | Library (thread-local) | **Do not free** — valid only until next FFI call |

**Key rule:** Every non-NULL pointer returned by `_create` or `_exec` must be freed exactly once with the matching `_free` function. Passing NULL to either free function is a safe no-op.

> **Important:** `ExecResult.stdout_ptr` and `ExecResult.stderr_ptr` are **not** null-terminated. Always use the corresponding `stdout_len` / `stderr_len` field to determine the byte count.

## Error Handling Pattern

All fallible functions (`rust_bash_create`, `rust_bash_exec`) return `NULL` on error. After receiving `NULL`, call `rust_bash_last_error()` to retrieve a human-readable error message:

```c
#include "rust_bash.h"
#include <stdio.h>

struct RustBash *sb = rust_bash_create("{invalid json}");
if (sb == NULL) {
    const char *err = rust_bash_last_error();
    fprintf(stderr, "Error: %s\n", err);  // err is null-terminated
    return 1;
}
```

- `rust_bash_last_error()` returns `NULL` when the last call succeeded.
- The error pointer is valid only until the next FFI call on the same thread.
- Copy the error string if you need to keep it.

## Using from Python

Complete example using the `ctypes` module. See also [`examples/ffi/python/`](../../examples/ffi/python/).

```python
import ctypes
import json
import os

# --- Load the shared library ---
lib_path = os.environ.get(
    "RUST_BASH_LIB",
    os.path.join(os.path.dirname(__file__), "../../../target/release/librust_bash.so"),
)
lib = ctypes.CDLL(lib_path)

# --- Define the ExecResult struct ---
# Use c_void_p (not c_char_p) for stdout_ptr/stderr_ptr because they are
# NOT null-terminated — c_char_p would auto-convert and read past the buffer.
class ExecResult(ctypes.Structure):
    _fields_ = [
        ("stdout_ptr", ctypes.c_void_p),
        ("stdout_len", ctypes.c_int32),
        ("stderr_ptr", ctypes.c_void_p),
        ("stderr_len", ctypes.c_int32),
        ("exit_code", ctypes.c_int32),
    ]

# --- Declare function signatures ---
lib.rust_bash_create.argtypes = [ctypes.c_char_p]
lib.rust_bash_create.restype = ctypes.c_void_p

lib.rust_bash_exec.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.rust_bash_exec.restype = ctypes.POINTER(ExecResult)

lib.rust_bash_result_free.argtypes = [ctypes.POINTER(ExecResult)]
lib.rust_bash_result_free.restype = None

lib.rust_bash_free.argtypes = [ctypes.c_void_p]
lib.rust_bash_free.restype = None

lib.rust_bash_last_error.argtypes = []
lib.rust_bash_last_error.restype = ctypes.c_char_p

lib.rust_bash_version.argtypes = []
lib.rust_bash_version.restype = ctypes.c_char_p

# --- Use the API ---
print("rust-bash version:", lib.rust_bash_version().decode())

config = json.dumps({
    "files": {"/hello.txt": "Hello from Python!"},
    "env": {"GREETING": "Hi"},
    "cwd": "/",
})

sb = lib.rust_bash_create(config.encode("utf-8"))
if not sb:
    err = lib.rust_bash_last_error()
    raise RuntimeError(f"Failed to create sandbox: {err.decode()}")

try:
    # Execute a command
    result = lib.rust_bash_exec(sb, b"cat /hello.txt")
    if not result:
        err = lib.rust_bash_last_error()
        raise RuntimeError(f"exec failed: {err.decode()}")

    # Read output — stdout/stderr are NOT null-terminated, use pointer + length
    stdout = ctypes.string_at(result.contents.stdout_ptr, result.contents.stdout_len)
    print("stdout:", stdout.decode())
    print("exit code:", result.contents.exit_code)
    lib.rust_bash_result_free(result)

    # Execute another command using the environment variable
    result = lib.rust_bash_exec(sb, b"echo $GREETING")
    if not result:
        err = lib.rust_bash_last_error()
        raise RuntimeError(f"exec failed: {err.decode()}")

    stdout = ctypes.string_at(result.contents.stdout_ptr, result.contents.stdout_len)
    print("stdout:", stdout.decode())
    lib.rust_bash_result_free(result)
finally:
    lib.rust_bash_free(sb)
```

**Key points:**
- stdout/stderr are **not** null-terminated — always use `ctypes.string_at(ptr, length)`.
- Use `try/finally` to ensure `rust_bash_free` is called even if an error occurs.
- Encode strings to UTF-8 bytes before passing to the C API.

## Using from Go

Complete example using cgo. See also [`examples/ffi/go/`](../../examples/ffi/go/).

```go
package main

/*
#cgo LDFLAGS: -L${SRCDIR}/../../../target/release -lrust_bash
#cgo CFLAGS: -I${SRCDIR}/../../../include
#include "rust_bash.h"
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"os"
	"unsafe"
)

func main() {
	// Print library version
	version := C.GoString(C.rust_bash_version())
	fmt.Println("rust-bash version:", version)

	// Build JSON config
	config := map[string]interface{}{
		"files": map[string]string{"/hello.txt": "Hello from Go!"},
		"env":   map[string]string{"GREETING": "Hi"},
		"cwd":   "/",
	}
	configJSON, _ := json.Marshal(config)

	// Create sandbox
	cConfig := C.CString(string(configJSON))
	defer C.free(unsafe.Pointer(cConfig))

	sb := C.rust_bash_create(cConfig)
	if sb == nil {
		errMsg := C.GoString(C.rust_bash_last_error())
		fmt.Fprintf(os.Stderr, "Failed to create sandbox: %s\n", errMsg)
		os.Exit(1)
	}
	defer C.rust_bash_free(sb)

	// Execute a command
	cCmd := C.CString("cat /hello.txt")
	defer C.free(unsafe.Pointer(cCmd))

	result := C.rust_bash_exec(sb, cCmd)
	if result == nil {
		errMsg := C.GoString(C.rust_bash_last_error())
		fmt.Fprintf(os.Stderr, "exec failed: %s\n", errMsg)
		os.Exit(1)
	}

	// Read output — stdout/stderr use pointer+length, NOT null-terminated
	stdout := C.GoStringN(result.stdout_ptr, C.int(result.stdout_len))
	fmt.Printf("stdout: %s", stdout)
	fmt.Printf("exit code: %d\n", result.exit_code)
	C.rust_bash_result_free(result)
}
```

**Key points:**
- `${SRCDIR}` in cgo directives resolves to the directory containing the Go source file.
- stdout/stderr are **not** null-terminated — use `C.GoStringN(ptr, len)`.
- Use `defer` for cleanup to ensure resources are freed.

## Common Pitfalls

### Forgetting to free resources

Every `rust_bash_create` must be paired with `rust_bash_free`, and every non-NULL `rust_bash_exec` result must be paired with `rust_bash_result_free`. In Python, use `try/finally`; in Go, use `defer`.

### Reading stdout/stderr as null-terminated strings

The `stdout_ptr` and `stderr_ptr` fields are **not** null-terminated. Reading them as C strings (e.g., `printf("%s", result->stdout_ptr)`) will read past the buffer. Always use the corresponding `_len` field:
- **C:** `fwrite(result->stdout_ptr, 1, result->stdout_len, stdout)`
- **Python:** `ctypes.string_at(result.contents.stdout_ptr, result.contents.stdout_len)`
- **Go:** `C.GoStringN(result.stdout_ptr, C.int(result.stdout_len))`

### Thread safety

A `RustBash*` handle is **not** thread-safe. Do not share a single handle across threads without external locking. Create separate handles for concurrent use. Error messages from `rust_bash_last_error()` are thread-local, so concurrent threads won't clobber each other's errors.

### UTF-8 encoding

All strings passed to the API (`config_json`, `command`) must be valid UTF-8. File contents in the JSON config are also UTF-8 text strings. Passing invalid UTF-8 will result in an error.

### Using `rust_bash_last_error()` after a successful call

`rust_bash_last_error()` returns `NULL` after a successful call — it is cleared on every FFI entry. Only check it immediately after a function returns `NULL`.

## Known Limitations

- **Binary files not supported via JSON config.** The `files` map in the JSON config accepts text strings only (UTF-8). Binary file content cannot be pre-seeded through the FFI.
- **No custom command callbacks.** The FFI does not currently support registering custom `VirtualCommand` implementations from the host language. This is deferred to Milestone 5.4.
- **Single-threaded per handle.** Each `RustBash*` handle must be used from one thread at a time.
