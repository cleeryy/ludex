# syntax=docker/dockerfile:1

# ---- Web UI ---------------------------------------------------------------
FROM node:22-bookworm-slim AS web-builder

ENV PNPM_HOME=/pnpm
ENV PATH=$PNPM_HOME:$PATH

RUN corepack enable \
 && corepack prepare pnpm@9.15.0 --activate \
 || npm install -g pnpm@9.15.0

WORKDIR /build

# Manifests first so dependency installation is cached independently of sources.
# The `*` in Cargo.lock*/pnpm-lock.yam[l] is optional: these files may not exist
# yet, and at least one source (package.json/Cargo.toml) always matches.
COPY package.json pnpm-workspace.yaml pnpm-lock.yam[l] ./
COPY apps/web/package.json apps/web/package.json
COPY apps/desktop/package.json apps/desktop/package.json

RUN if [ -f pnpm-lock.yaml ]; then \
      pnpm install --frozen-lockfile --filter @ludex/web...; \
    else \
      pnpm install --no-frozen-lockfile --filter @ludex/web...; \
    fi

COPY apps/web ./apps/web
RUN pnpm --filter @ludex/web build

# ---- Rust server ----------------------------------------------------------
FROM rust:1.99-bookworm AS rust-builder

WORKDIR /build

# Dependency layer: manifests + dummy sources, so `cargo build` compiles the
# whole dependency graph before the real code is copied in.
COPY Cargo.toml Cargo.lock* ./
COPY crates/ludex-core/Cargo.toml crates/ludex-core/Cargo.toml
COPY crates/ludex-server/Cargo.toml crates/ludex-server/Cargo.toml

RUN mkdir -p crates/ludex-core/src crates/ludex-server/src \
 && printf '// dependency cache placeholder\n' > crates/ludex-core/src/lib.rs \
 && printf 'fn main() {}\n' > crates/ludex-server/src/main.rs \
 && printf '// dependency cache placeholder\n' > crates/ludex-server/src/lib.rs

RUN if [ -f Cargo.lock ]; then \
      cargo build --release --locked -p ludex-server; \
    else \
      cargo build --release -p ludex-server; \
    fi

COPY crates ./crates
COPY --from=web-builder /build/apps/web/dist ./apps/web/dist

RUN if [ -f Cargo.lock ]; then \
      cargo build --release --locked -p ludex-server --features embed-web; \
    else \
      cargo build --release -p ludex-server --features embed-web; \
    fi

# ---- Runtime --------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="Ludex" \
      org.opencontainers.image.description="Self-hosted game library server" \
      org.opencontainers.image.source="https://github.com/cleeryy/ludex" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      tini \
      libstdc++6 \
      zlib1g \
 && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 1000 ludex \
 && useradd --uid 1000 --gid 1000 --home-dir /data --no-create-home \
      --shell /usr/sbin/nologin ludex \
 && mkdir -p /data /games /media /app \
 && chown -R 1000:1000 /data /games /media /app

COPY --from=rust-builder /build/target/release/ludex-server /usr/local/bin/ludex-server
RUN chmod 0755 /usr/local/bin/ludex-server

ENV LUDEX_BIND=0.0.0.0:8080 \
    LUDEX_DATA_DIR=/data \
    LUDEX_GAMES_DIR=/games \
    LUDEX_MEDIA_DIR=/media \
    RUST_LOG=ludex_server=info,tower_http=warn

WORKDIR /app
USER 1000:1000

VOLUME ["/data", "/games", "/media"]
EXPOSE 8080
STOPSIGNAL SIGTERM

# Override this if you change LUDEX_BIND's port.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ludex-server"]
