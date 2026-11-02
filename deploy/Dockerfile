# Nitch server + web app in one image. Build context: the repository root.
#   docker compose -f deploy/compose.yaml up -d --build
# Base is Ubuntu 24.04, the same release as the dev environment (D11).
# GStreamer runtime packages join the final stage with multiview (M4).

# ---------- server ----------
FROM ubuntu:24.04 AS server-build
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src

# Dependencies first, in their own layer, so changing our code doesn't rebuild them.
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/crates/nitch-hls/Cargo.toml crates/nitch-hls/
COPY server/crates/nitch-twitch/Cargo.toml crates/nitch-twitch/
COPY server/crates/nitch-server/Cargo.toml crates/nitch-server/
RUN for c in nitch-hls nitch-twitch; do mkdir -p crates/$c/src && touch crates/$c/src/lib.rs; done \
 && mkdir -p crates/nitch-server/src && echo 'fn main() {}' > crates/nitch-server/src/main.rs \
 && cargo build --release --locked \
 && rm -rf crates/*/src

COPY server/crates crates
RUN find crates -name '*.rs' -exec touch {} + && cargo build --release --locked

# ---------- web ----------
FROM node:lts-slim AS web-build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# ---------- runtime ----------
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data --no-create-home nitch \
 && mkdir -p /data && chown nitch /data
COPY --from=server-build /src/target/release/nitch-server /usr/local/bin/nitch-server
COPY --from=web-build /web/dist /app/web
ENV NITCH_BIND=0.0.0.0:8080 \
    NITCH_DATA_DIR=/data \
    NITCH_WEB_DIR=/app/web \
    RUST_LOG=info
USER nitch
VOLUME /data
EXPOSE 8080
# bash, not sh: Ubuntu's sh (dash) has no /dev/tcp.
HEALTHCHECK --interval=30s --timeout=3s CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/${NITCH_BIND##*:}"]
CMD ["nitch-server"]
