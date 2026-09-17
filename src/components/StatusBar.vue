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
import { useInsight, batteryText, shortName, ageLabel, isBtOk } from "../composables/useInsight";
import { toast } from "../composables/useToast";
import { insightApi } from "../api";

const {
  snapshot,
  quotaStale,
  trayReady,
  lastBtThrottled,
  refreshBluetooth,
  refreshQuota,
  applyQuota,
  deviceLine,
  quotaLine,
} = useInsight();

// ---- 蓝牙分组 ----

/** 类别排序权重：音频设备排最前（耳机剩多少电是用户最关心的信息）。 */
const CATEGORY_ORDER: Record<string, number> = { audio: 0, keyboard: 1, mouse: 2, other: 3 };

const devices = computed(() => {
  if (!isBtOk(snapshot.value.bluetooth)) return [];
  // ★ 排序在**拷贝**上做：`devices` 是快照里的数组，原地 sort 会改到
  //   useInsight 的单例状态（同一份数据托盘浮窗也在用）。
  return [...snapshot.value.bluetooth.devices].sort((a, b) => {
    const byCategory = (CATEGORY_ORDER[a.category] ?? 9) - (CATEGORY_ORDER[b.category] ?? 9);
    // 同类里电量低的排前 —— 快没电的那台最该被看见；未知电量排最后。
    return byCategory !== 0
      ? byCategory
      : (a.batteryPercent ?? 101) - (b.batteryPercent ?? 101);
  });
});

/**
 * 设备名缩写见 useInsight 的 `shortName` —— 浮窗也用同一套规则，
 * 两处各写一份迟早会不一致（同一台设备在两个界面上叫不同名字很怪）。
 */

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

/**
 * 状态色。
 *
 * ★ 与 `quotaStale` 的分工（2026-09-17 第 30 轮）：这里的四档描述的是
 *   **快照里那个值本身**是什么状态；"这一次没问到"不进这里，它为真时
 *   值仍是上一次的成功结果（`ok`），走 `.stale` 那条更轻的视觉通道 ——
 *   一次网络抖动不该把整组信息染成告警色。
 */
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

/** 最近一次刷新失败（显示的是保留的旧值）。 */
const quotaIsStale = computed(() => quotaStale.value !== null);

/** 多窗口额度（opencode Go：滚动 / 本周 / 本月）。空数组表示单值接口。 */
const quotaWindows = computed(() =>
  snapshot.value.quota.status === "ok" ? snapshot.value.quota.windows ?? [] : [],
);

/**
 * 状态栏**只显示"本月"**这一档。
 *
 * 用户的判断：三个窗口同时铺在状态栏里太占地方，而"本月还剩多少"
 * 是唯一需要随时瞄一眼的量 —— 滚动/本周的细节放进点击后的详情浮层。
 * 找不到"本月"（非 opencode Go 的接口）就退回最后一档，再不然交给
 * 单值文案。
 */
const monthWindow = computed(() => {
  const list = quotaWindows.value;
  if (!list.length) return null;
  return list.find((w) => w.label.includes("本月")) ?? list[list.length - 1];
});

/** 额度详情浮层（点击状态栏的额度胶囊展开）。 */
const detailOpen = ref(false);

/** 本月额度的紧张程度，用于标黄/标红。 */
const monthLow = computed(() => (monthWindow.value?.remainingPercent ?? 100) < 20);


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
  // 两个浮层互斥：配置面板与额度详情不叠在一起。
  detailOpen.value = false;
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

/**
 * 额度分组的悬停提示 —— 这里也是"陈旧"状态的**主要出口**：
 * 状态栏能放的字太少，可见部分只加一枚"未更新"小标，
 * 具体是哪一次刷新、什么原因、数据多旧，全部在这一句里说清。
 */
const quotaTitle = computed(() => {
  if (!quotaIsStale.value) return quotaAge.value;
  return `${quotaText.value}（显示 ${quotaAge.value}的数据；最近一次刷新失败：${quotaStale.value?.message}）`;
});

/** 详情浮层里那行"未更新"的原因。 */
const quotaStaleReason = computed(() => quotaStale.value?.message ?? "");
</script>

