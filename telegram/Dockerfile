FROM node:24-bookworm AS build
WORKDIR /app
COPY package.json package-lock.json ./
COPY patches ./patches
RUN npm ci --ignore-scripts && npm run postinstall
COPY telegram ./telegram
COPY shared ./shared
COPY server/profile.ts server/http.ts ./server/
RUN npm exec -- esbuild telegram/runtime.ts --bundle --platform=node --target=node24 --format=esm \
    --packages=external --outfile=/app/bot.mjs

FROM node:24-bookworm
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY --from=build /app/bot.mjs ./bot.mjs
ENV NODE_ENV=production
CMD ["node", "/app/bot.mjs"]
