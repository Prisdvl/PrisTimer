import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/

/**
 * 去掉 Vite 注入的 `crossorigin` 属性。
 *
 * ★ 为什么必须去掉（本项目实测踩中）：
 *
 *   Vite 给 build 产物注入 `<script type="module" crossorigin>` 与
 *   `<link crossorigin>`。`crossorigin` 让浏览器用 **CORS 模式**发起请求，
 *   并要求响应带 `Access-Control-Allow-Origin`。而 Tauri 提供前端资源的
 *   是自定义协议（打包后 origin 为 `http://tauri.localhost/`），
 *   **不返回该响应头** —— 于是脚本被拒绝执行、样式被丢弃，页面一片全白。
 *
 *   这类失败特别难查：Network 面板里看得到文件、状态码 200，
 *   只有 Console 里一行不起眼的 CORS 提示，而 GUI 应用根本没有 Console。
 */
function stripCrossorigin() {
  return {
    name: "strip-crossorigin",
    enforce: "post" as const,
    transformIndexHtml(html: string) {
      return html
        .replace(/\s+crossorigin(?:="[^"]*")?/g, "")
        // 顺带去掉 modulepreload 的 polyfill 注入（它同样带 crossorigin）
        .replace(/<link rel="modulepreload"[^>]*>/g, "");
    },
  };
}

export default defineConfig(() => ({
  plugins: [vue(), stripCrossorigin()],

  // ★ base: "./" —— 资源引用改**相对路径**（`./assets/x.js`）。
  //
  // 这是 Tauri 打包最常见的一处坑，本项目实测踩中过：
  // Vite 默认 `base: "/"`，index.html 里产出的是 `/assets/index-xxx.js`
  // （**绝对路径**）。dev 模式（http://localhost:1420）下没问题，因为
  // 那确实是个 HTTP 根；但打包后前端由 Tauri 的自定义协议提供，实际
  // origin 是 `http://tauri.localhost/`。WebView2 的 History 里能看到
  // 加载记录只有 `http://tauri.localhost/` 一条，**没有任何 assets 条目**
  // —— 说明脚本与样式压根没被请求到，页面于是停在纯白（我们的启动占位
  // 因为拿不到 JS 也不会被摘掉，但连内联样式也一起没生效时就是全白）。
  //
  // 改成相对路径后，资源按 index.html 自身位置解析，dev 与打包两种
  // 形态都正确。
  base: "./",

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
    // ★ 关掉 modulepreload 的 crossorigin 属性（Vite 5+ 对 build 产物默认加）。
    //
    // `crossorigin` 会触发 CORS 模式请求，而 Tauri 的自定义协议不返回
    // `Access-Control-Allow-Origin`，结果是**脚本被浏览器拒绝执行**。
    // 表现同样是白屏。索引 HTML 里注入的 script/link 标签由 Tauri 处理，
    // 这里主要防止构建期注入的 preload 链接带上该属性。
    modulePreload: { polyfill: false },
    // emptyOutDir 会 rmSync 整个 dist/assets（文件数 >50 时被本机安全策略拦截，
    // 导致 beforeBuildCommand 假失败）。关掉后旧 hash 产物会残留，但桌面 MSI /
    // 安卓 APK 打包时只嵌入本次构建的 index.html 引用链，残留文件无害。
    emptyOutDir: false,
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
      // 3. tell Vite to ignore watching `src-tauri` and the Rust build dir.
      //    target/ 下的 .dll 正被 rustc 写入时会被文件锁占用，
      //    Vite 强行 watch 它会在 Windows 上报 EBUSY 导致 vite 崩溃退出。
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
}));
