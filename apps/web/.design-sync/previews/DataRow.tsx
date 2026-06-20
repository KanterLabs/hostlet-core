import * as React from "react";
import { DataRow } from "@hostlet/web";

export const Basic = () => (
  <div style={{ maxWidth: 420, padding: 16, display: "flex", flexDirection: "column", gap: 8 }}>
    <DataRow label="Region" value="us-central1" />
    <DataRow label="Commit" value={<span style={{ fontFamily: "monospace", fontSize: 12 }}>a1b2c3d</span>} />
    <DataRow label="Branch" value="main" />
  </div>
);

export const Loading = () => (
  <div style={{ maxWidth: 420, padding: 16, display: "flex", flexDirection: "column", gap: 8 }}>
    <DataRow label="Version" value="" loading={true} />
    <DataRow label="Last checked" value="" loading={true} />
  </div>
);