<template>
  <div class="insight-bar" data-tauri-drag-region>
    <!-- ── 蓝牙分组 ─────────────────────────────────────────── -->
    <div class="group" :class="[btClass, { 'has-devices': devices.length > 0 }]" :title="btTitle">
      <!-- 蓝牙图标：lucide「bluetooth」图标路径（stroke 风格） -->
      <svg class="g-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="m7 7 10 10-5 5V2l5 5L7 17" />
      </svg>
      <!-- 无设备时给一句话；有设备时整行交给紧凑胶囊，不重复罗列 -->
      <span v-if="!devices.length" class="summary">{{ btText }}</span>
      <span v-if="devices.length" class="devices">
        <span
          v-for="dev in devices"
          :key="dev.id"
          class="chip"
          :title="`${dev.name}${dev.batteryPercent === null ? ' · 电量未知' : ` · ${dev.batteryPercent}%`}`"
        >
          <!-- 类型图标：内联 SVG，颜色跟随主题（audio / keyboard / mouse / 通用蓝牙） -->
          <svg class="d-icon" viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <template v-if="dev.category === 'audio'">
              <path d="M3 14h3a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z" />
              <path d="M21 14h-3a1 1 0 0 0-1 1v3a1 1 0 0 0 1 1h2a1 1 0 0 0 1-1z" />
              <path d="M3 14v-2a9 9 0 0 1 18 0v2" />
            </template>
            <template v-else-if="dev.category === 'keyboard'">
              <rect x="2" y="6" width="20" height="12" rx="2" />
              <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" />
            </template>
            <template v-else-if="dev.category === 'mouse'">
              <rect x="7" y="2" width="10" height="20" rx="5" />
              <path d="M12 6v4" />
            </template>
            <template v-else>
              <path d="m7 7 10 10-5 5V2l5 5L7 17" />
            </template>
          </svg>
          <span class="d-name">{{ shortName(dev.name) }}</span>
          <i
            class="bat"
            :class="{
              unknown: dev.batteryPercent === null,
              low: (dev.batteryPercent ?? 100) < 20,
            }"
          >
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
    <div class="group" :class="[quotaClass, { stale: quotaIsStale }]" :title="quotaTitle">
      <!-- API 额度图标：lucide「credit-card」图标路径 -->
      <svg class="g-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect width="20" height="14" x="2" y="5" rx="2" />
        <path d="M2 10h20" />
      </svg>
      <!-- 「未更新」小标：数值仍是上一次的成功结果，只是这次没问到。
           可见部分只有一个词，原因在 title 与详情浮层里。 -->
      <i v-if="quotaIsStale" class="stale-tag">未更新</i>
      <!-- 单值接口（非 opencode Go）显示一句话；多窗口只露"本月"，点击看全部 -->
      <span v-if="!monthWindow" class="summary">{{ quotaText }}</span>
      <button
        v-if="monthWindow"
        class="chip chip-btn"
        :class="{ open: detailOpen }"
        :title="`本月额度剩余 ${pct(monthWindow.remainingPercent)}，${resetText(monthWindow.resetsAt)} —— 点击查看全部窗口`"
        @click.stop="detailOpen = !detailOpen"
      >
        <span class="d-name">本月</span>
        <i
          class="bat"
          :class="{ unknown: monthWindow.remainingPercent <= 0, low: monthLow }"
        >
          {{ pct(monthWindow.remainingPercent) }}
        </i>
        <svg class="caret" viewBox="0 0 12 12" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M3 4.5 6 7.5 9 4.5" />
        </svg>
      </button>
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

    <!-- ── 额度详情浮层：点"本月"胶囊展开（三窗口 + 进度条 + 重置时间） ── -->
    <Transition name="pop">
      <div v-if="detailOpen" class="quota-pop" @click.stop>
        <p class="cfg-title">额度详情</p>
        <div v-for="win in quotaWindows" :key="win.label" class="qp-row">
          <span class="qp-label">{{ win.label }}</span>
          <span class="qp-track">
            <i
              class="qp-fill"
              :class="{ low: win.remainingPercent < 20, empty: win.remainingPercent <= 0 }"
              :style="{ width: `${Math.max(0, Math.min(100, win.remainingPercent))}%` }"
            />
          </span>
          <span class="qp-pct">{{ pct(win.remainingPercent) }}</span>
          <span class="qp-reset">{{ resetText(win.resetsAt) }}</span>
        </div>
        <p class="cfg-hint">
          更新于 {{ quotaAge }}
          <template v-if="quotaIsStale">
            ·
            <span class="qp-stale">最近一次刷新失败：{{ quotaStaleReason }}</span>
          </template>
        </p>
      </div>
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

