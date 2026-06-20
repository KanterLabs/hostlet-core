import * as React from "react";
import { Metric, MetricsGrid } from "@hostlet/web";
import { Box, Globe2, HardDrive, Rocket, ShieldAlert } from "lucide-react";

export const DashboardOverview = () => (
  <div style={{ maxWidth: 900 }}>
    <MetricsGrid columns="lg:grid-cols-5">
      <Metric label="Apps" value="12" detail="8 healthy" icon={Box} />
      <Metric label="Active deploys" value="2" detail="builds, checks, routing" icon={Rocket} />
      <Metric label="Unhealthy apps" value="1" detail="runtime monitor" icon={ShieldAlert} />
      <Metric label="Public apps" value="5" detail="Cloudflare DNS open" icon={Globe2} />
      <Metric label="Machines online" value="1/1" detail="agent heartbeat" icon={HardDrive} />
    </MetricsGrid>
  </div>
);

export const HealthPanel = () => (
  <div style={{ maxWidth: 600 }}>
    <MetricsGrid columns="md:grid-cols-3" className="mb-0 gap-3">
      <Metric label="Status" value="healthy" detail="latest agent check" />
      <Metric label="HTTP" value="200" detail="38 ms" />
      <Metric label="Failures" value="0" detail="checked 14:02 UTC" />
    </MetricsGrid>
  </div>
);
