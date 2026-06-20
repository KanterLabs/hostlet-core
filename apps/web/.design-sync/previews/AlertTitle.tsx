import * as React from "react";
import { Alert, AlertTitle, AlertDescription } from "@hostlet/web";

export const InContext = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 460 }}>
    <Alert variant="warning">
      <AlertTitle>Disk space low</AlertTitle>
      <AlertDescription>You have 1.1 GB remaining on the runner. Remove old images to free space.</AlertDescription>
    </Alert>
    <Alert variant="danger">
      <AlertTitle>Deploy rejected</AlertTitle>
      <AlertDescription>Storage quota exceeded — upgrade your plan or remove unused volumes.</AlertDescription>
    </Alert>
    <Alert variant="success">
      <AlertTitle>SSL certificate issued</AlertTitle>
      <AlertDescription>TLS is active for notes.hostlet.cloud. Certificate expires in 89 days.</AlertDescription>
    </Alert>
  </div>
);
