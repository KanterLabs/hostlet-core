# Pinned 2026-06-16 from docker.io/library/node:22-bookworm-slim via:
#   docker pull node:22-bookworm-slim
#   docker inspect node:22-bookworm-slim --format '{{index .RepoDigests 0}}'
FROM node:22-bookworm-slim@sha256:e21fc383b50d5347dc7a9f1cae45b8f4e2f0d39f7ade28e4eef7d2934522b752

WORKDIR /app
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium \
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN groupadd -r pwuser && useradd -r -g pwuser -m pwuser \
  && apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates chromium fonts-liberation \
  && rm -rf /var/lib/apt/lists/* \
  && npm init -y \
  && npm install --omit=dev playwright-core@1.60.0 \
  && npm cache clean --force \
  && chown -R pwuser:pwuser /app

COPY --chown=pwuser:pwuser apps/screenshotter/capture.js /app/capture.js

USER pwuser

ENTRYPOINT ["node", "/app/capture.js"]
