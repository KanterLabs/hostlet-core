import * as React from "react";
import { Panel, SectionHeader } from "@hostlet/web";
import { HardDrive } from "lucide-react";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 480 };
const row2: React.CSSProperties = { display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12, marginTop: 12 };
const metricBox: React.CSSProperties = {
  padding: "10px 14px",
  borderRadius: 8,
  background: "var(--surface-alt)",
  border: "1px solid var(--line)",
};

export const WithContent = () => (
  <div style={col}>
    <Panel>
      <SectionHeader icon={HardDrive} title="Node: prod-us-east-1" description="Reporting healthy. Last check 12 s ago." />
      <div style={row2}>
        <div style={metricBox}>
          <div style={{ fontSize: 11, color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.06em" }}>CPU</div>
          <div style={{ fontSize: 22, fontWeight: 700, marginTop: 4 }}>34%</div>
        </div>
        <div style={metricBox}>
          <div style={{ fontSize: 11, color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.06em" }}>Memory</div>
          <div style={{ fontSize: 22, fontWeight: 700, marginTop: 4 }}>2.1 GB</div>
        </div>
        <div style={metricBox}>
          <div style={{ fontSize: 11, color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.06em" }}>Disk</div>
          <div style={{ fontSize: 22, fontWeight: 700, marginTop: 4 }}>48 GB</div>
        </div>
        <div style={metricBox}>
          <div style={{ fontSize: 11, color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.06em" }}>Uptime</div>
          <div style={{ fontSize: 22, fontWeight: 700, marginTop: 4 }}>14 d</div>
        </div>
      </div>
    </Panel>
  </div>
);

export const Muted = () => (
  <div style={{ maxWidth: 480 }}>
    <Panel muted>
      <div style={{ fontSize: 14, color: "var(--muted)" }}>
        No recent deployments. Push to a connected repo or trigger a manual deploy to get started.
      </div>
    </Panel>
  </div>
);

export const Loading = () => (
  <div style={{ maxWidth: 480 }}>
    <Panel loading>
      {/* loading skeleton rendered by Panel itself */}
    </Panel>
  </div>
);
