import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  root: 'src',
  publicDir: '../public',
  plugins: [tailwindcss()],
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
