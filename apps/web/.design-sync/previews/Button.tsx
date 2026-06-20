import * as React from "react";
import { Button } from "@hostlet/web";

const row: React.CSSProperties = { display: "flex", gap: 12, alignItems: "center", flexWrap: "wrap" };

export const Variants = () => (
  <div style={row}>
    <Button>Deploy</Button>
    <Button variant="secondary">Cancel</Button>
    <Button variant="danger">Delete app</Button>
  </div>
);

export const Sizes = () => (
  <div style={row}>
    <Button>Default</Button>
    <Button size="compact">Compact</Button>
  </div>
);

export const Disabled = () => (
  <div style={row}>
    <Button disabled>Deploying…</Button>
    <Button variant="secondary" disabled>
      Unavailable
    </Button>
  </div>
);
