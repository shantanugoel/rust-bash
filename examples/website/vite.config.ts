import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const MAX_VFS_FILE_BYTES = 200 * 1024;
const TRACKED_VFS_PATHS = ['README.md', 'Cargo.toml', 'src', 'docs'];

function loadTrackedRepoVfsFiles(): Record<string, string> {
  const trackedFiles = execFileSync('git', ['ls-files', '--', ...TRACKED_VFS_PATHS], {
    cwd: repoRoot,
    encoding: 'utf8',
  })
    .split('\n')
    .filter(Boolean);

  const files: Record<string, string> = {};

  for (const relativePath of trackedFiles) {
    const absolutePath = resolve(repoRoot, relativePath);
    const content = readFileSync(absolutePath);

    // Skip binary files and unusually large tracked files to keep the website payload bounded.
    if (content.includes(0) || content.byteLength > MAX_VFS_FILE_BYTES) {
      continue;
    }

    files[`/home/user/${relativePath}`] = content.toString('utf8');
  }

  return files;
}

const trackedRepoVfsFiles = loadTrackedRepoVfsFiles();

export default defineConfig({
  root: 'src',
  publicDir: '../public',
  plugins: [tailwindcss()],
  define: {
    __RUST_BASH_VFS_FILES__: JSON.stringify(trackedRepoVfsFiles),
  },
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    target: 'es2022',
  },
  server: {
    fs: {
      // Allow serving files from pkg/ (WASM JS glue code)
      allow: ['..', '../../../pkg'],
    },
    proxy: {
      '/api': {
        target: 'http://localhost:8788',
        changeOrigin: true,
      },
    },
  },
});
