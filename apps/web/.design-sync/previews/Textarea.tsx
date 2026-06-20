import * as React from "react";
import { Textarea } from "@hostlet/web";

const wrap: React.CSSProperties = { maxWidth: 420 };

export const Default = () => (
  <div style={wrap}>
    <Textarea placeholder="Paste your docker-compose.yml here…" rows={4} />
  </div>
);

export const WithContent = () => (
  <div style={wrap}>
    <Textarea
      readOnly
      rows={5}
      defaultValue={`services:\n  web:\n    image: nginx:alpine\n    ports:\n      - "80:80"`}
    />
  </div>
);

export const Disabled = () => (
  <div style={wrap}>
    <Textarea defaultValue="Build output is read-only after deployment completes." rows={3} disabled />
  </div>
);
