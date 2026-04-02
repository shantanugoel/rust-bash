/**
 * Tracked repo snapshot metadata for the website sandbox.
 *
 * `VFS_PLACEHOLDER_FILES` contains the tracked file tree with inline content for
 * the tiny root files and empty placeholders for the larger source/docs files.
 * `VFS_FILES_URL` points at the deferred JSON payload with the real tracked
 * file contents for src/ and docs/.
 */

declare const __RUST_BASH_VFS_PLACEHOLDERS__: Record<string, string>;
declare const __RUST_BASH_VFS_FILES_URL__: string;

export const VFS_PLACEHOLDER_FILES: Record<string, string> =
  __RUST_BASH_VFS_PLACEHOLDERS__;
export const VFS_FILES_URL: string = __RUST_BASH_VFS_FILES_URL__;
