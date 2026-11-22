import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// During development, API calls are proxied to a running linkmeterd
// (e.g. `cargo run -p linkmeterd -- --simulate -l 127.0.0.1:8099`).
const target = process.env.LINKMETERD ?? 'http://127.0.0.1:8099'

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': { target, changeOrigin: true },
      '/luci': { target, changeOrigin: true },
      '/user.css': { target, changeOrigin: true },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2020',
    chunkSizeWarningLimit: 300,
  },
})
