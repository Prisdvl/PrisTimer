// ---------------------------------------------------------------------------
// 背景主题（六主题）。
//
// P1-⑤ 第二步起改为**模块级单例**（与 useToast 同款理由）：主题是跨层级
// 的横切状态 —— App 的 SmokeField/AnalogDial 吃 theme，底部菜单组件
// ThemeMenu 直接消费 THEMES/pickTheme，若每个调用方各建一份 ref，
// 菜单里点选就不会传导到背景层。模块级 ref = 全局唯一一份，恰好是
// 主题想要的语义。菜单的开合/外点关闭是纯 UI 态，归 ThemeMenu 组件私有。
//
// 切换只换 data-theme + 一组 CSS 变量（@property 注册过，0.9s 平滑插值），
// 特色动效层由 SmokeField 按 theme 交叉换景。持久化走 localStorage
// （pristimer.theme）。
// ---------------------------------------------------------------------------

import { computed, ref, watch } from "vue";

export type BgTheme = "deep" | "void" | "dawn" | "aurora" | "ember" | "paper";

export const THEMES: Array<{ id: BgTheme; label: string; swatch: string }> = [
  { id: "deep", label: "深空", swatch: "linear-gradient(135deg,#6082ff,#967dff 60%,#46a5e1)" },
  { id: "void", label: "虚空", swatch: "linear-gradient(135deg,#0a0b10,#343060)" },
  { id: "dawn", label: "晨雾", swatch: "linear-gradient(135deg,#eef3fa,#9fc6e8)" },
  { id: "aurora", label: "极光", swatch: "linear-gradient(135deg,#2fd48e,#1f8fa8)" },
  { id: "ember", label: "暮霞", swatch: "linear-gradient(135deg,#ff8a50,#d45a7a 62%,#5a3a4e)" },
  { id: "paper", label: "纸墨", swatch: "linear-gradient(135deg,#f4f1ea,#b0a890)" },
];

const THEME_KEY = "pristimer.theme";

/** 迁移：旧版值（aurora=出厂绿 / abyss=深海）都已退役，统一落到 deep。 */
function loadTheme(): BgTheme {
  const raw = localStorage.getItem(THEME_KEY);
  if (raw === "void" || raw === "dawn" || raw === "aurora" || raw === "deep") return raw;
  return "deep";
}

// ---- 模块级单例状态 -------------------------------------------------------

const theme = ref<BgTheme>(loadTheme());

const currentThemeLabel = computed(
  () => THEMES.find((t) => t.id === theme.value)?.label ?? "深空",
);
const currentThemeSwatch = computed(
  () => THEMES.find((t) => t.id === theme.value)?.swatch ?? "",
);

function pickTheme(id: BgTheme): void {
  theme.value = id;
}

function syncTheme(): void {
  document.documentElement.setAttribute("data-theme", theme.value);
}

watch(theme, (t) => {
  try {
    localStorage.setItem(THEME_KEY, t);
  } catch {
    /* 写不进去就本次会话生效 */
  }
  syncTheme();
});

// ---- 消费入口（多次调用返回同一份状态）------------------------------------

export function useTheme() {
  return {
    THEMES,
    theme,
    currentThemeLabel,
    currentThemeSwatch,
    pickTheme,
    syncTheme,
  };
}
