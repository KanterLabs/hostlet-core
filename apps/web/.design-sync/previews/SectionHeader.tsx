import * as React from "react";
import { Panel, SectionHeader } from "@hostlet/web";
import { ScrollText, Tag } from "lucide-react";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 520 };

export const WithAll = () => (
  <div style={col}>
    <Panel>
      <SectionHeader
        icon={ScrollText}
        title="Deployment log"
        description="Streaming output from the last build run."
        action={<button className="button-secondary" style={{ fontSize: 13 }}>Download</button>}
      />
      <div style={{ fontSize: 13, fontFamily: "var(--font-mono)", color: "var(--muted)", lineHeight: 1.6 }}>
        <div>Step 1/6 — Pulling base image node:20-alpine</div>
        <div>Step 2/6 — Installing dependencies (pnpm install)</div>
        <div>Step 3/6 — Building app (pnpm build)</div>
        <div style={{ color: "var(--action)" }}>Step 4/6 — Build complete in 38 s</div>
      </div>
    </Panel>
  </div>
);

export const TitleOnly = () => (
  <div style={{ maxWidth: 520 }}>
    <Panel>
      <SectionHeader icon={Tag} title="Release state" />
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {[
          { label: "Core version", value: "v0.2.12" },
          { label: "Cloud version", value: "v0.1.11" },
          { label: "Last deployed", value: "2026-06-17 14:22 UTC" },
        ].map(({ label, value }) => (
          <div key={label} style={{ display: "flex", justifyContent: "space-between", fontSize: 14, padding: "6px 0", borderBottom: "1px solid var(--line)" }}>
            <span style={{ color: "var(--muted)" }}>{label}</span>
            <span style={{ fontWeight: 500 }}>{value}</span>
          </div>
        ))}
      </div>
    </Panel>
  </div>
);
