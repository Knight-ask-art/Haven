import path from 'path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // 与 src-tauri/tauri.conf.json 的 devUrl(http://localhost:1420) 对齐；
  // strictPort 端口被占时直接失败，避免静默漂移到其它端口导致 WebView 白屏。
  server: {
    port: 1420,
    strictPort: true,
  },
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
})
