# syntax=docker/dockerfile:1

FROM node:24-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.96-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=frontend /app/frontend/build ./frontend/build
RUN cargo build --release --bin central

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        iputils-ping \
        mtr-tiny \
        traceroute \
        ca-certificates \
        libcap2-bin \
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
ENV PORT=8080 \
    LG_DB_PATH=/data/lookingglass.redb \
    LG_FILES_DIR=/data/files
EXPOSE 8080
USER lookingglass
ENTRYPOINT ["central"]
