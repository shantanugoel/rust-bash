// rust-bash FFI Example — Go (cgo)
//
// Demonstrates how to embed rust-bash in a Go application using the C FFI.
//
// Prerequisites:
//
//	cargo build --features ffi --release
//
// Run:
//
//	cd examples/ffi/go
//	CGO_ENABLED=1 go run main.go
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

// lastError retrieves the last FFI error message, or "unknown error" if none.
func lastError() string {
	errPtr := C.rust_bash_last_error()
	if errPtr == nil {
		return "unknown error"
	}
	return C.GoString(errPtr)
}

// execCommand runs a shell command in the sandbox and returns (stdout, stderr, exitCode, error).
// It handles freeing the ExecResult automatically.
func execCommand(sb *C.struct_RustBash, command string) (string, string, int, error) {
	cCmd := C.CString(command)
	defer C.free(unsafe.Pointer(cCmd))

	result := C.rust_bash_exec(sb, cCmd)
	if result == nil {
		return "", "", -1, fmt.Errorf("exec failed: %s", lastError())
	}
	defer C.rust_bash_result_free(result)

	// stdout/stderr use pointer+length — they are NOT null-terminated.
	stdout := C.GoStringN(result.stdout_ptr, C.int(result.stdout_len))
	stderr := C.GoStringN(result.stderr_ptr, C.int(result.stderr_len))
	exitCode := int(result.exit_code)

	return stdout, stderr, exitCode, nil
}

func main() {
	// 1. Print library version
	version := C.GoString(C.rust_bash_version())
	fmt.Printf("rust-bash version: %s\n\n", version)

	// 2. Build JSON config with pre-seeded files and environment variables
	config := map[string]interface{}{
		"files": map[string]string{
			"/hello.txt": "Hello from Go!",
			"/data.csv":  "name,age\nAlice,30\nBob,25\n",
		},
		"env": map[string]string{
			"GREETING": "Hi there",
			"APP_MODE": "demo",
		},
		"cwd": "/",
	}
	configJSON, err := json.Marshal(config)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to marshal config: %v\n", err)
		os.Exit(1)
	}

	// 3. Create sandbox
	cConfig := C.CString(string(configJSON))
	defer C.free(unsafe.Pointer(cConfig))

	sb := C.rust_bash_create(cConfig)
	if sb == nil {
		fmt.Fprintf(os.Stderr, "Failed to create sandbox: %s\n", lastError())
		os.Exit(1)
	}
	defer C.rust_bash_free(sb)

	// 4. Read a pre-seeded file
	fmt.Println("--- cat /hello.txt ---")
	stdout, _, exitCode, err := execCommand(sb, "cat /hello.txt")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("stdout: %q\n", stdout)
	fmt.Printf("exit code: %d\n\n", exitCode)

	// 5. Use environment variables
	fmt.Println("--- echo $GREETING ---")
	stdout, _, _, err = execCommand(sb, "echo $GREETING")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("stdout: %q\n\n", stdout)

	// 6. Multi-command pipeline
	fmt.Println("--- text processing pipeline ---")
	stdout, _, _, err = execCommand(sb, "cat /data.csv | grep -v name | sort")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("stdout: %q\n\n", stdout)

	// 7. State persists across exec calls
	fmt.Println("--- state persistence ---")
	_, _, _, err = execCommand(sb, "MY_VAR=42")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	stdout, _, _, err = execCommand(sb, "echo $MY_VAR")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("MY_VAR = %q\n\n", stdout)

	// 8. Capture stderr
	fmt.Println("--- stderr capture ---")
	_, stderr, _, err := execCommand(sb, "echo 'this is stderr' >&2")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("stderr: %q\n\n", stderr)

	// 9. Non-zero exit code
	fmt.Println("--- non-zero exit ---")
	_, _, exitCode, err = execCommand(sb, "exit 42")
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
	fmt.Printf("exit code: %d\n\n", exitCode)

	// 10. Error handling — invalid JSON config
	fmt.Println("--- error handling ---")
	badConfig := C.CString("not valid json")
	defer C.free(unsafe.Pointer(badConfig))

	badSb := C.rust_bash_create(badConfig)
	if badSb == nil {
		fmt.Printf("Expected error: %s\n\n", lastError())
	} else {
		C.rust_bash_free(badSb)
	}

	fmt.Println("All examples completed successfully!")
}
