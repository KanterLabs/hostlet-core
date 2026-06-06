FROM mcr.microsoft.com/playwright:v1.60.0-noble

WORKDIR /app
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN npm init -y \
  && npm install --omit=dev playwright@1.60.0 \
  && npm cache clean --force

COPY apps/screenshotter/capture.js /app/capture.js

ENTRYPOINT ["node", "/app/capture.js"]
