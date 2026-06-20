import * as React from "react";
import { Metric } from "@hostlet/web";
import { Box, Globe2, HardDrive, ShieldAlert } from "lucide-react";

export const AppCount = () => (
  <div style={{ maxWidth: 200 }}>
    <Metric label="Apps" value="12" detail="8 healthy" icon={Box} />
  </div>
);

export const StorageUsed = () => (
  <div style={{ maxWidth: 200 }}>
    <Metric label="Storage used" value="2.4 GB" detail="of 10 GB quota" icon={HardDrive} />
  </div>
);

export const PublicApps = () => (
  <div style={{ maxWidth: 200 }}>
    <Metric label="Public apps" value="5" detail="Cloudflare DNS open" icon={Globe2} />
  </div>
);

export const LoadingState = () => (
  <div style={{ maxWidth: 200 }}>
    <Metric label="Unhealthy apps" value="" loading={true} icon={ShieldAlert} />
  </div>
);
