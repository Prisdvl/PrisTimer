<script setup lang="ts">
// ---------------------------------------------------------------------------
// 系统信息状态栏：计时器界面的底部信息展示区。
//
// 需求落点：
//   · 蓝牙图标 + 设备名 / 类型 / 剩余电量；多设备罗列展示；
//   · API 额度图标 + opencode-go 订阅剩余额度；
//   · 蓝牙未开启 / 无已连接设备 / 接口请求失败 → 友好提示而不是崩溃；
//   · 图标用常见 web 图标（全部内联 SVG，零字体依赖）。
//
// 数据全部来自 useInsight（模块级单例），本组件**只做渲染**。
// 文案统一走 deviceLine / quotaLine —— 与 Rust 侧托盘的文案同一套规则。
// ---------------------------------------------------------------------------

import { computed, ref } from "vue";
import { useInsight, batteryText, kindLabel, ageLabel, isBtOk } from "../composables/useInsight";
import { toast } from "../composables/useToast";
import { insightApi } from "../api";

const {
  snapshot,
  trayReady,
  lastBtThrottled,
  refreshBluetooth,
  refreshQuota,
  applyQuota,
  deviceLine,
  quotaLine,
} = useInsight();

// ---- 蓝牙分组 ----
const devices = computed(() => (isBtOk(snapshot.value.bluetooth) ? snapshot.value.bluetooth.devices : []));
const btText = computed(() => deviceLine(snapshot.value.bluetooth));
const btClass = computed(() => {
  switch (snapshot.value.bluetooth.status) {
    case "ok":
      return devices.value.length ? "ok" : "idle";
    case "poweredOff":
      return "warn";
    case "noAdapter":
      return "warn";
    default:
      return "err";
  }
});

// ---- 额度分组 ----
const quotaText = computed(() => quotaLine(snapshot.value.quota));
const quotaClass = computed(() => {
  switch (snapshot.value.quota.status) {
    case "ok":
      return "ok";
    case "notConfigured":
      return "idle";
    case "unauthorized":
      return "err";
    case "network":
    case "apiError":
      return "warn";
  }
});

/** 多窗口额度（opencode Go：滚动 / 本周 / 本月）。空数组表示单值接口。 */
const quotaWindows = computed(() =>
  snapshot.value.quota.status === "ok" ? snapshot.value.quota.windows ?? [] : [],
);

/** 窗口剩余百分比的整数显示。 */
function pct(value: number): string {
  return `${Math.round(value)}%`;
}

/** 窗口重置时刻的人话（解析不了就原样显示，绝不显示 Invalid Date）。 */
function resetText(iso: string | null): string {
  if (!iso) return "重置时间未知";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return `${date.toLocaleString("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  })} 重置`;
}

// ---- 数据新鲜度（"xx 秒前更新"）----
const btAge = computed(() => ageLabel(snapshot.value.bluetoothAtMs));
// 时间戳为 0 = 还没查过：不能拿它去减 Date.now()（会显示"29000 小时前"）。
const quotaAge = computed(() =>
  snapshot.value.quotaAtMs > 0 ? ageLabel(snapshot.value.quotaAtMs) : "尚未查询",
);

// ---- 额度配置弹层 ----
const configOpen = ref(false);
const keyInput = ref("");
const endpointInput = ref("");
const saving = ref(false);
const saveError = ref<string | null>(null);
/** 当前地址是否来自内置默认（用户没自定义）—— 用于给一句提示。 */
const endpointIsDefault = ref(true);
/** 上次打开弹层时后端回显的 Key 打码串。用于区分「用户没动 Key」和「用户填了新 Key」。 */
let echoedKeyHint = "";
/** 上次回显的地址。用于区分「用户没动地址」和「用户改了地址」。 */
let echoedEndpoint = "";

