import * as React from "react";
import { Input } from "@hostlet/web";

const field: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 4, maxWidth: 320 };

export const Default = () => (
  <div style={field}>
    <Input placeholder="api.myapp.com" />
  </div>
);

export const WithValue = () => (
  <div style={field}>
    <Input defaultValue="postgres.internal:5432" />
  </div>
);

export const Disabled = () => (
  <div style={field}>
    <Input defaultValue="redis-stg.hostlet.cloud" disabled />
  </div>
);
