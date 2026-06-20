import * as React from "react";
import { IconFrame } from "@hostlet/web";
import { Server, Box, HardDrive, ScrollText, Settings, TerminalSquare } from "lucide-react";

const row: React.CSSProperties = { display: "flex", gap: 12, alignItems: "center", flexWrap: "wrap" };

export const Various = () => (
  <div style={row}>
    <IconFrame icon={Server} />
    <IconFrame icon={Box} />
    <IconFrame icon={HardDrive} />
    <IconFrame icon={ScrollText} />
    <IconFrame icon={Settings} />
  </div>
);

const settingItem: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 12,
  padding: "10px 0",
  borderBottom: "1px solid var(--line)",
};

export const WithLabels = () => (
  <div style={{ maxWidth: 360 }}>
    <div style={settingItem}>
      <IconFrame icon={Server} />
      <div>
        <div style={{ fontWeight: 600, fontSize: 14 }}>prod-us-east-1</div>
        <div style={{ fontSize: 12, color: "var(--muted)" }}>Ubuntu 22.04 · 4 vCPU · 8 GB RAM</div>
      </div>
    </div>
    <div style={settingItem}>
      <IconFrame icon={HardDrive} />
      <div>
        <div style={{ fontWeight: 600, fontSize: 14 }}>Storage volume</div>
        <div style={{ fontSize: 12, color: "var(--muted)" }}>48 GB · /var/lib/hostlet</div>
      </div>
    </div>
    <div style={{ ...settingItem, borderBottom: "none" }}>
      <IconFrame icon={Settings} />
      <div>
        <div style={{ fontWeight: 600, fontSize: 14 }}>Environment</div>
        <div style={{ fontSize: 12, color: "var(--muted)" }}>8 variables configured</div>
      </div>
    </div>
  </div>
);
