import * as React from "react";
import { Panel, PanelHeader } from "@hostlet/web";
import { Box } from "lucide-react";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 480 };
const body: React.CSSProperties = { padding: "12px 16px", fontSize: 14, color: "var(--muted)" };

export const WithAction = () => (
  <div style={col}>
    <Panel padded={false} className="overflow-hidden">
      <PanelHeader
        icon={Box}
        title="Recent apps"
        description="Latest deployment state by project."
        action={<a href="#" className="button-secondary" style={{ fontSize: 13 }}>View all</a>}
      />
      <div style={body}>4 apps running across 2 servers.</div>
    </Panel>
  </div>
);

export const TitleOnly = () => (
  <div style={{ maxWidth: 480 }}>
    <Panel padded={false} className="overflow-hidden">
      <PanelHeader title="Build log" />
      <div style={body}>
        <code style={{ fontSize: 12, fontFamily: "var(--font-mono)" }}>
          [12:04:07] Building image ghcr.io/acme/api:a3f9c12…
        </code>
      </div>
    </Panel>
  </div>
);
