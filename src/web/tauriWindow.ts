/**
 * Web 版 `@tauri-apps/api/window` 垫片（仅 --mode web 构建时经 vite alias 替换）。
 * 浏览器没有宿主窗口可控，所有窗口操作为空实现；桌面壳的标题栏按钮在网页里点击无动作，
 * 关闭/最小化请用浏览器自身的窗口控制。
 */
export function getCurrentWindow() {
  return {
    minimize: async () => {},
    toggleMaximize: async () => {},
    close: async () => {},
    isMaximized: async () => false,
    isFullscreen: async () => false,
    setFullscreen: async (_fullscreen: boolean) => {},
    onResized: async (_handler: () => void) => () => {},
    startDragging: async () => {},
    startResizeDragging: async (_direction: string) => {},
  };
}