/** 打开弹层时回显当前配置（Key 打码，只让用户确认"是哪一个"）。 */
async function openConfig(): Promise<void> {
  configOpen.value = true;
  saveError.value = null;
  try {
    const view = await insightApi.quotaConfigGet();
    echoedKeyHint = view.keyHint;
    keyInput.value = view.keyHint;
    // 地址回显**实际请求地址**：用户没自定义时看到的就是内置默认端点。
    // 留空保存 = 继续用内置默认（所以这里把默认值填回去只是展示，
    // 用户清空它再保存同样有效）。
    endpointInput.value = view.endpoint;
    echoedEndpoint = view.endpoint;
    endpointIsDefault.value = view.endpointIsDefault;
  } catch {
    echoedKeyHint = "";
    echoedEndpoint = "";
    keyInput.value = "";
    endpointInput.value = "";
    endpointIsDefault.value = true;
  }
}

async function saveConfig(): Promise<void> {
  saving.value = true;
  saveError.value = null;
  try {
    // ★ 发送规则（这里踩过坑，务必保持）：
    //   · API Key：输入框内容 ≠ 上次回显的打码串 → 说明用户**真的填了**
    //     新 Key，原样发送；等于打码串 → 用户没动它，发 `null` 表示
    //     "不改动"（把打码串当 Key 存回去会覆盖真实 Key，是致命的）；
    //     输入框被清空 → 发送空串 = 清除配置。
    //   · 接口地址：不是秘密，无论是否编辑都发送（空串 = 清除）。
    const typedKey = keyInput.value;
    const apiKeyArg: string | null =
      typedKey === "" ? "" : typedKey === echoedKeyHint ? null : typedKey;

    // ★ 地址自动补协议：用户很可能填 `api.xxx.com/...` 不带 https://。
    //   后端 parse_url 会补全，这里只是给一个即时反馈（否则保存后
    //   地址栏看起来"没变化"，用户以为没保存上）。
    let endpoint = endpointInput.value.trim();
    if (endpoint && !endpoint.includes("://")) {
      endpoint = `https://${endpoint}`;
    }
    // ★ 用户没动地址、且它本来就是内置默认 → 提交空串，保持"用默认"语义。
    //   否则默认地址会被固化进配置：将来内置端点更新（比如官方改了路径），
    //   这类用户的配置就跟不上了。
    if (endpointIsDefault.value && endpoint === echoedEndpoint) {
      endpoint = "";
    }

    const status = await insightApi.quotaConfigSet({
      apiKey: apiKeyArg,
      endpoint,
    });
    // 返回值就是最新额度状态，直接展示（事件推送也会到达，双写幂等）。
    applyQuota(status);
    configOpen.value = false;
    // ★ 保存反馈：成功/失败都要有明确的动静，不然就是"没反应"。
    //
    //   两件事要分开说：**配置是否存下** 和 **查询是否成功**。
    //   （旧版把两者写反了 —— 查询成功时弹普通 info，查询失败却弹
    //    绿色 success，用户看到"成功"配着错误文案，完全对不上。）
    if (status.status === "ok") {
      toast.success("额度配置已保存", quotaLine(status));
    } else {
      toast.info("配置已保存，查询未成功", quotaLine(status));
    }
  } catch (err) {
    const msg = String(err).replace(/^.*?: /, "");
    saveError.value = msg;
    toast.error("保存失败", msg);
  } finally {
    saving.value = false;
  }
}

// ---- 托盘悬浮信息窗 ----
const overlayOn = ref(false);

async function toggleOverlay(): Promise<void> {
  try {
    overlayOn.value = await insightApi.overlayToggle(!overlayOn.value);
  } catch {
    overlayOn.value = false;
  }
}

const btTitle = computed(() => {
  const base = btText.value;
  return snapshot.value.bluetoothAtMs > 0 ? `${base}（${btAge.value}）` : base;
});
</script>

