// ---------------------------------------------------------------------------
// 背景主题（六主题）+ 底部主题菜单。
//
// 从 App.vue 抽出的纯逻辑（P1-⑤ 第一步，不改行为）：切换只换 data-theme +
// 一组 CSS 变量（@property 注册过，0.9s 平滑插值），特色动效层由 SmokeField
// 按 theme 交叉换景。持久化走 localStorage（pristimer.theme）。
// ---------------------------------------------------------------------------

import { computed, ref, useTemplateRef, watch } from "vue";

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

export function useTheme() {
  const theme = ref<BgTheme>(loadTheme());
  const currentThemeLabel = computed(
    () => THEMES.find((t) => t.id === theme.value)?.label ?? "深空",
  );
  const currentThemeSwatch = computed(
    () => THEMES.find((t) => t.id === theme.value)?.swatch ?? "",
  );

  /** 底部主题菜单（第 21 轮：主题键从裸色点升级为底部菜单 + 弹出面板）。
   *  useTemplateRef：模板里 ref="themeCtlRoot" 按名字绑进来，App 无需中转。 */
  const themeMenuOpen = ref(false);
  const themeCtlRoot = useTemplateRef<HTMLElement>("themeCtlRoot");

  function pickTheme(id: BgTheme): void {
    theme.value = id;
    themeMenuOpen.value = false;
  }

  function onGlobalPointerDown(e: PointerEvent): void {
    if (!themeMenuOpen.value) return;
    if (themeCtlRoot.value && !themeCtlRoot.value.contains(e.target as Node)) {
      themeMenuOpen.value = false;
    }
  }

  function onGlobalKeydown(e: KeyboardEvent): void {
    if (e.key === "Escape") themeMenuOpen.value = false;
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

  return {
    THEMES,
    theme,
    currentThemeLabel,
    currentThemeSwatch,
    themeMenuOpen,
    pickTheme,
    onGlobalPointerDown,
    onGlobalKeydown,
    syncTheme,
  };
}
