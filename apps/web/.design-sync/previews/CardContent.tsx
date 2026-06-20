import * as React from "react";
import { Card, CardHeader, CardTitle, CardContent } from "@hostlet/web";

export const InContext = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>Build logs</CardTitle>
    </CardHeader>
    <CardContent>
      <pre style={{ fontSize: 12, color: "var(--muted)", margin: 0, whiteSpace: "pre-wrap", lineHeight: 1.6 }}>
        {`Step 1/6 — Cloning repository … done
Step 2/6 — Installing dependencies … done
Step 3/6 — Running build (npm run build) …
> next build
info  - Compiled successfully in 34s
Step 4/6 — Building Docker image … done`}
      </pre>
    </CardContent>
  </Card>
);