<template>
  <div class="insight-bar" data-tauri-drag-region>
    <!-- ── 蓝牙分组 ─────────────────────────────────────────── -->
    <div class="group" :class="[btClass, { 'has-devices': devices.length > 0 }]" :title="btTitle">
      <!-- 蓝牙图标：lucide「bluetooth」图标路径（stroke 风格） -->
      <svg class="g-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="m7 7 10 10-5 5V2l5 5L7 17" />
      </svg>
      <!-- 状态文案 + 多设备罗列：每台设备一个小胶囊 -->
      <span class="summary">{{ btText }}</span>
      <span v-if="devices.length" class="devices">
        <span v-for="dev in devices" :key="dev.id" class="chip" :title="`${dev.name} · ${kindLabel(dev.kind)}`">
          {{ dev.name }}
          <i class="kind">{{ kindLabel(dev.kind) }}</i>
          <i class="bat" :class="{ unknown: dev.batteryPercent === null }">
            {{ batteryText(dev.batteryPercent) }}
          </i>
        </span>
      </span>
      <!-- 立即刷新（节流回执给一句提示） -->
      <button class="mini-btn" title="立即刷新" @click="refreshBluetooth">
        <svg viewBox="0 0 14 14" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M12.5 7a5.5 5.5 0 1 1-1.6-3.9" />
          <path d="M12.5 1.5v2.6H9.9" />
        </svg>
      </button>
    </div>

    <!-- ── 额度分组 ─────────────────────────────────────────── -->
    <div class="group" :class="quotaClass" :title="quotaAge">
      <!-- API 额度图标：lucide「credit-card」图标路径 -->
      <svg class="g-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect width="20" height="14" x="2" y="5" rx="2" />
        <path d="M2 10h20" />
      </svg>
      <!-- 单值接口显示一句话；多窗口时下面每档一个胶囊，不重复罗列 -->
      <span v-if="!quotaWindows.length" class="summary">{{ quotaText }}</span>
      <!-- 多窗口：每档一个小胶囊（滚动 / 本周 / 本月），悬停看重置时间 -->
      <span v-if="quotaWindows.length" class="devices">
        <span
          v-for="win in quotaWindows"
          :key="win.label"
          class="chip"
          :title="`${win.label}窗口剩余 ${pct(win.remainingPercent)}，${resetText(win.resetsAt)}`"
        >
          {{ win.label }}
          <i class="bat" :class="{ unknown: win.remainingPercent <= 0, low: win.remainingPercent < 20 }">
            {{ pct(win.remainingPercent) }}
          </i>
        </span>
      </span>
      <button class="mini-btn" title="立即刷新额度" @click="refreshQuota">
        <svg viewBox="0 0 14 14" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M12.5 7a5.5 5.5 0 1 1-1.6-3.9" />
          <path d="M12.5 1.5v2.6H9.9" />
        </svg>
      </button>
      <button class="mini-btn" title="配置 API Key / 接口地址" @click="openConfig">
        <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
        </svg>
      </button>
      <!-- 悬浮信息窗开关：托盘就绪前禁用（Rust 还没建好托盘） -->
      <button
        class="mini-btn"
        :class="{ on: overlayOn }"
        :disabled="!trayReady"
        :title="trayReady ? '切换托盘旁的信息浮窗' : '托盘尚未就绪'"
        @click="toggleOverlay"
      >
        <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="4" width="18" height="12" rx="2" />
          <path d="M7 20h10" />
          <path d="M12 16v4" />
        </svg>
      </button>
    </div>

    <!-- 刷新节流提示 -->
    <Transition name="fade">
      <span v-if="lastBtThrottled" class="hint">刚扫过，请稍候再刷新</span>
    </Transition>

    <!-- ── 额度配置弹层 ─────────────────────────────────────── -->
    <Transition name="pop">
      <div v-if="configOpen" class="config-pop" @click.stop>
        <p class="cfg-title">opencode Go 额度配置</p>
        <label class="cfg-row">
          <span>API Key（Go 订阅的令牌）</span>
          <input v-model="keyInput" type="password" placeholder="oc_sk_…（留空则保持不变）" autocomplete="off" />
        </label>
        <label class="cfg-row">
          <span>额度查询接口地址（留空用内置默认）</span>
          <input
            v-model="endpointInput"
            type="url"
            placeholder="留空 → opencode Go 用量接口"
            autocomplete="off"
          />
        </label>
        <!-- 默认端点已内置，用户不必猜路径；自建网关仍可自填 -->
        <p class="cfg-hint">
          <template v-if="endpointIsDefault">
            当前使用内置端点
            <code>opencode.ai/zen/go/v1/usage</code>
            —— 填好 API Key 保存即可查询。
          </template>
          <template v-else>
            自建网关可用：
            <code>/api/user/self</code>
            （one-api 风格）或
            <code>/v1/dashboard/billing/usage</code>
            （OpenAI 风格）。
          </template>
        </p>
        <p v-if="saveError" class="cfg-err">{{ saveError }}</p>
        <div class="cfg-actions">
          <button class="cfg-btn ghost" @click="configOpen = false">取消</button>
          <button class="cfg-btn primary" :disabled="saving" @click="saveConfig">
            {{ saving ? "保存中…" : "保存并查询" }}
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* ------------------------------------------------------------ 整体布局 */
.insight-bar {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: var(--z-chrome);
  display: flex;
  align-items: center;
  gap: 0.7rem;
  /* 靠右的 FAB（组件控制台）需要留出位置，否则互相叠住 */
  padding: 0.2rem 72px 0.25rem 16px;
  border-top: 1px solid var(--glass-border);
  background: color-mix(in srgb, var(--glass-bg-deep) 82%, transparent);
  backdrop-filter: var(--glass-blur);
  user-select: none;
}

