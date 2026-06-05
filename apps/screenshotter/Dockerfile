FROM mcr.microsoft.com/playwright:v1.60.0-noble

WORKDIR /app
COPY apps/screenshotter/capture.js /app/capture.js

ENTRYPOINT ["node", "/app/capture.js"]
