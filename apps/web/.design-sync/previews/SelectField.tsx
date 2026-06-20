import * as React from "react";
import { SelectField } from "@hostlet/web";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 360 };

export const Default = () => {
  const [region, setRegion] = React.useState("us-east-1");
  return (
    <div style={col}>
      <SelectField label="Deploy region" value={region} onChange={setRegion}>
        <option value="us-east-1">US East (N. Virginia)</option>
        <option value="eu-west-1">EU West (Ireland)</option>
        <option value="ap-southeast-1">AP Southeast (Singapore)</option>
      </SelectField>
    </div>
  );
};

export const Disabled = () => (
  <div style={col}>
    <SelectField label="Deploy region" value="us-east-1" onChange={() => {}} disabled>
      <option value="us-east-1">US East (N. Virginia)</option>
      <option value="eu-west-1">EU West (Ireland)</option>
    </SelectField>
  </div>
);
