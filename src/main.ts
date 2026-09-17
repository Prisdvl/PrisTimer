import { createApp } from "vue";
import "./styles/glass.css";
import App from "./App.vue";
import OverlayPanel from "./components/OverlayPanel.vue";
import { syncTheme } from "./composables/useTheme";

// ---------------------------------------------------------------------------
// 启动占位清理。
//
// index.html 里放了一个纯 CSS 的加载指示（"正在启动 PrisTimer…"），
// 它保证窗口在 JS 执行前就有内容，且**绝不出现浏览器式白屏/错误页**。
// Vue 一旦挂载成功就把它摘掉。
//
// ★ 为什么要有这一层：
//   在这个应用里，"看起来像浏览器"是明确要避免的体验。WebView2 在
//   资源加载失败时会显示它**内置的 Edge 错误页**（"无法访问此页面"、
//   一大块灰白 + 一个重试按钮）—— 那是最刺眼的内核暴露。
//   有了自带占位，失败时用户看到的是我们的深色界面 + 一句中文提示。
// ---------------------------------------------------------------------------
function clearBoot(): void {
  const boot = document.getElementById("boot");
  if (boot) {
    boot.remove();
  }
}

// 兜底：如果 6 秒后 Vue 还没挂载（bundle 加载失败 / 执行出错），
// 把占位文案换成可行动的说明，而不是让用户对着转圈干等。
const bootTimer = window.setTimeout(() => {
  const text = document.getElementById("boot-text");
  if (text) {
    text.textContent = "前端资源加载超时，请重试或重新安装";
  }
}, 6000);

// ---------------------------------------------------------------------------
// 双窗口挂载：主窗口（index.html） / 悬浮信息窗（index.html#overlay）
//
// Tauri 的每个 webview 窗口都加载同一个 frontendDist 入口。
// overlay 窗口在 tauri.conf.json 里被配置成 `url: "index.html#overlay"`，
// 所以这里靠 location.hash 区分当前跑在哪个窗口里 ——
// 同一份 bundle，两套界面。
// ---------------------------------------------------------------------------
const isOverlay = window.location.hash === "#overlay";

if (isOverlay) {
  // 悬浮信息窗：只渲染信息卡。它自己订阅 insight:update 事件。
  // 窗口是 `transparent: true` 的 —— 必须让页面背景透明，
  // 否则 <html> 上那层渐变底色会把整窗糊成不透明。
  document.documentElement.classList.add("overlay-mode");

  // ★ 主题必须在这里**主动**应用（2026-09-17 修）。
  //   主题的 data-theme 属性平时由 App.vue 里的 useTheme() 写入，
  //   而浮窗走的是 `createApp(OverlayPanel)` —— **根本不经 App.vue**，
  //   于是 data-theme 永远缺失，所有主题变量都落回 :root 的默认值（深空）。
  //   症状：主窗口切到晨雾/纸墨等浅色主题后，浮窗仍是深色玻璃配浅色文字，
  //   两个界面的玻璃底色、文字色、进度条槽色全对不上（实测
  //   --glass-bg-deep 取到 rgb(10 13 18 / .44) 而非浅色主题的 rgb(28 40 60 / .1)）。
  //   syncTheme() 是纯读单例 + 写属性，重复调用安全。
  syncTheme();

  createApp(OverlayPanel).mount("#app");
  clearBoot();
} else {
  createApp(App).mount("#app");
  clearBoot();
}

window.clearTimeout(bootTimer);
