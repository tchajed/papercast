# Frontend build stage
FROM oven/bun:1 AS frontend-builder
WORKDIR /frontend
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ .
RUN bun run build

# Backend build stage
FROM rust:1.83-slim AS backend-builder
WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY backend/ .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Litestream for SQLite backup to Tigris
ADD https://github.com/benbjohnson/litestream/releases/download/v0.3.13/litestream-v0.3.13-linux-amd64.tar.gz /tmp/
RUN tar -C /usr/local/bin -xzf /tmp/litestream-v0.3.13-linux-amd64.tar.gz && rm /tmp/litestream-*.tar.gz

COPY --from=backend-builder /app/target/release/tts-podcast-backend /usr/local/bin/backend
COPY --from=frontend-builder /frontend/build /app/static
COPY litestream.yml /etc/litestream.yml
COPY start.sh /start.sh
RUN chmod +x /start.sh

# Create data directory for SQLite
RUN mkdir -p /data

ENV STATIC_DIR=/app/static

CMD ["/start.sh"]
