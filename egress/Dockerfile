FROM docker.1panel.live/library/alpine:3.20
COPY sing-box /usr/local/bin/sing-box
RUN chmod +x /usr/local/bin/sing-box
COPY config.json /etc/sing-box/config.json
ENTRYPOINT ["sing-box", "run", "-c", "/etc/sing-box/config.json"]
