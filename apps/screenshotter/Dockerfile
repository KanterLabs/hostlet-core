FROM node:22-bookworm-slim

WORKDIR /app
ENV PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium \
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates chromium fonts-liberation \
  && rm -rf /var/lib/apt/lists/* \
  && npm init -y \
  && npm install --omit=dev playwright-core@1.60.0 \
  && npm cache clean --force

COPY apps/screenshotter/capture.js /app/capture.js

ENTRYPOINT ["node", "/app/capture.js"]
