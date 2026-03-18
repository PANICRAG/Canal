# ── Builder ──
FROM rust:1.80-slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

RUN cargo build --release -p gateway-core

# ── Runtime ──
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/config ./config
COPY --from=builder /app/plugins ./plugins
COPY --from=builder /app/plugin-bundles ./plugin-bundles
COPY --from=builder /app/.env.example ./.env.example

EXPOSE 4000

ENV RUST_LOG=info
ENV HOST=0.0.0.0
ENV PORT=4000

# NOTE: gateway-api binary requires external auth crates to build.
# This Dockerfile builds the core library only.
# To run the full server, provide your own binary that imports gateway-core.
CMD ["echo", "Canal Engine core built successfully. Provide your own server binary."]
