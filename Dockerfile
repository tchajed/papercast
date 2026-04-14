# Frontend build stage
FROM oven/bun:1 AS frontend-builder
WORKDIR /frontend
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ .
RUN bun run build

# Backend dependency planner
FROM rust:1.94-slim AS chef
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY backend/ .
RUN cargo chef prepare --recipe-path recipe.json

# Backend build stage
FROM chef AS backend-builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY backend/ .
RUN cargo build --release -p tts-podcast-backend

# Runtime stage
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates poppler-utils sqlite3 && rm -rf /var/lib/apt/lists/*

# Litestream for SQLite backup to Tigris
ADD https://github.com/benbjohnson/litestream/releases/download/v0.5.11/litestream-0.5.11-linux-x86_64.tar.gz /tmp/
RUN tar -C /usr/local/bin -xzf /tmp/litestream-*-linux-x86_64.tar.gz && rm /tmp/litestream-*.tar.gz

COPY --from=backend-builder /app/target/release/tts-podcast-backend /usr/local/bin/backend
COPY --from=frontend-builder /frontend/build /app/static
COPY litestream.yml /etc/litestream.yml
COPY start.sh /start.sh
RUN chmod +x /start.sh

# Create data directory for SQLite
RUN mkdir -p /data

ENV STATIC_DIR=/app/static

CMD ["/start.sh"]
