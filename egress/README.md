# egress 边车（sing-box 出口）

NAS 部署时把本目录放到 app 目录的**同级**（即 ../egress，compose 里 egress 服务的 build context），
放入 sing-box 二进制（linux-amd64 musl 版）与真实 config.json（复制 config.example.json 改），
真实配置含节点凭据，**不要提交进仓库**。
