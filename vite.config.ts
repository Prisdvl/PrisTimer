import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [vue()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  build: {
    // 运行环境只有 WebView2（Chromium 内核），把目标钉死在现代 Chrome 上：
    // 默认目标包含 Safari，压缩器会把 `backdrop-filter` 改写成 `-webkit-` 前缀版，
    // 而新版 Chromium 已经**不再支持**该前缀 —— 结果是磨砂玻璃整片失效。
    // 明确目标后压缩器只产出标准属性。
    target: "chrome120",
    cssTarget: "chrome120",
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
