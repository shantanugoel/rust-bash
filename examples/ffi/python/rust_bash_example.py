#!/usr/bin/env python3
"""
rust-bash FFI Example — Python (ctypes)

Demonstrates how to embed rust-bash in a Python application using the C FFI.

Prerequisites:
    cargo build --features ffi --release

Run:
    cd examples/ffi/python
    LD_LIBRARY_PATH=../../../target/release python3 rust_bash_example.py
"""

import ctypes
import json
import os
import sys

# ---------------------------------------------------------------------------
# Load the shared library
# ---------------------------------------------------------------------------

# Determine library path — prefer environment override, fall back to relative path.
if sys.platform == "darwin":
    lib_name = "librust_bash.dylib"
else:
    lib_name = "librust_bash.so"

lib_path = os.environ.get(
    "RUST_BASH_LIB",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "target", "release", lib_name),
)

try:
    lib = ctypes.CDLL(lib_path)
except OSError as e:
    print(f"ERROR: Could not load {lib_path}: {e}", file=sys.stderr)
    print("Did you run: cargo build --features ffi --release ?", file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------------------
# Define the ExecResult struct (matches include/rust_bash.h)
# ---------------------------------------------------------------------------

class ExecResult(ctypes.Structure):
    """Mirrors the C ExecResult struct.

    IMPORTANT: stdout_ptr/stderr_ptr are NOT null-terminated.
    We use c_void_p (not c_char_p) to prevent ctypes from automatically
    reading the pointer as a null-terminated string. Always use
    ctypes.string_at(ptr, length) to read the correct number of bytes.
    """
    _fields_ = [
        ("stdout_ptr", ctypes.c_void_p),
        ("stdout_len", ctypes.c_int32),
        ("stderr_ptr", ctypes.c_void_p),
        ("stderr_len", ctypes.c_int32),
        ("exit_code", ctypes.c_int32),
    ]


# ---------------------------------------------------------------------------
# Declare function signatures
# ---------------------------------------------------------------------------

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


# ---------------------------------------------------------------------------
# Helper functions
# ---------------------------------------------------------------------------

def check_sandbox(sb):
    """Raise if sandbox creation failed."""
    if not sb:
        err = lib.rust_bash_last_error()
        msg = err.decode("utf-8") if err else "unknown error"
        raise RuntimeError(f"Failed to create sandbox: {msg}")


def run_command(sb, command):
    """Execute a command and return (stdout, stderr, exit_code).

    Handles memory management of the ExecResult automatically.
    """
    result = lib.rust_bash_exec(sb, command.encode("utf-8"))
    if not result:
        err = lib.rust_bash_last_error()
        msg = err.decode("utf-8") if err else "unknown error"
        raise RuntimeError(f"exec failed: {msg}")

    try:
        # stdout/stderr are NOT null-terminated — read exactly _len bytes.
        stdout = ctypes.string_at(result.contents.stdout_ptr, result.contents.stdout_len)
        stderr = ctypes.string_at(result.contents.stderr_ptr, result.contents.stderr_len)
        exit_code = result.contents.exit_code
        return stdout.decode("utf-8"), stderr.decode("utf-8"), exit_code
    finally:
        lib.rust_bash_result_free(result)


# ---------------------------------------------------------------------------
# Main demonstration
# ---------------------------------------------------------------------------

def main():
    # 1. Print library version
    version = lib.rust_bash_version().decode("utf-8")
    print(f"rust-bash version: {version}")
    print()

    # 2. Create a sandbox with pre-seeded files and environment variables
    config = json.dumps({
        "files": {
            "/hello.txt": "Hello from Python!",
            "/data.csv": "name,age\nAlice,30\nBob,25\n",
        },
        "env": {
            "GREETING": "Hi there",
            "APP_MODE": "demo",
        },
        "cwd": "/",
    })

    sb = lib.rust_bash_create(config.encode("utf-8"))
    check_sandbox(sb)

    try:
        # 3. Read a pre-seeded file
        print("--- cat /hello.txt ---")
        stdout, stderr, code = run_command(sb, "cat /hello.txt")
        print(f"stdout: {stdout!r}")
        print(f"exit code: {code}")
        print()

        # 4. Use environment variables
        print("--- echo $GREETING ---")
        stdout, stderr, code = run_command(sb, "echo $GREETING")
        print(f"stdout: {stdout!r}")
        print()

        # 5. Multi-command pipeline
        print("--- text processing pipeline ---")
        stdout, stderr, code = run_command(sb, "cat /data.csv | grep -v name | sort")
        print(f"stdout: {stdout!r}")
        print()

        # 6. State persists across exec calls
        print("--- state persistence ---")
        run_command(sb, "MY_VAR=42")
        stdout, stderr, code = run_command(sb, "echo $MY_VAR")
        print(f"MY_VAR = {stdout.strip()!r}")
        print()

        # 7. Capture stderr
        print("--- stderr capture ---")
        stdout, stderr, code = run_command(sb, "echo 'this is stderr' >&2")
        print(f"stderr: {stderr!r}")
        print()

        # 8. Non-zero exit code
        print("--- non-zero exit ---")
        stdout, stderr, code = run_command(sb, "exit 42")
        print(f"exit code: {code}")
        print()

        # 9. Error handling — invalid JSON config
        print("--- error handling ---")
        bad_sb = lib.rust_bash_create(b"not valid json")
        if not bad_sb:
            err = lib.rust_bash_last_error()
            print(f"Expected error: {err.decode('utf-8')}")
        print()

        print("All examples completed successfully!")

    finally:
        # Always free the sandbox handle
        lib.rust_bash_free(sb)


if __name__ == "__main__":
    main()