/* 两组信息放在左右两端，中间留白（弹层定位基准是整条栏） */
.group {
  display: flex;
  align-items: center;
  gap: 0.32rem;
  min-width: 0;
  font-size: 0.74rem;
  color: var(--ink-dim);
  white-space: nowrap;
}

.group + .group {
  margin-left: auto;
}

.g-icon {
  flex: none;
  opacity: 0.85;
}

/* 状态色：正常 / 空闲提示 / 警告 / 错误 */
.group.ok .g-icon {
  color: var(--accent);
}
.group.ok {
  color: var(--ink-soft);
}
.group.idle .g-icon {
  color: var(--ink-faint);
}
.group.warn .g-icon {
  color: #e8b64c;
}
.group.warn {
  color: #d8b15c;
}
.group.err .g-icon {
  color: #ff8a8a;
}
.group.err {
  color: #e08b8b;
}

.summary {
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 26vw;
}

/* ------------------------------------------------------- 多设备罗列 */
.devices {
  display: inline-flex;
  gap: 0.28rem;
  overflow: hidden;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 0.28rem;
  padding: 0.06rem 0.42rem;
  border: 1px solid var(--glass-border);
  border-radius: 999px;
  background: var(--glass-bg);
  font-size: 0.7rem;
  color: var(--ink-soft);
  max-width: 168px;
  overflow: hidden;
}
.chip .kind {
  font-style: normal;
  font-size: 0.62rem;
  color: var(--ink-faint);
  border: 1px solid var(--glass-border);
  border-radius: 4px;
  padding: 0 0.22rem;
}
.chip .bat {
  font-style: normal;
  font-variant-numeric: tabular-nums;
  color: var(--accent);
}
.chip .bat.unknown {
  color: var(--ink-faint);
}
/* 剩余低于 20% 的配额窗口标黄 —— 一眼看出哪个快用完了 */
.chip .bat.low {
  color: #e8b64c;
}

/* ------------------------------------------------------- 小按钮组 */
.mini-btn {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: 50%;
  background: transparent;
  color: var(--ink-faint);
  cursor: pointer;
  transition:
    color var(--t-base) ease,
    border-color var(--t-base) ease,
    background var(--t-base) ease;
}
.mini-btn:hover:not(:disabled) {
  color: var(--ink-soft);
  background: var(--glass-bg-strong);
  border-color: var(--glass-border);
}
.mini-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
.mini-btn.on {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 40%, transparent);
}

/* ------------------------------------------------------- 节流提示 */
.hint {
  position: absolute;
  right: 76px;
  bottom: 30px;
  padding: 0.2rem 0.6rem;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: var(--glass-bg-deep);
  backdrop-filter: var(--glass-blur);
  font-size: 0.68rem;
  color: var(--ink-dim);
  pointer-events: none;
}

