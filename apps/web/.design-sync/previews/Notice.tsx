import * as React from "react";
import { Notice } from "@hostlet/web";

export const Tones = () => (
  <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 460 }}>
    <Notice
      tone="neutral"
      title="Deployment queued"
      description="Your app is queued behind 2 other builds. Estimated start in 45 seconds."
    />
    <Notice
      tone="success"
      title="Deploy complete"
      description="api-gateway v0.3.7 is live at api.hostlet.cloud. Build took 1m 12s."
    />
    <Notice
      tone="warning"
      title="Storage at 87%"
      description="You've used 8.7 GB of your 10 GB limit. Free up space or upgrade your plan."
      action={<a className="button-secondary" href="#">Upgrade plan</a>}
    />
    <Notice
      tone="danger"
      title="Build failed"
      description="Step 3/6 — npm install exited with code 1. Check the build logs for details."
      action={<a className="button-secondary" href="#">View logs</a>}
    />
  </div>
);
