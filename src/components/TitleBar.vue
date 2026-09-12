<script setup lang="ts">
// ---------------------------------------------------------------------------
// 自建标题栏。
//
// 窗口关掉了系统装饰（decorations: false），所以拖拽、最小化、最大化、
// 关闭这四件事必须自己实现。拖拽不用写 JS —— `data-tauri-drag-region`
// 交给 Tauri 处理，连"双击标题栏最大化"都是它内建的。
// ---------------------------------------------------------------------------

import { onMounted, onUnmounted, ref } from "vue";

const emit = defineEmits<{ close: []; mini: [] }>();

const maximized = ref(false);
let unlisten: (() => void) | null = null;

/** 惰性取窗口句柄：非 Tauri 环境（如纯浏览器预览）下拿不到，不该让整页崩掉。 */
async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** 窗口是否处于「铺满屏幕」的状态。
 *
 *  不能只信 `isMaximized()` —— 实测在「启动时由 Rust 侧 maximize() 摆好」的窗口上
 *  它会返回 false。用它做分支判断，图标会显示错，"还原"也会退化成再最大化。 */
async function looksMaximized(mod: typeof import("@tauri-apps/api/window"), w: Awaited<ReturnType<typeof currentWindow>>): Promise<boolean> {
  if (await w.isMaximized()) return true;
  try {
    const [monitor, outer] = await Promise.all([mod.currentMonitor(), w.outerSize()]);
    return (
      !!monitor &&
      outer.width >= monitor.size.width - 8 &&
      outer.height >= monitor.size.height - 8
    );
  } catch {
    return false;
  }
}

async function syncMaximized(): Promise<void> {
  try {
    const mod = await import("@tauri-apps/api/window");
    maximized.value = await looksMaximized(mod, mod.getCurrentWindow());
  } catch {
    /* 非 Tauri 环境：忽略 */
  }
}

async function minimize(): Promise<void> {
  try {
    await (await currentWindow()).minimize();
  } catch {
    /* 忽略 */
  }
}

/** 最大化 / 还原。
 *
 *  刻意不用 `toggleMaximize()`：它内部按 `isMaximized()` 二选一，而那个判断在
 *  上面说的场景下会返回 false —— 于是"还原"被当成"最大化"，用户点了没反应。
 *  这里自己判：铺满屏幕就还原，否则最大化。 */
async function toggleMaximize(): Promise<void> {
  try {
    const mod = await import("@tauri-apps/api/window");
    const w = mod.getCurrentWindow();
    if (await looksMaximized(mod, w)) {
      await w.unmaximize();
      // 兜底：万一 unmaximize 没落地（观察到的还原失败就是这一类），
      // 退回到"按记忆的常规几何重设一次"这条路。
      if (await looksMaximized(mod, w)) {
        await w.toggleMaximize();
        await new Promise((r) => setTimeout(r, 240));
        if (await looksMaximized(mod, w)) await w.unmaximize();
      }
    } else {
      await w.maximize();
    }
  } catch {
    /* 忽略 */
  }
}

async function close(): Promise<void> {
  // 关闭意图要先告诉上层（它负责提示"已收进托盘"），再真正关窗 ——
  // Rust 侧拦下 CloseRequested 并 hide，窗口不退出。
  emit("close");
  try {
    await (await currentWindow()).close();
  } catch {
    /* 忽略 */
  }
}

onMounted(async () => {
  await syncMaximized();
  try {
    // 拖拽最大化 / 双击还原都会触发 resize，靠它同步按钮图标
    unlisten = await (await currentWindow()).onResized(() => {
      void syncMaximized();
    });
  } catch {
    /* 非 Tauri 环境：忽略 */
  }
});

onUnmounted(() => unlisten?.());
</script>

<template>
  <header class="bar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <span class="mark" />
      <span class="name">PrisTimer</span>
    </div>

    <div class="winctl">
      <!-- 缩小至组件：整窗收成 264×96 的置顶小组件（逻辑在 App.vue） -->
      <button class="wbtn" title="缩小至组件" @click="emit('mini')">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true" fill="none">
          <path
            d="M1 3.4V1h2.4M9 3.4V1H6.6M1 6.6V9h2.4M9 6.6V9H6.6"
            stroke="currentColor"
            stroke-width="1.1"
            stroke-linecap="round"
          />
        </svg>
      </button>
      <button class="wbtn" title="最小化" @click="minimize">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <path d="M1 5h8" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" />
        </svg>
      </button>
      <button
        class="wbtn"
        :title="maximized ? '还原' : '最大化'"
        @click="toggleMaximize"
      >
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true" fill="none">
          <rect
            v-if="!maximized"
            x="1.2"
            y="1.2"
            width="7.6"
            height="7.6"
            rx="1.2"
            stroke="currentColor"
            stroke-width="1.1"
          />
          <template v-else>
            <rect x="1.2" y="3" width="5.8" height="5.8" rx="1.1" stroke="currentColor" stroke-width="1.1" />
            <path d="M3.6 3V1.4h5.2v5.2H7.2" stroke="currentColor" stroke-width="1.1" fill="none" />
          </template>
        </svg>
      </button>
      <button class="wbtn danger" title="关闭（收进托盘继续计时）" @click="close">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <path
            d="M1.6 1.6l6.8 6.8M8.4 1.6l-6.8 6.8"
            stroke="currentColor"
            stroke-width="1.1"
            stroke-linecap="round"
          />
        </svg>
      </button>
    </div>
  </header>
</template>

<style scoped>
.bar {
  position: sticky;
  top: 0;
  z-index: var(--z-chrome);
  display: flex;
  align-items: center;
  height: 38px;
  padding-left: 0.85rem;
  /* 玻璃条：只描下边，避免四条边把窗口切成方块（墨色派生，浅色主题下同样成立） */
  background: color-mix(in srgb, var(--ink) 3.5%, transparent);
  border-bottom: 1px solid var(--glass-border);
  backdrop-filter: var(--glass-blur);
}

.brand {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  pointer-events: none; /* 让整条都能拖 */
}
.mark {
  width: 12px;
  height: 12px;
  border-radius: 4px;
  background: linear-gradient(140deg, #3ecf8e, #5aa7ff);
  box-shadow: 0 0 10px rgb(62 207 142 / 0.45);
}
.name {
  font-size: 0.74rem;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--ink-faint);
}

.winctl {
  margin-left: auto;
  display: flex;
  height: 100%;
  /* 按钮区必须退出拖拽区域，否则点击被拖拽吞掉 */
}
.wbtn {
  width: 44px;
  height: 100%;
  display: grid;
  place-items: center;
  border: none;
  background: transparent;
  color: var(--ink-dim);
  cursor: pointer;
  transition:
    background var(--t-fast) ease,
    color var(--t-fast) ease;
}
.wbtn:hover {
  background: color-mix(in srgb, var(--ink) 8%, transparent);
  color: var(--ink);
}
.wbtn:active {
  background: color-mix(in srgb, var(--ink) 4%, transparent);
}
.wbtn.danger:hover {
  background: #c0392b;
  color: #fff;
}
</style>
