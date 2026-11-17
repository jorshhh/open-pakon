// The page is built into web/static, which the Rust service serves
// (pakon-web paths.rs) and the packaging scripts copy into each app.
// `npm run dev` serves it with hot reload and sends /api to a running
// pakon_web (default port 8000; PAKON_API to point elsewhere).
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  base: './',
  build: {
    outDir: 'static',
    emptyOutDir: true,
    // the app window is WebKit on macOS 13+ (Safari 16)
    target: ['safari16', 'chrome110', 'firefox110'],
  },
  server: {
    proxy: { '/api': { target: process.env.PAKON_API || 'http://127.0.0.1:8000', changeOrigin: true } },
  },
});
