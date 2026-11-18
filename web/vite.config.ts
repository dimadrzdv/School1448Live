import { readFileSync } from 'node:fs'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The Rust server runs on :8080 in development; Vite proxies API and media to it.
const backend = process.env.BACKEND ?? 'http://127.0.0.1:8080'
const proxy = {
  '/api': { target: backend, changeOrigin: false },
  '/media': { target: backend, changeOrigin: false },
}
// HTTPS for testing from a phone on the local network (scripts/phone-test.sh).
const https =
  process.env.HTTPS_KEY && process.env.HTTPS_CERT
    ? { key: readFileSync(process.env.HTTPS_KEY), cert: readFileSync(process.env.HTTPS_CERT) }
    : undefined

export default defineConfig({
  plugins: [react()],
  server: {
    host: true,
    port: 5173,
    https,
    proxy,
  },
  preview: { host: true, https, proxy },
  build: {
    target: 'es2022',
    cssCodeSplit: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules/hls.js')) return 'hls'
          if (id.includes('node_modules/react')) return 'react'
        },
      },
    },
  },
})
