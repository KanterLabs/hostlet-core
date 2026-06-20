# design-sync notes — hostlet-core/apps/web

Repo-specific gotchas for `/design-sync` (package shape). Read before any re-sync.

## Source shape
- `apps/web` is a **Next.js app**, not a packaged component library — there is no `dist/`. The
  converter runs in **synth-entry mode** (bundles directly from `components/ui/*.tsx`).
- `srcDir = components/ui` scopes the import to the design-system primitives. The top-level
  feature components (`AuthGate`, `GitHubDeviceFlow`, `GitHubStatus`, `Nav`, `WebhookNotice`)
  are app features, not DS primitives — deliberately out of scope.
- `@/*` path alias resolves via `cfg.tsconfig = tsconfig.json` (baseUrl `.`, `@/*` → `./*`).
  Components import `cn` from `@/lib/utils` and `cx` from `@/components/ui/cx` — esbuild follows
  these across the package even though they live outside `srcDir`.

## Styling — Tailwind must be compiled first (load-bearing)
- `app/globals.css` is Tailwind **source** (`@tailwind base/components/utilities`, `@apply`,
  `@layer components` for `.button`/`.panel`/`.pill`/`.eyebrow`…). It is NOT usable as `cssEntry`
  raw.
- **`cfg.buildCmd` compiles it** to `.design-sync/compiled.css` (run from the package dir
  `apps/web`): `node_modules/.bin/tailwindcss -c tailwind.config.ts -i app/globals.css -o .design-sync/compiled.css`.
  The config's `content` already scans `app/**`+`components/**`, so every utility + component
  class the bundled primitives use is emitted, plus the `:root` brand tokens and shadcn HSL layer.
- `cfg.cssEntry = .design-sync/compiled.css` → the converter appends it to `_ds_bundle.css`,
  which `styles.css` imports. **Always re-run buildCmd before the converter on re-sync.**

## Fonts
- Core `globals.css` uses a **system sans stack** for `--font-sans` (Exo 2.0 is loaded only by the
  hostlet-cloud overlay via `next/font/local`, not by core). `--font-mono` is "Geist Mono".
- `cfg.runtimeFontPrefixes = ["Geist Mono", "Geist"]` suppresses `[FONT_MISSING]` for the mono
  family (it's a host-app font, not shipped by core). System sans needs no `@font-face`.
- If the DS should carry the real brand font (Exo 2.0), add it via `cfg.extraFonts` pointing at
  the cloud overlay's Exo `.otf` files — a deliberate addition, not a faithful core import.

## Synth-entry / self-package wiring (load-bearing)
- The converter resolves `PKG_DIR = node_modules/<pkg>` and crashes (ENOENT) in the
  package's own repo. Fix: a symlink `node_modules/@hostlet/web -> ../..` makes `PKG_DIR`
  resolve and keeps synth-entry discovery (`deriveComponentsFromSrc`) working. The symlink is
  gitignored-by-effect (it's under node_modules); **recreate it on a fresh clone**:
  `mkdir -p node_modules/@hostlet && ln -sfn ../.. node_modules/@hostlet/web`.
- The `process` shim (`cfg.extraEntries: ["./.design-sync/process-shim.mjs"]`) is REQUIRED:
  the bundle references `process.env.*`/`process.nextTick`; without the shim the IIFE throws
  `ReferenceError: process is not defined` and `window.HostletUI` is never assigned (every
  component then fails `[BUNDLE_EXPORT]`).

## Render check / playwright
- playwright isn't installed in the repo. Install `playwright@1.60.0` into `.ds-sync`
  (`PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --prefix .ds-sync playwright@1.60.0`) — 1.60.0
  pins chromium-1223, which is already in `~/.cache/ms-playwright`. validate imports playwright
  from `.ds-sync/node_modules`.

## Authoring patterns (for preview .tsx)
- Import components from the package name: `import { Button } from "@hostlet/web"` (mapped to
  `window.HostletUI`); also `import * as React from "react"`.
- Use **inline styles** for preview layout glue (gap/flex/maxWidth). Tailwind utilities used
  only inside preview .tsx are NOT in `compiled.css` (its content scan is app/+components/), so
  invented utility classes won't style. Component brand classes (in the bundle) work fine.
- Components render with brand tokens (emerald action, status tones). `next/link` bundles OK
  (StatusPill/EmptyState use it).
- Solo set authored + graded good: Button, Card, Alert, StatusPill.

## Known render warns
- `[RENDER_THIN] ConfirmDialog ... rendered height is 0px` is BENIGN — ConfirmDialog uses
  `position: fixed`, which contributes 0 to document height, but the modal renders fully
  (confirmed in the screenshot). Do not "fix" it. `cfg.overrides.ConfirmDialog` (cardMode
  single + 560x400 viewport) keeps the fixed overlay inside its card.

## Authoring outcome (first sync 2026-06-20)
- 46 components in scope (AppShell excluded — see below). 44 authored previews, all graded good.
- Intentional FLOOR cards: ConfirmProvider, ToastProvider — pure React context providers with no
  static visual; floor card is the honest representation (still fully importable).
- AppShell EXCLUDED via `cfg.componentSrcMap.AppShell = null`: it renders `<Nav>` →
  `usePathname()` (next/navigation), which throws outside Next App Router — unrenderable in the
  design environment and for the design agent. The `.app-shell`/`.page` classes remain in the
  shipped CSS for building shells.
- Preview authoring patterns (folded from subagent learnings): controlled form components need
  `React.useState` (the harness renders live React); `defaultValue` for simply-filled inputs;
  Lucide icons import from `"lucide-react"` and bundle into the preview; Metric/MetricsGrid take
  a Lucide component (not element) as `icon`; StorageMeter props are in BYTES; LogViewer `lines`
  is `readonly string[]` and it has a built-in min-height; ServiceCard reads `ServiceSummary`
  (role "web"→Globe else Database; healthStatus precedes status); Menu owns its open state so the
  static card shows the closed trigger.
- NOTE: ServiceStack needed its OWN `previews/ServiceStack.tsx` (the domain batch first composed
  it only inside ServiceCard.tsx, leaving ServiceStack a floor card).

## Re-sync risks
- The `node_modules/@hostlet/web` symlink and the `.ds-sync` playwright install are NOT
  committed (under node_modules) — recreate both on a fresh clone (see above).
- `.design-sync/compiled.css` is gitignored/regenerated — always run `cfg.buildCmd` before the
  converter on re-sync, or `cssEntry` points at a stale/missing file.
- Fonts: bundle ships NO brand font (system sans + host-provided mono). If core later adopts
  Exo 2.0 in globals.css, add it via `cfg.extraFonts`.
