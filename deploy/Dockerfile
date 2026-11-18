# syntax=docker/dockerfile:1.7
# Two images from one file:
#   --target server  Rust media/API server with ffmpeg
#   --target web     Caddy with the built web app (HTTPS, static files, archive media)

FROM node:24-slim AS web-build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM rust:1-slim-trixie AS server-build
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang perl pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server/ server/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p schoollive && cp target/release/schoollive /schoollive

FROM debian:trixie-slim AS server
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data schoollive \
 && mkdir -p /data && chown schoollive /data
COPY --from=server-build /schoollive /usr/local/bin/schoollive
USER schoollive
ENV DATA_DIR=/data HTTP_ADDR=127.0.0.1:8080 RUST_LOG=schoollive=info,str0m=warn
VOLUME /data
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/schoollive"]

FROM caddy:2.11-alpine AS web
COPY --from=web-build /web/dist /srv/web
COPY deploy/caddy/ /etc/caddy/
