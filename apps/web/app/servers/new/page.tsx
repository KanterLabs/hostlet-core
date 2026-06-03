"use client";

import Link from "next/link";
import { HardDrive } from "lucide-react";
import { AppShell, EmptyState, PageHeader } from "@/components/ui";

export default function AddServer() {
  return (
    <AppShell maxWidth="max-w-4xl">
      <PageHeader
        eyebrow="Remote agents"
        title="VPS support is deferred"
        description="Hostlet deploys apps on the same machine that runs the UI and API."
        actions={<Link className="button-secondary" href="/servers"><HardDrive size={16} />Back to machine</Link>}
      />

      <EmptyState
        icon={HardDrive}
        title="Use this machine for deployments"
        description="Remote VPS registration is intentionally disabled for this release while the local deploy path is hardened."
        actionHref="/apps/new"
        actionLabel="Create app"
      />
    </AppShell>
  );
}