/* ------------------------------------------------------- "未更新"（软失败）
   一次网络抖动不该把整组染成告警色 —— 数值本身仍然是可信的（上一次的成功结果），
   只是"这一次没问到"。所以这里只做两件事：把强调色降下来，并挂一枚小标。
   颜色一律走 --ink 令牌混合，浅色主题下同样是"变淡"而不是"消失"。 */
.group.stale {
  color: var(--ink-dim);
}
.group.stale .g-icon {
  color: var(--ink-faint);
}
/* 百分比本身也必须跟着降下来：`.chip .bat` 是显式 `--accent`，
   只把 `.group` 变淡的话数值仍是一副"新鲜"的强调色，与"这是旧值"
   自相矛盾 —— 实测第一版就是这样（数值绿着、旁边挂着"未更新"）。 */
.group.stale .chip .bat {
  color: var(--ink-dim);
}
.stale-tag {
  padding: 0 0.3rem;
  border: 1px dashed color-mix(in srgb, var(--ink) 28%, transparent);
  border-radius: 4px;
  font-style: normal;
  font-size: 0.62rem;
  letter-spacing: 0.02em;
  color: var(--ink-faint);
  white-space: nowrap;
}
.qp-stale {
  color: var(--ink-faint);
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

/* 类型图标 + 缩短后的设备名（全名保留在 title 里） */
.chip .d-icon {
  flex: none;
  color: var(--ink-dim);
}
.chip .d-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 92px;
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

/* ------------------------------------------------------- 额度详情浮层 */
.quota-pop {
  position: absolute;
  right: 74px;
  bottom: 2.1rem;
  z-index: 6;
  width: 272px;
  padding: 0.7rem 0.8rem 0.6rem;
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  background: var(--glass-bg-deep);
  backdrop-filter: var(--glass-blur-lg);
  box-shadow: var(--glass-edge), 0 16px 40px rgb(0 0 0 / 0.38);
  color: var(--ink-soft);
}
.qp-row {
  display: grid;
  grid-template-columns: 2.4rem 1fr 2.4rem auto;
  align-items: center;
  gap: 0.45rem;
  margin-bottom: 0.42rem;
  font-size: 0.7rem;
}
.qp-label {
  color: var(--ink-dim);
}
/* 进度条底槽用**主题相对**的叠层（color-mix(--ink)）：
   写死 rgb(255 255 255 / x) 在浅色主题下会整条隐形。 */
.qp-track {
  position: relative;
  height: 5px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--ink) 14%, transparent);
  overflow: hidden;
}
.qp-fill {
  position: absolute;
  inset: 0 auto 0 0;
  border-radius: 999px;
  background: var(--accent);
  /* 宽度做成动画：刷新后能直观看出"少了多少" */
  transition: width 0.45s cubic-bezier(0.22, 1, 0.36, 1);
}
.qp-fill.low {
  background: #e8b64c;
}
.qp-fill.empty {
  background: color-mix(in srgb, var(--ink) 30%, transparent);
}
.qp-pct {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.qp-reset {
  color: var(--ink-faint);
  font-size: 0.66rem;
  white-space: nowrap;
}

/* ------------------------------------------------------- 可点击胶囊（本月额度） */
.chip-btn {
  font: inherit;
  cursor: pointer;
  transition:
    border-color var(--t-base) ease,
    background var(--t-base) ease;
}
.chip-btn:hover,
.chip-btn.open {
  border-color: color-mix(in srgb, var(--accent) 45%, transparent);
  background: var(--glass-bg-strong);
}
.chip-btn .caret {
  color: var(--ink-faint);
  transition: transform var(--t-base) ease;
}
.chip-btn.open .caret {
  transform: rotate(180deg);
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
  /* 窄窗下"未更新"小标让位给数值本身：状态栏一旦换行会顶起整页布局，
     title 里那一句仍在（悬停可见），信息不丢。 */
  .stale-tag {
    display: none;
  }
}
</style>