/* ------------------------------------------------------- 配置弹层 */
.config-pop {
  position: absolute;
  right: 74px;
  bottom: 2.1rem;
  z-index: 5;
  width: 300px;
  padding: 0.75rem 0.85rem 0.8rem;
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  background: var(--glass-bg-deep);
  backdrop-filter: var(--glass-blur-lg);
  box-shadow: var(--glass-edge), 0 16px 40px rgb(0 0 0 / 0.38);
  color: var(--ink-soft);
}
.cfg-title {
  margin: 0 0 0.55rem;
  font-size: 0.7rem;
  letter-spacing: 0.08em;
  color: var(--ink-dim);
}
.cfg-row {
  display: grid;
  gap: 0.22rem;
  margin-bottom: 0.5rem;
  font-size: 0.7rem;
  color: var(--ink-dim);
}
.cfg-row input {
  min-width: 0;
  padding: 0.32rem 0.5rem;
  border: 1px solid var(--glass-border);
  border-radius: 8px;
  background: rgb(0 0 0 / 0.24);
  color: var(--ink-soft);
  font: inherit;
  font-size: 0.74rem;
  outline: none;
}
.cfg-row input:focus {
  border-color: color-mix(in srgb, var(--accent) 55%, transparent);
}
/* 两种常见网关路径的提示小字 */
.cfg-hint {
  margin: 0 0 0.45rem;
  font-size: 0.66rem;
  line-height: 1.55;
  color: var(--ink-faint);
}
.cfg-hint code {
  padding: 0 0.22rem;
  border: 1px solid var(--glass-border);
  border-radius: 4px;
  background: rgb(0 0 0 / 0.18);
  font-size: 0.62rem;
  color: var(--ink-dim);
}
.cfg-err {
  margin: 0 0 0.4rem;
  font-size: 0.68rem;
  color: #ff8a8a;
}
.cfg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 0.45rem;
  margin-top: 0.2rem;
}
.cfg-btn {
  padding: 0.28rem 0.85rem;
  border-radius: 8px;
  border: 1px solid var(--glass-border);
  background: transparent;
  color: var(--ink-dim);
  font: inherit;
  font-size: 0.72rem;
  cursor: pointer;
  transition: all var(--t-base) ease;
}
.cfg-btn.primary {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  color: var(--accent);
}
.cfg-btn.primary:disabled {
  opacity: 0.5;
}

/* ------------------------------------------------------- 动画 */
.pop-enter-active {
  transition: opacity 0.16s ease, transform 0.16s var(--ease-out-expo);
}
.pop-leave-active {
  transition: opacity 0.12s ease;
}
.pop-enter-from,
.pop-leave-to {
  opacity: 0;
  transform: translateY(6px) scale(0.97);
}
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

/* ------------------------------------------------------- 响应式降级
   760px 是常规窗口的最小宽度，两组信息（设备胶囊 + 额度 + 一排按钮）
   在窄窗下并排放不下。降级策略：
     · 设备胶囊与主文案收窄，靠 ellipsis 兜底（悬停 title 看全文）；
     · 绝不换行 —— 底部栏一旦变成两行会顶起整个页面布局，
       在 560px 的矮窗口里直接会把表盘挤出视口。
   低于 820px 时隐藏蓝牙的「文字摘要」，只留设备胶囊 ——
   胶囊已经把「名字 + 类型 + 电量」都说全了，摘要纯属重复。 */
@media (max-width: 900px) {
  .insight-bar {
    padding-right: 64px;
    gap: 0.45rem;
  }
  .chip {
    max-width: 112px;
  }
  .summary {
    max-width: 20vw;
  }
  .config-pop {
    right: 12px;
    width: min(300px, calc(100vw - 24px));
  }
  /* 窄窗下摘要与胶囊二选一：有设备胶囊时摘要就是重复信息
     （胶囊已经把「名字 + 类型 + 电量」说全了）。
     这里用容器类 `has-devices` 而不是兄弟选择器 —— `.summary` 在
     `.devices` 之前，`~` 选不到它。 */
  .group.has-devices .summary {
    display: none;
  }
}
</style>