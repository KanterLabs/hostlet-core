"use client";

import { RefreshCw } from "lucide-react";
import { AppShell, PageHeader } from "@/components/ui";
import { WebhookNotice } from "@/components/WebhookNotice";
import { useSettingsData } from "./settings-data";
import {
  AccessSummarySection,
  BuildFleetSection,
  ConnectionsSection,
  OperationsSection,
  ProductTourSection,
  UpdatesSection,
} from "./settings-sections";

export default function Settings() {
  const {
    github,
    cloudflare,
    version,
    jobs,
    audit,
    cleanup,
    backup,
    buildPools,
    builders,
    registry,
    builderMessage,
    updateMessage,
    operationsMessage,
    busy,
    refresh,
    checkForUpdates,
    runCleanup,
    retryJob,
    cancelJob,
    createVmPool,
    enrollBuilder,
    setDefaultPool,
  } = useSettingsData();

  return (
    <AppShell>
      <PageHeader
        eyebrow="Control plane"
        title="Settings"
        description="Connection status for GitHub auth, webhooks, Cloudflare DNS, and public app routing."
        actions={<button className="button-secondary" onClick={refresh}><RefreshCw size={16} />Refresh</button>}
      />

      <WebhookNotice className="mb-6" />

      <ConnectionsSection github={github} cloudflare={cloudflare} />

      <BuildFleetSection
        pools={buildPools}
        builders={builders}
        registry={registry}
        message={builderMessage}
        onCreateVmPool={createVmPool}
        onEnrollBuilder={enrollBuilder}
        onSetDefaultPool={setDefaultPool}
      />

      <UpdatesSection
        version={version}
        backup={backup}
        message={updateMessage}
        busy={busy}
        onCheckForUpdates={checkForUpdates}
      />

      <AccessSummarySection />

      <OperationsSection
        cleanup={cleanup}
        jobs={jobs}
        audit={audit}
        message={operationsMessage}
        busy={busy}
        onRunCleanup={runCleanup}
        onRetryJob={retryJob}
        onCancelJob={cancelJob}
      />

      <ProductTourSection />
    </AppShell>
  );
}
