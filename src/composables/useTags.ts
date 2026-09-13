// ---------------------------------------------------------------------------
// 科目标签：下一条会话开始前选好，落库那一刻固定（会话是历史事实）。
//
// P1-⑤ 第二步起改为**模块级单例**：选中态被 App（迷你组件展示当前科目、
// 启动时 tagCurrent 回填）与 TagRow（胶囊行/编辑/删除）两处消费，必须
// 共享同一份 ref。
//
// 标签集合归用户管：预设只是**初始值**，之后可以新增（输入框回车）、双击重命名、
// 点 × 删除。集合存本地 —— 它是 UI 偏好，不是需要跨设备对齐的数据；
// 而历史会话上的 tag 是既成事实，改名只影响"下一条"，不回改历史。
// ---------------------------------------------------------------------------

import { nextTick, ref } from "vue";
import { timerApi } from "../api";
import { TAG_PRESETS } from "../tags";
import { toast } from "./useToast";

const TAGS_KEY = "pristimer.tags";
/** 上限纯防御：胶囊行换到第三排就开始难看了。 */
const MAX_TAGS = 12;

function loadTags(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(TAGS_KEY) ?? "null") as unknown;
    if (Array.isArray(raw)) {
      const clean = raw
        .filter((t): t is string => typeof t === "string")
        .map((t) => t.trim().slice(0, 20))
        .filter((t) => t.length > 0);
      // 去重：手改过的 localStorage 可能塞进重复项
      if (clean.length) return [...new Set(clean)].slice(0, MAX_TAGS);
    }
  } catch {
    /* 坏数据回默认预设 */
  }
  return [...TAG_PRESETS];
}

// ---- 模块级单例状态 -------------------------------------------------------

const tags = ref<string[]>(loadTags());
const selectedTag = ref<string | null>(null);
const tagInput = ref("");

function saveTags(): void {
  try {
    localStorage.setItem(TAGS_KEY, JSON.stringify(tags.value));
  } catch {
    /* 隐身模式等写不进去的场景：本次会话内生效即可 */
  }
}

/** 选中即切、再点取消。 */
async function applyTag(tag: string | null): Promise<void> {
  try {
    selectedTag.value = await timerApi.tagSet(tag);
    tagInput.value = "";
  } catch (err) {
    console.error("设置科目标签失败", err);
  }
}

/** 回车提交输入框：已存在的直接选中，新的先入列再选中。 */
async function submitTag(): Promise<void> {
  const value = tagInput.value.trim().slice(0, 20);
  if (!value) return;
  if (!tags.value.includes(value)) {
    if (tags.value.length >= MAX_TAGS) {
      toast.error("标签太多了", `最多 ${MAX_TAGS} 个，先删掉几个再加`);
      return;
    }
    tags.value = [...tags.value, value];
    saveTags();
  }
  await applyTag(value);
}

// ---- 重命名与删除 ---------------------------------------------------------

/** 正在内联重命名的标签（null = 没有）。同一时刻只会有一个。 */
const editingTag = ref<string | null>(null);
const editingText = ref("");

async function startTagEdit(tag: string): Promise<void> {
  editingTag.value = tag;
  editingText.value = tag;
  await nextTick();
  // v-if 刚插入的 input 不会自动聚焦，只能自己找回来；同时只有一个，全局选择器够用
  const el = document.querySelector<HTMLInputElement>(".tag-edit");
  el?.focus();
  el?.select();
}

function cancelTagEdit(): void {
  editingTag.value = null;
}

/** 提交重命名。空串 / 未改动 / 撞名都安全退出，不留半截状态。 */
async function commitTagEdit(): Promise<void> {
  const from = editingTag.value;
  if (from === null) return;
  const next = editingText.value.trim().slice(0, 20);
  editingTag.value = null;
  if (!next || next === from) return;
  if (tags.value.includes(next)) {
    toast.error("标签重复", `「${next}」已经在列表里了`);
    return;
  }
  const wasSelected = selectedTag.value === from;
  tags.value = tags.value.map((t) => (t === from ? next : t));
  saveTags();
  // 改的正是当前选中的那个 → 同步跟进，否则下一条会话会挂着一个已不存在的名字
  if (wasSelected) await applyTag(next);
  toast.success("已重命名", `${from} → ${next}`);
}

/** 删除标签；删掉的正好是选中的那个时顺手清空选择。 */
async function removeTag(tag: string): Promise<void> {
  if (!tags.value.includes(tag)) return;
  tags.value = tags.value.filter((t) => t !== tag);
  saveTags();
  if (selectedTag.value === tag) await applyTag(null);
  toast.info("已删除标签", tag);
}

// ---- 消费入口（多次调用返回同一份状态）------------------------------------

export function useTags() {
  return {
    tags,
    selectedTag,
    tagInput,
    editingTag,
    editingText,
    applyTag,
    submitTag,
    startTagEdit,
    cancelTagEdit,
    commitTagEdit,
    removeTag,
  };
}
