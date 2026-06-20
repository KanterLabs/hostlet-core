// Minimal `process` global for the browser IIFE bundle.
//
// The Hostlet web components are bundled from Next.js app source (synth-entry
// mode). Parts of the module graph reference `process.env.*` (read at module
// load) and `process.nextTick` (from a dependency). In a browser IIFE none of
// those resolve, so the bundle throws `ReferenceError: process is not defined`
// before `window.HostletUI` is ever assigned — taking every component with it.
//
// Loaded first via cfg.extraEntries so this side effect runs before React and
// the components initialize. Env reads then return undefined and fall back to
// the components' own defaults; no app config leaks into the design bundle.
const g = globalThis;
if (typeof g.process === "undefined") {
  g.process = { env: {}, nextTick: (fn, ...args) => queueMicrotask(() => fn(...args)) };
} else if (!g.process.env) {
  g.process.env = {};
}
