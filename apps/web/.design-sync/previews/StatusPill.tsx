import * as React from "react";
import { StatusPill } from "@hostlet/web";

export const Statuses = () => (
  <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
    <StatusPill status="running" />
    <StatusPill status="success" />
    <StatusPill status="failed" />
    <StatusPill status="needs attention" />
    <StatusPill status="not deployed" />
  </div>
);
