import * as React from "react";
import { PageHeader } from "@hostlet/web";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 24, maxWidth: 640 };

export const WithActions = () => (
  <div style={col}>
    <PageHeader
      eyebrow="Apps / acme-api"
      title="acme-api"
      description="Node.js REST API · prod-us-east-1 · Last deployed 3 min ago from commit a3f9c12"
      actions={
        <>
          <button className="button-secondary">Logs</button>
          <button className="button">Redeploy</button>
        </>
      }
    />
  </div>
);

export const Minimal = () => (
  <div style={{ maxWidth: 640 }}>
    <PageHeader
      title="Servers"
      description="Machines registered to your Hostlet control plane."
      actions={<button className="button">Add server</button>}
    />
  </div>
);
