FROM oven/bun:1.3.0 AS build
WORKDIR /app
COPY package.json bun.lock ./
COPY patches ./patches
RUN HUSKY=0 bun install --frozen-lockfile --ignore-scripts
COPY index.html vite.config.ts tsconfig*.json ./
COPY src ./src
COPY shared ./shared
COPY public ./public
COPY scripts/precompress-assets.mjs ./scripts/precompress-assets.mjs
RUN bunx vite build && bun scripts/precompress-assets.mjs

FROM rust:1.93-bookworm AS rust-build
WORKDIR /app
COPY rust-sync/Cargo.toml rust-sync/Cargo.lock ./
COPY rust-sync/src ./src
RUN cargo build --release --locked

FROM oven/bun:1.3.0
WORKDIR /app
ENV NODE_ENV=production WARTAQ_WEB_RUNTIME=hybrid ASSET_DIR=/app/dist PORT=3000
COPY package.json bun.lock ./
COPY patches ./patches
RUN HUSKY=0 bun install --frozen-lockfile --production --ignore-scripts
COPY --from=build /app/dist ./dist
COPY --from=rust-build /app/target/release/wartaq-workspace-relay /usr/local/bin/wartaq-web
COPY server ./server
COPY functions/notebook-service.ts ./functions/notebook-service.ts
COPY shared ./shared
USER bun
CMD ["wartaq-web"]
