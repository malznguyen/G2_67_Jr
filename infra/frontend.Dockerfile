# =========================================================
# G2_67_Jr — Frontend (Next.js) multi-stage Dockerfile
#
# Build context is the repository root (`.`), so this file
# references `./frontend` for sources. The `output: "standalone"`
# option in next.config.mjs lets us copy just the standalone
# runtime into a slim runtime image.
#
# Image tag built from this Dockerfile: gmrag/frontend:dev
# Matches `infra/docker-compose.yml` service `frontend`.
# =========================================================

# ---------- Stage 1: deps (pnpm install) ----------
FROM node:22-bookworm-slim AS deps
ENV CI=true \
    HUSKY=0 \
    PNPM_HOME="/pnpm" \
    PATH="/pnpm:$PATH"
RUN corepack enable && corepack prepare pnpm@10.32.1 --activate
WORKDIR /app
COPY frontend/package.json frontend/pnpm-lock.yaml* ./
# If a lockfile is present, do a reproducible install; otherwise generate
# one and install. This keeps the first build working even before the
# developer has run `pnpm install` locally.
COPY frontend ./
RUN if [ -f pnpm-lock.yaml ]; then \
      pnpm install --frozen-lockfile; \
    else \
      pnpm install; \
    fi

# ---------- Stage 2: build ----------
FROM node:22-bookworm-slim AS builder
RUN corepack enable && corepack prepare pnpm@10.32.1 --activate
WORKDIR /app
ENV NEXT_TELEMETRY_DISABLED=1
COPY --from=deps /app /app
RUN pnpm build

# ---------- Stage 3: runtime ----------
FROM node:22-bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends wget tini ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1001 gmrag \
 && useradd  --system --uid 1001 --gid gmrag --create-home gmrag

WORKDIR /app
ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    PORT=3000 \
    HOSTNAME=0.0.0.0

# Copy the standalone server output (controlled by next.config.mjs `output: "standalone"`)
COPY --from=builder --chown=gmrag:gmrag /app/.next/standalone ./
COPY --from=builder --chown=gmrag:gmrag /app/.next/static     ./.next/static
COPY --from=builder --chown=gmrag:gmrag /app/public            ./public

USER gmrag
EXPOSE 3000

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["node", "server.js"]
