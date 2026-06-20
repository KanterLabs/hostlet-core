import * as React from "react";
import { CopyButton } from "@hostlet/web";

export const Examples = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
    <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
      <code style={{ fontSize: 13, color: "var(--muted)", fontFamily: "monospace" }}>
        https://api.hostlet.cloud/v1
      </code>
      <CopyButton value="https://api.hostlet.cloud/v1" label="Copy URL" />
    </div>
    <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
      <code style={{ fontSize: 13, color: "var(--muted)", fontFamily: "monospace" }}>
        a1b2c3d
      </code>
      <CopyButton value="a1b2c3d4e5f6g7h8" label="Copy SHA" copiedLabel="Copied!" />
    </div>
    <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
      <code style={{ fontSize: 13, color: "var(--muted)", fontFamily: "monospace" }}>
        sk-live-kYtP…q9Xz
      </code>
      <CopyButton value="sk-live-kYtPq9Xz" label="Copy key" />
    </div>
  </div>
);
