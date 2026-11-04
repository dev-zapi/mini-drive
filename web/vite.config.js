import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const backend = 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [svelte()],
  build: {
    // 构建产物直接落到后端托管的 public/，clone 后无需构建即可运行
    outDir: '../public',
    emptyOutDir: true,
    target: 'es2020',
    assetsDir: 'assets',
  },
  server: {
    port: 5173,
    proxy: {
      '/api': backend,
      '/files': backend,
      '/recycle': backend,
      '/llms.txt': backend,
      '/help': backend,
    },
  },
});
