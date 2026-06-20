import * as React from "react";
import { DataList, DataRow } from "@hostlet/web";

export const VersionInfo = () => (
  <div style={{ maxWidth: 480, padding: 16 }}>
    <DataList>
      <DataRow label="Current version" value={<span style={{ fontFamily: "monospace", fontSize: 12 }}>v0.2.12</span>} />
      <DataRow label="Latest version" value={<span style={{ fontFamily: "monospace", fontSize: 12 }}>v0.2.12</span>} />
      <DataRow label="Runtime" value="Docker + Caddy" />
      <DataRow label="Default access" value="Private apps" />
      <DataRow label="Last backup" value="2026-06-20 03:00 UTC (scheduled)" />
      <DataRow label="Update command" value={<span style={{ fontFamily: "monospace", fontSize: 12 }}>hostlet update</span>} />
    </DataList>
  </div>
);

export const TwoColumn = () => (
  <div style={{ maxWidth: 640, padding: 16 }}>
    <DataList className="lg:grid-cols-2">
      <DataRow label="Database cleanup" value="7 days · auto" />
      <DataRow label="Docker keep set" value="5 containers, 3 images" />
      <DataRow label="Docker cleanup" value="available" />
      <DataRow label="CI target" value="self-hosted Linux X64" />
    </DataList>
  </div>
);
