// ---------------------------------------------------------------------------
// 迷你模式 + 窗口几何：整窗缩成一枚贴边置顶的"组件"（缩小至组件），
// 以及窗口状态记忆（几何防抖落盘）。
//
// 从 App.vue 抽出的纯逻辑（P1-⑤ 第一步，不改行为）。
//
// 窗口几何全走运行时 API —— tauri.conf 里不能静态写死 minWidth（会把
// 264px 的小窗挡在门外），全尺寸的最小约束改由 setMinSize 在运行时恢复。
//
// 窗口状态记忆：几何存进 Rust 侧（应用数据目录的 window.json），**启动时的恢复
// 由 Rust 在 show 之前完成** —— 所以这里只有「上报」，没有「恢复」。
//
// 前端不参与启动期恢复是刻意的：WebView2 把页面加载完要好几秒（窗口隐藏期间
// 还会被节流到十几秒），那时窗口早该出现在用户眼前了。让前端去摆位，就会
// 先露出默认位置的窗口、再跳过去。
//
// 坐标系：尺寸用 innerSize（与 Rust 的 set_size 同为「客户区」），位置用
// outerPosition（与 set_position 同为「外框左上角」）。混用会让窗口每次
// 重启长大一圈边框 16×9。
// ---------------------------------------------------------------------------

import { onUnmounted, ref } from "vue";
import { windowApi } from "../api";
import { toast } from "./useToast";

const MINI_W = 264;
const MINI_H = 96;
/** 迷你形态标志（前端 UI 状态，决定渲染哪套模板）。窗口几何那部分由 Rust 落盘。 */
const MINI_KEY = "pristimer.mini";
/** 进入迷你前的常规客户区尺寸 —— 只在「退出迷你」时用得到，属于运行时状态。 */
const RESTORE_KEY = "pristimer.mini-restore";

type WindowModule = typeof import("@tauri-apps/api/window");
let windowModule: WindowModule | null = null;

/** 惰性加载窗口 API 模块（同时拿到 PhysicalSize 等类型构造器）。 */
async function winModule(): Promise<WindowModule> {
  if (!windowModule) windowModule = await import("@tauri-apps/api/window");
  return windowModule;
}

/** 位置 (x,y) 是否落在某台显示器的可见范围内（留 40px 容差）。
 *  拔掉显示器 / 改过分辨率后，存下的位置可能跑到屏幕外 —— 那就别恢复。 */
async function positionVisible(x: number, y: number, w: number, h: number): Promise<boolean> {
  try {
    const mod = await winModule();
    const monitors = await mod.availableMonitors();
    return monitors.some((m) => {
      const mx = m.position.x;
      const my = m.position.y;
      const mw = m.size.width;
      const mh = m.size.height;
      return x + w > mx + 40 && x < mx + mw - 40 && y + h > my + 40 && y < my + mh - 40;
    });
  } catch {
    return false;
  }
}

