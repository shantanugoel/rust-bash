import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const websiteRoot = fileURLToPath(new URL('./', import.meta.url));
const websitePublicDir = resolve(websiteRoot, 'public');
const MAX_VFS_FILE_BYTES = 200 * 1024;
const TRACKED_VFS_PATHS = ['README.md', 'Cargo.toml', 'src', 'docs'];
const INLINE_VFS_PATHS = new Set(['README.md', 'Cargo.toml']);

function loadTrackedRepoVfsFiles(): {
  placeholders: Record<string, string>;
  deferredFileContentsUrl: string;
} {
  const trackedFiles = execFileSync('git', ['ls-files', '--', ...TRACKED_VFS_PATHS], {
    cwd: repoRoot,
    encoding: 'utf8',
  })
    .split('\n')
    .filter(Boolean);

  const placeholders: Record<string, string> = {};
  const deferredFileContents: Record<string, string> = {};

  for (const relativePath of trackedFiles) {
    const absolutePath = resolve(repoRoot, relativePath);
    const content = readFileSync(absolutePath);

    // Skip binary files and unusually large tracked files to keep the website payload bounded.
    if (content.includes(0) || content.byteLength > MAX_VFS_FILE_BYTES) {
      continue;
    }

    const vfsPath = `/home/user/${relativePath}`;
    const text = content.toString('utf8');
    placeholders[vfsPath] = INLINE_VFS_PATHS.has(relativePath) ? text : '';

    if (!INLINE_VFS_PATHS.has(relativePath)) {
      deferredFileContents[vfsPath] = text;
    }
  }

  mkdirSync(websitePublicDir, { recursive: true });

  // Clean up stale hashed files from previous builds
  for (const file of readdirSync(websitePublicDir)) {
    if (/^repo-vfs-files\.[0-9a-f]+\.json$/.test(file)) {
      unlinkSync(resolve(websitePublicDir, file));
    }
  }

  const jsonContent = JSON.stringify(deferredFileContents);
  const hash = createHash('md5').update(jsonContent).digest('hex').slice(0, 8);
  const hashedFilename = `repo-vfs-files.${hash}.json`;
  writeFileSync(
    resolve(websitePublicDir, hashedFilename),
    jsonContent,
  );

  return {
    placeholders,
    deferredFileContentsUrl: `/${hashedFilename}`,
  };
}

const trackedRepoVfsFiles = loadTrackedRepoVfsFiles();

export default defineConfig({
  root: 'src',
  publicDir: '../public',
  plugins: [
    tailwindcss(),
    // Rewrite the prefetch link in index.html to use the content-hashed filename
    {
      name: 'vfs-prefetch-hash',
      transformIndexHtml(html) {
        return html.replace(
          /href="\/repo-vfs-files[^"]*\.json"/,
          `href="${trackedRepoVfsFiles.deferredFileContentsUrl}"`,
        );
      },
    },
  ],
  define: {
    __RUST_BASH_VFS_PLACEHOLDERS__: JSON.stringify(trackedRepoVfsFiles.placeholders),
    __RUST_BASH_VFS_FILES_URL__: JSON.stringify(trackedRepoVfsFiles.deferredFileContentsUrl),
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
