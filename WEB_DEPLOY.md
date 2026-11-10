# HGhub Web 服务版（本 fork 的改造）

把上游 Tauri 客户端改造成可在 NAS / 任意 Linux 主机上以 Docker 跑的 Web 服务：
浏览器打开页面即可刷剧、播放，后端逻辑与客户端版共用同一套 Rust 代码。

> 许可：沿用上游 PolyForm Noncommercial 1.0.0 与 LICENSE 中的 Required Notice。
> **仅限个人非商业自用，不要公开分发、不要对公众提供服务。**

## 改造点

| 部位 | 做法 |
| --- | --- |
| Rust 后端 | 新增 `server/` crate（axum）。`src-tauri/src` 的 net / media / session / lines / network_policy / logging / playback_log 模块移植复用（文件头有移植注记）；api 命令层去掉 Tauri 外壳后由 `POST /api/{命令}` 分发 |
| 媒体 | 原 `hg://` 自定义协议改为同源 HTTP：`GET /img`（封面 AES 解密）、`GET /hls`（播放列表改写）、`GET /seg`（分片/密钥代理），改写前缀取请求实际 Host，分片回指本服务 |
| 前端 | `vite build --mode web` 专用构建：经 vite alias 把 `@tauri-apps/api/core` 换成 `src/web/tauriCore.ts`（invoke → HTTP）、`@tauri-apps/api/window` 换成空实现垫片；`src/api.ts` 在 Web 标记下媒体地址走同源、外链用浏览器打开。客户端版构建（`pnpm build` / Tauri）不受影响 |
| 会话 | 与客户端版相同：服务端单会话，登录态落盘在数据目录 `session.json`（所有浏览器共用一个登录，自用场景正合适） |
| 安全 | API 与媒体路由都做同源校验（Origin 与 Host 不一致拒绝），等价于客户端版对 `hg://` 的来源校验；服务端本身没有再加账号门禁，**只应在局域网 / Tailscale 内使用，不要暴露到公网** |

## 构建与运行

```bash
# NAS（QNAP Container Station 的 docker）或任意装了 Docker 的主机：
# 仓库放到 /share/Public/hghub/app，数据目录 /share/Public/hghub/data
docker compose -f docker-compose.web.yml up -d --build
```

打开 `http://<NAS 局域网 IP>:8892/`（Tailscale 地址同理）。

端口 8892 可在 `docker-compose.web.yml` 左侧修改。数据卷默认
`/share/Public/hghub/data:/data`，容器重启不丢登录与线路选择。

## 本地开发（不用 Docker）

```bash
pnpm install
pnpm build:web                     # 产物在 dist/
cd server && cargo run --release   # 需在仓库根目录外另开终端
# 环境变量：HG_PORT=8892 HG_DATA_DIR=./data HG_DIST_DIR=../dist
```

注意 `cargo run` 的工作目录决定相对路径，本地联调建议显式给绝对路径的环境变量。

## 已知差异（相对客户端版）

- 桌面标题栏的最小化/最大化/关闭按钮在网页里无动作（浏览器窗口控制为准）。
- 「打开外链」改为浏览器新标签页打开。
- 检查更新仍对比上游 Minireel/HGhub 的 Release，仅作提示，不影响使用。
- 屏幕方向、安卓返回键等移动端宿主能力为空实现。