export function useMiniWindow() {
  const mini = ref(localStorage.getItem(MINI_KEY) === "1");

  /** 窗口几何分帧插值动画：尺寸走客户区、位置走外框，easeOutCubic 缓出。
   *  ★ 第十六轮性能重构：插值循环挪进 Rust（animate_window_to，原生 SetWindowPos
   *  分帧）—— 旧实现 JS rAF 每帧 2 次 IPC（setSize/setPosition），WebView2 桥的
   *  往返延迟把 16ms/帧预算吃光，是迷你过渡卡顿的根因；现在一次 invoke 完成
   *  整段动画。这里保留 JS 插值作兜底（旧后端/命令缺失时）。 */
  async function animateWindowTo(
    mod: WindowModule,
    w: ReturnType<WindowModule["getCurrentWindow"]>,
    toSize: { w: number; h: number },
    toPos: { x: number; y: number },
    durationMs = 320,
  ): Promise<void> {
    try {
      mm("animate-invoke-start");
      await windowApi.animateTo({ x: toPos.x, y: toPos.y, w: toSize.w, h: toSize.h, durationMs });
      mm("animate-invoke-done");
      return;
    } catch {
      /* 原生通道不可用 → 走下面的 JS 插值 */
    }
    const fromSize = await w.innerSize();
    const fromPos = await w.outerPosition();
    const t0 = performance.now();
    await new Promise<void>((resolve) => {
      const step = (): void => {
        const t = Math.min(1, (performance.now() - t0) / durationMs);
        const e = 1 - Math.pow(1 - t, 3);
        void w.setSize(
          new mod.PhysicalSize(
            Math.round(fromSize.width + (toSize.w - fromSize.width) * e),
            Math.round(fromSize.height + (toSize.h - fromSize.height) * e),
          ),
        );
        void w.setPosition(
          new mod.PhysicalPosition(
            Math.round(fromPos.x + (toPos.x - fromPos.x) * e),
            Math.round(fromPos.y + (toPos.y - fromPos.y) * e),
          ),
        );
        if (t < 1) requestAnimationFrame(step);
        else resolve();
      };
      step();
    });
  }

  /** 迷你形态的目标几何：客户区 264×96（物理），位置优先沿用记忆点、否则右下角。 */
  async function miniTarget(
    mod: WindowModule,
    w: ReturnType<WindowModule["getCurrentWindow"]>,
  ): Promise<{ size: { w: number; h: number }; pos: { x: number; y: number } | null }> {
    const scale = await w.scaleFactor();
    const pw = Math.round(MINI_W * scale);
    const ph = Math.round(MINI_H * scale);
    try {
      const savedPos = (await windowApi.get()).mini;
      if (savedPos && (await positionVisible(savedPos.x, savedPos.y, pw, ph))) {
        return { size: { w: pw, h: ph }, pos: savedPos };
      }
    } catch {
      /* 读不到就走默认位置 */
    }
    try {
      const monitor = await mod.currentMonitor();
      if (monitor) {
        const margin = Math.round(14 * scale);
        return {
          size: { w: pw, h: ph },
          pos: {
            x: monitor.position.x + monitor.size.width - pw - margin,
            y: monitor.position.y + monitor.size.height - ph - margin,
          },
        };
      }
    } catch {
      /* 连显示器都拿不到就原地收缩 */
    }
    return { size: { w: pw, h: ph }, pos: null };
  }

  async function enterMiniWindow(animate = false, prep: MiniPrep | null = null): Promise<void> {
    try {
      mm("enter-start");
      const mod = await winModule();
      const w = mod.getCurrentWindow();
      // 记住进入前的窗口大小，还原时原样恢复。
      // 守卫：如果当前已经是组件尺寸（上次关闭时就是迷你态、这次启动直接
      // 以组件醒来），不能把它存成"恢复尺寸"，否则还原只能回到默认大小。
      // ★ 尺寸/目标几何优先用淡出期间并行算好的 prep（无磁盘/显示器 IO）。
      let cur = prep?.kind === "enter" ? prep.cur : null;
      if (!cur) {
        const size = await w.innerSize();
        cur = { w: size.width, h: size.height };
      }
      if (cur.w >= 400 && cur.h >= 300) {
        localStorage.setItem(RESTORE_KEY, JSON.stringify({ w: cur.w, h: cur.h }));
      }
      // ★ 4 项窗口属性合并成一次 IPC（逐项 ~21ms × 4 白占关键路径 ~80ms）
      try {
        await windowApi.setMiniShell(true);
      } catch {
        await w.setResizable(false);
        await w.setMinSize(null);
        await w.setAlwaysOnTop(true);
        await w.setShadow(false);
      }
      mm("enter-toggles-done");
      const target =
        prep?.kind === "enter" ? prep.target : await miniTarget(mod, w);
      mm("enter-target-ready");
      if (animate && target.pos) {
        await animateWindowTo(mod, w, target.size, target.pos);
      } else {
        await w.setSize(new mod.PhysicalSize(target.size.w, target.size.h));
        if (target.pos) await w.setPosition(new mod.PhysicalPosition(target.pos.x, target.pos.y));
      }
    } catch (err) {
      console.error("进入迷你模式失败", err);
      toast.error("迷你模式切换失败", String(err));
    }
  }

  async function exitMiniWindow(animate = false, prep: MiniPrep | null = null): Promise<void> {
    try {
      mm("exit-start");
      const mod = await winModule();
      const w = mod.getCurrentWindow();
      // ★ 4 项窗口属性合并成一次 IPC（与 enter 对称）
      try {
        await windowApi.setMiniShell(false);
      } catch {
        await w.setAlwaysOnTop(false);
        await w.setShadow(true);
        await w.setMinSize(new mod.LogicalSize(760, 560));
        await w.setResizable(true);
      }
      mm("exit-toggles-done");

      // 1) 尺寸：优先回到进入迷你前的客户区尺寸（prep 已在淡出期间读好）
      let target = prep?.kind === "exit" ? prep.size : null;
      if (!target) {
        const scale = await w.scaleFactor();
        target = { w: Math.round(900 * scale), h: Math.round(640 * scale) };
      }
      // 最小尺寸要先放开，否则下面的 setSize 会被夹到 760×560
      await w.setMinSize(new mod.LogicalSize(760, 560));
      await w.setResizable(true);

      // 2) 位置：回到**进入迷你前的常规位置**（记在 Rust 侧，prep 已读好）。
      //    ★ 少了这一步，"还原"出来的窗口会停在组件待过的那个角落 —— 更糟的是
      //    紧接着触发的 onMoved 会把角落写进 normal，等于把记忆永久改坏，
      //    下次启动也在角落里。用户看到的正是"还原后跳到屏幕右下角"。
      let pos = prep?.kind === "exit" ? prep.pos : null;
      if (!pos) {
        // 没有记忆位置（第一次就进了迷你）→ 至少保证窗口完整落在屏幕内
        const cur = await w.outerPosition();
        if (!(await positionVisible(cur.x, cur.y, target.w, target.h))) {
          try {
            const monitor = await mod.currentMonitor();
            if (monitor) {
              pos = {
                x: monitor.position.x + Math.round((monitor.size.width - target.w) / 2),
                y: monitor.position.y + Math.round((monitor.size.height - target.h) / 2),
              };
            }
          } catch {
            /* 连尺寸都拿不到就算了，保持 Windows 给的位置 */
          }
        }
      }
      mm("exit-target-ready");
      if (animate) {
        const cur = await w.outerPosition();
        await animateWindowTo(mod, w, { w: target.w, h: target.h }, pos ?? { x: cur.x, y: cur.y });
      } else {
        await w.setSize(new mod.PhysicalSize(target.w, target.h));
        if (pos) await w.setPosition(new mod.PhysicalPosition(pos.x, pos.y));
      }
    } catch (err) {
      console.error("退出迷你模式失败", err);
      toast.error("还原窗口失败", String(err));
    }
  }

  /** 切换形态期间抑制「几何变动 → 自动落盘」。
   *
   *  切换过程中会连续触发 onResized / onMoved，其中间态（例如尺寸已经改小、
   *  位置还没摆好那一帧）并不是用户意图 —— 记下来就把记忆改坏了。 */
  let suppressWinSave = false;

  /** 形态切换的内容淡出：几何动画期间两套模板都不该以"被拉伸/压扁"的
   *  中间态示人 —— 先把当前内容淡出（0.18s），动画到位后再切模板进场。 */
  const morphOut = ref(false);
  /** 防连点：一次形态切换没走完不接受下一次。 */
  let morphing = false;

  /** 形态切换各阶段的耗时标记（验证/调优用：CDP 里读 window.__mm）。
   *  只在 dev/验证时有意义，生产里多几条数组写入无碍。 */
  function mm(label: string): void {
    const w = window as unknown as { __mm?: [string, number][] };
    (w.__mm ??= []).push([label, Math.round(performance.now())]);
  }

  /** 迷你形态开关：把迷你态同步到根元素类上 —— html.mini-mode 会禁掉滚动
   *  本身（滚动条已全局取消，见 glass.css），防止小窗内容意外溢出时出现橡皮筋。 */
  function syncMiniClass(): void {
    document.documentElement.classList.toggle("mini-mode", mini.value);
  }

  /** 形态切换的目标几何预备数据：在内容淡出期间并行算好，
   *  让几何动画只等纯设置类 IPC，不等磁盘/显示器查询。 */
  type MiniPrep =
    | { kind: "enter"; scale: number; target: { size: { w: number; h: number }; pos: { x: number; y: number } | null }; cur: { w: number; h: number } }
    | { kind: "exit"; scale: number; size: { w: number; h: number }; pos: { x: number; y: number } | null };

  /** 进迷你前的准备：目标尺寸/位置 + 记忆当前常规尺寸。 */
  async function prepMiniEnter(
    mod: WindowModule,
    w: ReturnType<WindowModule["getCurrentWindow"]>,
  ): Promise<MiniPrep> {
    mm("prep-enter-start");
    const scale = await w.scaleFactor();
    const cur = await w.innerSize();
    const target = await miniTarget(mod, w);
    mm("prep-enter-done");
    return {
      kind: "enter",
      scale,
      target,
      cur: { w: cur.width, h: cur.height },
    };
  }

  /** 还原窗口前的准备：目标尺寸（localStorage）+ 位置（Rust 侧 normal 记忆）。 */
  async function prepMiniExit(
    w: ReturnType<WindowModule["getCurrentWindow"]>,
  ): Promise<MiniPrep> {
    mm("prep-exit-start");
    const scale = await w.scaleFactor();
    // 尺寸：优先回到进入迷你前的客户区尺寸
    let size = { w: Math.round(900 * scale), h: Math.round(640 * scale) };
    try {
      const saved = JSON.parse(localStorage.getItem(RESTORE_KEY) ?? "null") as {
        w: number;
        h: number;
      } | null;
      if (saved && saved.w >= 400 && saved.h >= 300) size = saved;
    } catch {
      /* 存了坏数据就走默认尺寸 */
    }
    // 位置：回到进入迷你前的常规位置（记在 Rust 侧），跑出屏幕则放弃
    let pos: { x: number; y: number } | null = null;
    try {
      const n = (await windowApi.get()).normal;
      if (n && (await positionVisible(n.x, n.y, n.w, n.h))) pos = { x: n.x, y: n.y };
    } catch {
      /* 没有记忆位置就交给还原路径兜底 */
    }
    mm("prep-exit-done");
    return { kind: "exit", scale, size, pos };
  }

  async function toggleMini(): Promise<void> {
    if (morphing) return;
    const next = !mini.value;
    morphing = true;
    mm("click");
    localStorage.setItem(MINI_KEY, next ? "1" : "0");
    suppressWinSave = true;
    try {
      // ⓪ 与淡出并行的准备工作：目标几何查询（磁盘/显示器 IO）不占动画关键路径
      const mod0 = await winModule();
      const w0 = mod0.getCurrentWindow();
      const prep: Promise<MiniPrep | null> =
        next ? prepMiniEnter(mod0, w0) : prepMiniExit(w0);
      // ① 当前内容淡出（避免几何动画中模板被压扁的变形感）
      morphOut.value = true;
      await new Promise((r) => setTimeout(r, 190));
      mm("fade-done");
      // ② 窗口几何分帧插值到目标形态（~320ms）
      const ready = await prep;
      if (next) await enterMiniWindow(true, ready);
      else await exitMiniWindow(true, ready);
      mm("geom-done");
      // ③ 切模板：迷你进场走 mini-in 缩放弹出，还原走常规内容 rise
      mini.value = next;
      syncMiniClass();
      morphOut.value = false;
      // 先让 Rust 知道形态（下次启动才能在 show 之前摆对几何 + 换圆角半径），
      // 再把这**一次切换的结果**主动写一次 —— 不等防抖，中间态一律不写。
      await windowApi.save({ miniMode: next });
      await persistWinState();
    } catch {
      /* 非 Tauri 环境：失败也要保证 UI 形态正确 */
      mini.value = next;
      syncMiniClass();
      morphOut.value = false;
    } finally {
      morphing = false;
      // 放开自动落盘。等一拍再放：切换期间的事件是异步投递的，
      // 立刻放开会把最后几个中间事件又收进来。
      window.setTimeout(() => {
        suppressWinSave = false;
      }, 360);
    }
  }

  /** 迷你组件上的「收进托盘」：窗口藏起来，计时继续（与标题栏关闭同一条链路）。 */
  async function closeToTray(): Promise<void> {
    try {
      const mod = await winModule();
      await mod.getCurrentWindow().close();
    } catch (err) {
      console.error("收进托盘失败", err);
    }
  }

  // -------------------------------------------------------------------------
  // 窗口状态记忆：几何防抖落盘
  // -------------------------------------------------------------------------

  async function persistWinState(): Promise<void> {
    try {
      const mod = await winModule();
      const w = mod.getCurrentWindow();
      // 最小化时必须直接跳过：此刻 innerSize / outerPosition 返回的是 Windows 的
      // 哨兵值（位置 -32000,-32000、尺寸小到 138×15），存进去就把"记忆的几何"毁了 ——
      // 下次启动还会被屏幕可见性校验判为屏幕外而丢弃，等于把记忆清空。
      if (await w.isMinimized()) return;
      if (mini.value) {
        // 迷你态：尺寸固定，只有位置是用户调过的
        const pos = await w.outerPosition();
        await windowApi.save({ mini: { x: pos.x, y: pos.y } });
        return;
      }
      // 不能只信 isMaximized()：实测它在某些时序下会返回 false（窗口明明已经最大化），
      // 那时就会把"铺满屏幕"的矩形当成常规几何存下来，下次启动直接开成满屏。
      // 所以补一个几何判据：外框几乎盖满当前显示器也算最大化。
      let maximized = await w.isMaximized();
      if (!maximized) {
        const monitor = await mod.currentMonitor().catch(() => null);
        if (monitor) {
          const outer = await w.outerSize();
          maximized =
            outer.width >= monitor.size.width - 8 && outer.height >= monitor.size.height - 8;
        }
      }
      if (maximized) {
        // 最大化时不记几何，保留上一次的常规几何，只标 max
        await windowApi.save({ maximized: true });
        return;
      }
      const size = await w.innerSize();
      // 再补一道：尺寸小到不像常规窗口时坚决不写 normal。
      // Rust 侧加载时也会丢弃这种值，但那时 normal 已经被污染 —— 记忆的位置
      // 就再也回不来了（"还原后跑到右下角"的另一条成因）。
      if (size.width < 400 || size.height < 300) return;
      const pos = await w.outerPosition();
      await windowApi.save({
        normal: { w: size.width, h: size.height, x: pos.x, y: pos.y },
        maximized: false,
      });
    } catch {
      /* 窗口 API 不可用（非 Tauri 环境）就算了 */
    }
  }

  let saveWinTimer: number | null = null;
  let unlistenWinMove: (() => void) | null = null;
  let unlistenWinResize: (() => void) | null = null;

  function scheduleWinSave(): void {
    // 形态切换中：中间态不是用户意图，直接丢掉
    if (suppressWinSave) return;
    if (saveWinTimer !== null) clearTimeout(saveWinTimer);
    saveWinTimer = window.setTimeout(() => {
      saveWinTimer = null;
      void persistWinState();
    }, 500);
  }

  /** 订阅窗口几何变动 → 防抖落盘（App onMounted 时调用一次）。 */
  async function watchWindowPersistence(): Promise<void> {
    try {
      const mod = await winModule();
      const w = mod.getCurrentWindow();
      unlistenWinMove = await w.onMoved(() => scheduleWinSave());
      unlistenWinResize = await w.onResized(() => scheduleWinSave());
    } catch {
      /* 非 Tauri 环境：忽略 */
    }
  }

  /** 常规形态的最小尺寸在运行时补（配置里不静态写死，给迷你小窗让路）。 */
  async function ensureNormalMinSize(): Promise<void> {
    try {
      const mod = await winModule();
      await mod.getCurrentWindow().setMinSize(new mod.LogicalSize(760, 560));
    } catch {
      /* 非 Tauri 环境：忽略 */
    }
  }

  onUnmounted(() => {
    unlistenWinMove?.();
    unlistenWinResize?.();
    if (saveWinTimer !== null) clearTimeout(saveWinTimer);
  });

  return {
    mini,
    morphOut,
    toggleMini,
    syncMiniClass,
    closeToTray,
    enterMiniWindow,
    ensureNormalMinSize,
    watchWindowPersistence,
  };
}
