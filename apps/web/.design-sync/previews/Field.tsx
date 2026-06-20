import * as React from "react";
import { Field } from "@hostlet/web";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 16, maxWidth: 360 };

export const Default = () => {
  const [name, setName] = React.useState("my-postgres");
  return (
    <div style={col}>
      <Field label="Application name" value={name} onChange={setName} placeholder="my-app" />
    </div>
  );
};

export const EmailType = () => {
  const [email, setEmail] = React.useState("ops@acme.com");
  return (
    <div style={col}>
      <Field label="Alert email" type="email" value={email} onChange={setEmail} placeholder="you@example.com" />
    </div>
  );
};

export const WithPlaceholder = () => {
  const [val, setVal] = React.useState("");
  return (
    <div style={col}>
      <Field label="GitHub repository" value={val} onChange={setVal} placeholder="acme/my-app" />
    </div>
  );
};
