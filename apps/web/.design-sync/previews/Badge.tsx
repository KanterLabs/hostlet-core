import * as React from "react";
import { Badge } from "@hostlet/web";

export const Variants = () => (
  <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
    <Badge variant="neutral">v0.2.12</Badge>
    <Badge variant="success">running</Badge>
    <Badge variant="warning">building</Badge>
    <Badge variant="danger">failed</Badge>
    <Badge variant="outline">us-east-1</Badge>
  </div>
);
