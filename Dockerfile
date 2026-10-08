# syntax=docker/dockerfile:1

FROM node:24-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
# npm ci runs `prepare` (svelte-kit sync && panda codegen), which reads these configs.
COPY frontend/panda.config.ts frontend/svelte.config.js frontend/vite.config.ts frontend/tsconfig.json frontend/postcss.config.js ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.96-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=frontend /app/frontend/build ./frontend/build
RUN cargo build --locked --release --bin central

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        iputils-ping \
        mtr-tiny \
        traceroute \
        ca-certificates \
        libcap2-bin \
        tini \
    && groupadd --system lookingglass \
    && useradd --system --gid lookingglass --home-dir /nonexistent --shell /usr/sbin/nologin lookingglass \
    && mkdir -p /data/files \
    && chown -R lookingglass:lookingglass /data \
    && for bin in /usr/bin/ping /usr/bin/mtr-packet /usr/bin/traceroute; do \
        real="$(readlink -f "$bin")"; \
        [ -f "$real" ] && setcap cap_net_raw+ep "$real"; \
    done \
    && rm -rf /var/lib/apt/lists/*
COPY --from=backend /app/target/release/central /usr/local/bin/central
COPY LICENSE THIRD_PARTY_NOTICES.md /usr/share/doc/looking-glass/
# release.yml fills these with the tag's asset URLs and README pins; a plain
# `docker build` leaves them empty, so central refuses to hand out install commands.
ARG LG_AGENT_URL= \
    LG_AGENT_SHA256= \
    LG_AGENT_INSTALL_SCRIPT_URL= \
    LG_AGENT_INSTALL_SCRIPT_SHA256=
ENV PORT=8080 \
    LG_DB_PATH=/data/lookingglass.redb \
    LG_FILES_DIR=/data/files \
    LG_AGENT_URL=$LG_AGENT_URL \
    LG_AGENT_SHA256=$LG_AGENT_SHA256 \
    LG_AGENT_INSTALL_SCRIPT_URL=$LG_AGENT_INSTALL_SCRIPT_URL \
    LG_AGENT_INSTALL_SCRIPT_SHA256=$LG_AGENT_INSTALL_SCRIPT_SHA256
EXPOSE 8080 8443
USER lookingglass
# No curl in the image: bash's /dev/tcp probes /health directly.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --start-interval=2s \
    CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/$PORT && printf "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n" >&3 && head -n1 <&3 | grep -q " 200 "'
# tini is PID 1: it forwards `docker stop`'s SIGTERM to central (which shuts
# down on it) and reaps any orphaned process a diagnostic tool leaves behind.
ENTRYPOINT ["/usr/bin/tini", "--", "central"]
