FROM node:24-bookworm AS build
WORKDIR /app
COPY --from=oven/bun:1.3.0 /usr/local/bin/bun /usr/local/bin/bun
COPY package.json bun.lock ./
COPY patches ./patches
RUN bun install --frozen-lockfile --ignore-scripts
COPY index.html vite.config.ts tsconfig*.json ./
COPY src ./src
COPY shared ./shared
COPY public ./public
COPY scripts/precompress-assets.mjs ./scripts/precompress-assets.mjs
RUN npm exec -- vite build && node scripts/precompress-assets.mjs

FROM rust:1.93-bookworm AS rust-build
WORKDIR /app/rust-sync
COPY rust-common /app/rust-common
COPY rust-sync/Cargo.toml rust-sync/Cargo.lock ./
COPY rust-sync/src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
WORKDIR /app
ENV NODE_ENV=production ASSET_DIR=/app/dist PORT=3000
COPY --from=build /app/dist ./dist
COPY --from=rust-build /app/rust-sync/target/release/wartaq-workspace-relay /usr/local/bin/wartaq-web
USER 65532:65532
CMD ["wartaq-web"]
