import * as React from "react";
import { Alert, AlertTitle, AlertDescription } from "@hostlet/web";

export const InContext = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 460 }}>
    <Alert>
      <AlertTitle>Scheduled maintenance</AlertTitle>
      <AlertDescription>
        The runner will be offline on 2026-06-22 from 02:00–04:00 UTC for kernel updates. Deployments will
        resume automatically once maintenance ends.
      </AlertDescription>
    </Alert>
    <Alert variant="danger">
      <AlertTitle>Health check failed</AlertTitle>
      <AlertDescription>
        GET /health returned 503 three times in a row. The container has been restarted. Check your app logs
        for the root cause.
      </AlertDescription>
    </Alert>
  </div>
);
