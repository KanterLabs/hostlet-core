import * as React from "react";
import { Label, Input } from "@hostlet/web";

export const Default = () => (
  <Label>Custom domain</Label>
);

export const WithInput = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 6, maxWidth: 320 }}>
    <Label htmlFor="domain-preview">Custom domain</Label>
    <Input id="domain-preview" placeholder="api.myapp.com" />
  </div>
);

export const DisabledPair = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 6, maxWidth: 320 }}>
    <Label htmlFor="region-preview">Deploy region</Label>
    <Input id="region-preview" defaultValue="us-east-1" disabled />
  </div>
);
