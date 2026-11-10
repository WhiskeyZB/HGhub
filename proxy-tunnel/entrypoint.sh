#!/bin/sh
# 环境变量：TUNNEL_TARGET=user@host（必填），TUNNEL_PORT=1080（SOCKS 监听端口）
set -e
TARGET="${TUNNEL_TARGET:?TUNNEL_TARGET required}"
PORT="${TUNNEL_PORT:-1080}"
exec ssh -N \
  -D "0.0.0.0:${PORT}" \
  -i /key/id_ed25519 \
  -o StrictHostKeyChecking=accept-new \
  -o UserKnownHostsFile=/tmp/known_hosts \
  -o ServerAliveInterval=30 \
  -o ServerAliveCountMax=3 \
  -o ExitOnForwardFailure=yes \
  -o BatchMode=yes \
  "${TARGET}"
