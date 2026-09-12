// ---------------------------------------------------------------------------
// 轻量 Toast。
//
// 为什么用模块级单例而不是 provide/inject：提示是**跨层级**的横切关注点
// （导出按钮在统计页深处、重置按钮在计时页、托盘提示在 App 层），
// 一路往上传事件会把所有中间组件都变成"信使"。模块级 ref 让任何地方
// 一行调用就能弹提示，代价是全局唯一一份状态 —— 对提示恰好是想要的语义。
//
// 队列行为：新的在**上方**（数组尾部），先来的先自动消解。
// ---------------------------------------------------------------------------

import { ref, type Ref } from "vue";

export type ToastKind = "info" | "success" | "error";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  title: string;
  /** 次要说明，如落盘路径、改写条数。 */
  detail?: string;
  /** 自动消解的毫秒数（用于进度条动画）。 */
  ttl: number;
}

/** 同屏最多保留的条数：再多就该被上一条的 TTL 收走了，堆叠会挡住界面。 */
const MAX_VISIBLE = 4;

const items = ref<ToastItem[]>([]);
let seq = 0;

export const toasts: Ref<ToastItem[]> = items;

export function dismissToast(id: number): void {
  items.value = items.value.filter((item) => item.id !== id);
}

export function pushToast(
  kind: ToastKind,
  title: string,
  detail?: string,
  ttl = 3600,
): number {
  const id = ++seq;
  const next = [...items.value, { id, kind, title, detail, ttl }];
  // 超限时把最老的挤掉，保证最新的一条一定看得见
  items.value = next.length > MAX_VISIBLE ? next.slice(next.length - MAX_VISIBLE) : next;
  window.setTimeout(() => dismissToast(id), ttl);
  return id;
}

/** 用法：`toast.success("已导出", path)`。 */
export const toast = {
  info: (title: string, detail?: string): number => pushToast("info", title, detail, 3600),
  success: (title: string, detail?: string): number => pushToast("success", title, detail, 4400),
  /** 错误信息通常更长，多看一会儿才读得完。 */
  error: (title: string, detail?: string): number => pushToast("error", title, detail, 6400),
};
