import { mkdir, writeFile } from "node:fs/promises";

const version = process.env.APP_VERSION || "v1";
const websocketUrl = process.env.VITE_WS_URL || "missing";
await mkdir("dist", { recursive: true });
await writeFile("dist/index.html", `<!doctype html><h1>patchwork-${version}</h1><p id="ws">${websocketUrl}</p>`);
