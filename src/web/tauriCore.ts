/**
 * Web 版 `@tauri-apps/api/core` 垫片（仅 --mode web 构建时经 vite alias 替换）。
 *
 * - `invoke(cmd, args)` → `POST /api/{cmd}`，与 hghub-server 的分发约定对应：
 *   成功返回结果 JSON；失败时服务端返回 JSON 字符串错误，这里原样抛出字符串，
 *   与 Tauri invoke 的 reject 形态保持一致（src/api.ts 的 call() 按字符串处理）。
 * - `addPluginListener` 只用于安卓宿主的返回键，浏览器无此宿主，返回空句柄。
 */
export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const resp = await fetch(`/api/${encodeURIComponent(cmd)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  const data: unknown = await resp.json().catch(() => null);
  if (!resp.ok) {
    throw typeof data === "string" ? data : `请求失败（HTTP ${resp.status}）`;
  }
  return data as T;
}

export interface PluginListener {
  unregister: () => Promise<void>;
}

export async function addPluginListener<T>(
  _plugin: string,
  _event: string,
  _callback: (payload: T) => void,
): Promise<PluginListener> {
  return { unregister: async () => {} };
}
