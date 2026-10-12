# builds the vite spa (the same web bundle tauri wraps) and ships it inside caddy.
# the Caddyfile is rendered by ansible and mounted by compose, see deploy/compose.
FROM node:22-alpine AS build
WORKDIR /app
RUN corepack enable pnpm
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
# served from the same origin as the api, so requests go to /v1/... relative
ARG VITE_API_URL=
ENV VITE_API_URL=$VITE_API_URL
RUN pnpm build

FROM caddy:2-alpine
COPY --from=build /app/dist /srv
