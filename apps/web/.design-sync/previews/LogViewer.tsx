import * as React from "react";
import { LogViewer } from "@hostlet/web";

const successLines = [
  "$ git clone https://github.com/acme/api.git /build/src",
  "Cloning into '/build/src'...",
  "HEAD is now at a3f9c12 feat: add /health endpoint",
  "$ docker build -t ghcr.io/acme/api:a3f9c12 .",
  "Step 1/9 : FROM node:20-alpine AS deps",
  "Step 2/9 : WORKDIR /app",
  "Step 3/9 : COPY package*.json ./",
  "Step 4/9 : RUN npm ci --omit=dev",
  "npm warn deprecated inflight@1.0.6",
  "added 312 packages in 14.2s",
  "Step 5/9 : COPY . .",
  "Step 6/9 : RUN npm run build",
  "> api@1.4.2 build",
  "> tsc -p tsconfig.build.json && node esbuild.mjs",
  "Build complete. Output: dist/ (2.1 MB)",
  "Step 7/9 : FROM node:20-alpine AS runtime",
  "Step 8/9 : COPY --from=deps /app/dist ./dist",
  "Step 9/9 : CMD [\"node\", \"dist/main.js\"]",
  "Successfully built d7e821fa4c09",
  "Successfully tagged ghcr.io/acme/api:a3f9c12",
  "$ docker push ghcr.io/acme/api:a3f9c12",
  "Pushed digest: sha256:8c3f9a2d...",
  "Pulling image on host: runner-eu-west-1...",
  "Starting container: acme-api-1",
  "Health check passed at :8080/health",
  "Deployed successfully. Live at https://api.acme-app.hostlet.app",
];

const failedLines = [
  "$ git clone https://github.com/acme/dashboard.git /build/src",
  "Cloning into '/build/src'...",
  "HEAD is now at b91d4e8 fix: resolve import cycles",
  "$ docker build -t ghcr.io/acme/dashboard:b91d4e8 .",
  "Step 1/7 : FROM node:20-alpine AS deps",
  "Step 2/7 : WORKDIR /app",
  "Step 3/7 : COPY package*.json ./",
  "Step 4/7 : RUN npm ci",
  "added 1204 packages in 31s",
  "Step 5/7 : COPY . .",
  "Step 6/7 : RUN npm run build",
  "> dashboard@2.0.1 build",
  "> next build",
  "Error: Cannot find module '@/lib/auth'",
  "    at Object.<anonymous> (/app/src/app/layout.tsx:3:1)",
  "Failed to compile. See above for details.",
  "The command 'npm run build' returned a non-zero code: 1",
  "Build failed.",
];

export const SuccessfulDeploy = () => (
  <div style={{ maxWidth: 680, height: 340 }}>
    <LogViewer lines={successLines} />
  </div>
);

export const FailedBuild = () => (
  <div style={{ maxWidth: 680, height: 340 }}>
    <LogViewer lines={failedLines} highlightFirstError={true} />
  </div>
);
