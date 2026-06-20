## Building with HostletUI

HostletUI is Hostlet's real React component library (`@hostlet/web`), styled with Tailwind CSS
v3 over a single brand token layer. Build screens by composing these components and styling your
own layout with the **same Tailwind utilities and tokens** the components use — never hand-rolled
hex or ad-hoc CSS.

### Setup
- Link `styles.css` once. It `@import`s the brand tokens, the shadcn HSL layer, and every
  component's compiled CSS (`_ds_bundle.css`). Without it components render unstyled.
- Components live on `window.HostletUI` (the bundle); React must be on the page first.
- Most components are self-contained and need no provider. Exceptions: `useToast()` must be inside
  `<ToastProvider>`, and `useConfirm()` must be inside `<ConfirmProvider>`. `<ConfirmDialog>` can
  also be driven directly via its `open` prop.

### Styling idiom — Tailwind utilities + brand tokens
Style your own layout with Tailwind utilities. The brand is exposed as Tailwind color utilities
backed by CSS variables — use these names:

Brand palette (canonical hex tokens):
- Surfaces: `bg-surface` (white card), `bg-surface-alt`, `bg-panel` (app canvas), `bg-rail` (dark nav rail)
- Text: `text-ink` (primary), `text-muted` (secondary)
- Borders: `border-line`
- Accent (emerald): `bg-action`, `text-action`, hover `bg-action-strong`

shadcn semantic layer (HSL, alpha-enabled):
- `bg-background` / `text-foreground`, `bg-card`, `bg-primary` / `text-primary-foreground`,
  `bg-secondary`, `bg-accent`, `bg-destructive`, `border-border`, `border-input`, `ring-ring`;
  radius via `rounded-lg` / `rounded-md` / `rounded-sm`.

Status tones (used by Badge / Alert / pills):
- success: `bg-success-bg text-success-fg ring-success-border`
- warning: `bg-warning-bg text-warning-fg ring-warning-border`
- danger:  `bg-danger-bg text-danger-fg ring-danger-border`

Component/utility classes from the stylesheet (apply directly to elements):
- Buttons: `.button`, `.button-secondary`, `.button-danger`, plus `.compact` size modifier
- Containers: `.panel`, `.panel-muted`, `.metric`, `.pill`
- Page scaffold: `.page`, `.page-inner`, `.app-shell`
- Type accents: `.eyebrow` (emerald uppercase kicker), `.data-label` (muted uppercase), `.muted`, `.skeleton`

Typography: system sans (`font-sans`); `font-mono` is reserved for commit SHAs, logs, and version chips.

### Where the truth lives
- `styles.css` → `_ds_bundle.css`: every token and class above is defined here — read it before styling.
- `components/<group>/<Name>/<Name>.prompt.md` (usage + examples) and `<Name>.d.ts` (props) per component.

### Idiomatic example
```tsx
<Card>
  <CardHeader>
    <CardTitle>app-prod</CardTitle>
    <CardDescription>Deployed 2m ago · commit a1b2c3d</CardDescription>
  </CardHeader>
  <CardContent>
    <div className="flex items-center gap-2">
      <StatusPill status="running" />
      <span className="text-sm text-muted">us-central1</span>
    </div>
  </CardContent>
  <CardFooter className="gap-2">
    <Button>View app</Button>
    <Button variant="secondary">Logs</Button>
  </CardFooter>
</Card>
```
