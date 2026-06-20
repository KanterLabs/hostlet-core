import * as React from "react";
import { Alert, AlertTitle, AlertDescription } from "@hostlet/web";

export const Variants = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 460 }}>
    <Alert>
      <AlertTitle>Heads up</AlertTitle>
      <AlertDescription>Your deployment is queued and will start shortly.</AlertDescription>
    </Alert>
    <Alert variant="success">
      <AlertTitle>Deployed</AlertTitle>
      <AlertDescription>app-prod is live at app.hostlet.cloud.</AlertDescription>
    </Alert>
    <Alert variant="warning">
      <AlertTitle>Action needed</AlertTitle>
      <AlertDescription>Add a payment method to keep your app running.</AlertDescription>
    </Alert>
    <Alert variant="danger">
      <AlertTitle>Build failed</AlertTitle>
      <AlertDescription>Exit code 1 during the install step — check the build logs.</AlertDescription>
    </Alert>
  </div>
);
