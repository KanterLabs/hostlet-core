import * as React from "react";
import { ConfirmDialog } from "@hostlet/web";

const noop = () => {};

export const Destructive = () => (
  <ConfirmDialog
    open
    title="Delete app-prod?"
    description="This permanently removes the deployment, its volumes, and DNS records. This action cannot be undone."
    confirmLabel="Delete app"
    destructive
    onConfirm={noop}
    onCancel={noop}
  />
);

export const Neutral = () => (
  <ConfirmDialog
    open
    title="Restart app-prod?"
    description="The app will be briefly unavailable while it restarts."
    confirmLabel="Restart"
    onConfirm={noop}
    onCancel={noop}
  />
);
