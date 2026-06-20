import * as React from "react";
import { SecretField } from "@hostlet/web";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 420 };

export const Default = () => {
  const [token, setToken] = React.useState("sk-hostlet-7f3a2b91c4d8e6f0a1b2c3d4");
  return (
    <div style={col}>
      <SecretField label="API token" value={token} onChange={setToken} placeholder="sk-hostlet-…" />
    </div>
  );
};

export const DatabasePassword = () => {
  const [pass, setPass] = React.useState("Pg!xK9#mRv$2qLwT");
  return (
    <div style={col}>
      <SecretField label="Database password" value={pass} onChange={setPass} placeholder="Enter password…" />
    </div>
  );
};
