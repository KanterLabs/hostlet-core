import * as React from "react";
import { SummaryItem, DataList } from "@hostlet/web";

export const Standalone = () => (
  <div style={{ maxWidth: 280 }}>
    <SummaryItem label="Framework" value="Next.js 14" />
  </div>
);

export const DeploymentMeta = () => (
  <div style={{ maxWidth: 520 }}>
    <DataList className="sm:grid-cols-2 lg:grid-cols-3">
      <SummaryItem label="Packaging" value="Nixpacks" />
      <SummaryItem label="Framework" value="Next.js 14" />
      <SummaryItem label="Build backend" value="docker" />
      <SummaryItem label="Package manager" value="pnpm" />
      <SummaryItem label="Git sync" value="1.2 s" />
      <SummaryItem label="Build time" value="48.3 s" />
      <SummaryItem label="Image size" value="312 MB" />
      <SummaryItem label="Boot time" value="4.1 s" />
      <SummaryItem label="Routing" value="0.8 s" />
    </DataList>
  </div>
);
