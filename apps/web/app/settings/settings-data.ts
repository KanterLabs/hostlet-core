"use client";

import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useVisibilityPoll } from "@/lib/useVisibilityPoll";

export type StatusPayload = {
  configured?: boolean;
  oauthConfigured?: boolean;
  webhookConfigured?: boolean;
  tokenValid?: boolean | null;
  authenticated?: boolean;
  login?: string | null;
  baseDomain?: string | null;
  domainPrefix?: string;
  defaultDomainPattern?: string | null;
  tunnelTargetConfigured?: boolean;
  message: string;
};

export type VersionPayload = {
  currentVersion: string;
  updateChecksEnabled: boolean;
  update?: UpdatePayload | null;
};

export type UpdatePayload = {
  latestVersion?: string;
  releaseNotesUrl?: string;
  releasedAt?: string;
  minimumSupportedVersion?: string | null;
  composeMigrations?: boolean;
  databaseMigrations?: boolean;
  updateAvailable?: boolean;
  unsupportedDirectUpdate?: boolean;
  checkedAt?: string;
};

export type AgentJob = {
  id: string;
  type: string;
  status: string;
  failure?: string | null;
  attempt: number;
  maxAttempts: number;
  createdAt: string;
};

export type AuditEvent = {
  id: string;
  eventType: string;
  actorType: string;
  createdAt: string;
};

export type CleanupPlan = {
  database: Record<string, number>;
  docker: { keepContainers: number; keepImages: number; jobWillRun: boolean };
};

export type BackupMetadata = {
  created_at?: string;
  scheduled?: string;
};

// A status message carries its tone explicitly so the UI never has to infer
// severity by matching substrings against locale-bound prose.
export type StatusMessage = { text: string; tone: "neutral" | "danger" };

const EMPTY_MESSAGE: StatusMessage = { text: "", tone: "neutral" };

function errorText(error: unknown, fallback: string) {
  return error instanceof Error ? error.message : fallback;
}

export type SettingsData = {
  github: StatusPayload | null;
  cloudflare: StatusPayload | null;
  version: VersionPayload | null;
  jobs: AgentJob[];
  audit: AuditEvent[];
  cleanup: CleanupPlan | null;
  backup: BackupMetadata | null;
  updateMessage: StatusMessage;
  operationsMessage: StatusMessage;
  refresh: () => void;
  checkForUpdates: () => Promise<void>;
  runCleanup: () => Promise<void>;
  retryJob: (id: string) => Promise<void>;
  cancelJob: (id: string) => Promise<void>;
};

// Owns every piece of settings state and all of the control-plane fetches so the
// page component can stay focused on layout. Each fetch fails independently and
// resolves to a safe fallback, matching the original per-request error handling.
export function useSettingsData(): SettingsData {
  const [github, setGithub] = useState<StatusPayload | null>(null);
  const [cloudflare, setCloudflare] = useState<StatusPayload | null>(null);
  const [version, setVersion] = useState<VersionPayload | null>(null);
  const [jobs, setJobs] = useState<AgentJob[]>([]);
  const [audit, setAudit] = useState<AuditEvent[]>([]);
  const [cleanup, setCleanup] = useState<CleanupPlan | null>(null);
  const [backup, setBackup] = useState<BackupMetadata | null>(null);
  const [updateMessage, setUpdateMessage] = useState<StatusMessage>(EMPTY_MESSAGE);
  const [operationsMessage, setOperationsMessage] = useState<StatusMessage>(EMPTY_MESSAGE);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useVisibilityPoll(
    () => {
      api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
    },
    { intervalMs: 30 * 60 * 1000, runImmediately: false },
  );

  function refresh() {
    api<StatusPayload>("/api/github/status").then(setGithub).catch((error) => setGithub({ message: errorText(error, "Could not load GitHub status.") }));
    api<StatusPayload>("/api/cloudflare/status").then(setCloudflare).catch((error) => setCloudflare({ message: errorText(error, "Could not load Cloudflare status.") }));
    api<VersionPayload>("/api/system/version").then(setVersion).catch(() => setVersion(null));
    api<AgentJob[]>("/api/agent-jobs").then(setJobs).catch(() => setJobs([]));
    api<AuditEvent[]>("/api/audit-events").then(setAudit).catch(() => setAudit([]));
    api<CleanupPlan>("/api/system/cleanup").then(setCleanup).catch(() => setCleanup(null));
    api<BackupMetadata | undefined>("/api/system/backups/latest").then((value) => setBackup(value || null)).catch(() => setBackup(null));
  }

  async function checkForUpdates() {
    setUpdateMessage({ text: "Checking for updates...", tone: "neutral" });
    try {
      const update = await api<UpdatePayload>("/api/system/update-check", { method: "POST", body: "{}" });
      setVersion((current) => current ? { ...current, update } : current);
      setUpdateMessage({ text: update.updateAvailable ? "Update available. Run hostlet update on the server." : "Hostlet is up to date.", tone: "neutral" });
    } catch (error) {
      setUpdateMessage({ text: errorText(error, "Could not check for updates."), tone: "danger" });
    }
  }

  async function runCleanup() {
    setOperationsMessage({ text: "Cleanup requested...", tone: "neutral" });
    try {
      await api("/api/system/cleanup", { method: "POST", body: "{}" });
      setOperationsMessage({ text: "Cleanup started. Docker cleanup will appear as an agent job.", tone: "neutral" });
      refresh();
    } catch (error) {
      setOperationsMessage({ text: errorText(error, "Cleanup failed."), tone: "danger" });
    }
  }

  async function retryJob(id: string) {
    try {
      await api(`/api/agent-jobs/${id}/retry`, { method: "POST", body: "{}" });
      refresh();
    } catch (error) {
      setOperationsMessage({ text: errorText(error, "Could not retry job."), tone: "danger" });
    }
  }

  async function cancelJob(id: string) {
    try {
      await api(`/api/agent-jobs/${id}/cancel`, { method: "POST", body: "{}" });
      refresh();
    } catch (error) {
      setOperationsMessage({ text: errorText(error, "Could not cancel job."), tone: "danger" });
    }
  }

  return {
    github,
    cloudflare,
    version,
    jobs,
    audit,
    cleanup,
    backup,
    updateMessage,
    operationsMessage,
    refresh,
    checkForUpdates,
    runCleanup,
    retryJob,
    cancelJob,
  };
}
