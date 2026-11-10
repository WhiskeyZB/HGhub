# SSH 隧道边车：以受限密钥登录出口 VPS，开 SOCKS5 动态转发给 hghub 服务用。
# 密钥在运行时只读挂载（/key/id_ed25519），不打进镜像。
FROM alpine:3.20
RUN apk add --no-cache openssh-client
COPY entrypoint.sh /entrypoint.sh
RUN chmod 755 /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
