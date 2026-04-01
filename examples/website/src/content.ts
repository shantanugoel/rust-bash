/**
 * Preloaded file content for the website sandbox.
 *
 * Injected at build time from git-tracked files in the repo root allowlist:
 * README.md, Cargo.toml, src/, and docs/.
 */

declare const __RUST_BASH_VFS_FILES__: Record<string, string>;

export const VFS_FILES: Record<string, string> = __RUST_BASH_VFS_FILES__;
