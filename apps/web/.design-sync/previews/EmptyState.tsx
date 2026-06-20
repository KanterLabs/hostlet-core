import * as React from "react";
import { Server, GitBranch } from "lucide-react";
import { EmptyState } from "@hostlet/web";

export const NoApps = () => (
  <div style={{ maxWidth: 560 }}>
    <EmptyState
      icon={Server}
      title="No apps yet"
      description="Deploy your first app from a GitHub repo, a Docker image, or a Compose file. It only takes a minute."
      actionHref="/apps/new"
      actionLabel="Deploy an app"
    />
  </div>
);

export const NoDeployments = () => (
  <div style={{ maxWidth: 560 }}>
    <EmptyState
      icon={GitBranch}
      title="No deployments"
      description="Push to your connected branch or trigger a manual deploy to see builds here."
      actionHref="/apps/notes-app/deploy"
      actionLabel="Deploy now"
    />
  </div>
);
