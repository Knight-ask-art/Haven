import path from 'path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { live2dCspPlugin } from './scripts/live2d-csp-plugin.ts'

// https://vite.dev/config/
export default defineConfig({
  plugins: [live2dCspPlugin(), react()],
  optimizeDeps: {
    // 保持原始 ESM 进入 haven-live2d-csp transform，避免开发期预构建绕过补丁。
    exclude: ['oh-my-live2d'],
  },
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
