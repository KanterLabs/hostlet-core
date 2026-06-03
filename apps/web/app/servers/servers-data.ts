// Types, polling helper, and derived counts for the machines (servers) list.

import { useEffect } from "react";

export type ServerRow = {
  id: string;
  name: string;
  publicIp?: string;
  kind: string;
  status: string;
  lastSeenAt?: string;
};

// How often the machines list re-polls its data while the tab is visible.
export const SERVERS_POLL_INTERVAL_MS = 10000;

// Runs `load` once on mount, then re-runs it on an interval while the tab is
// visible. Mirrors the visibility-aware polling used by the dashboard overview.
export function useVisibilityPoll(load: () => void, intervalMs: number) {
  useEffect(() => {
    load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") load();
    }, intervalMs);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

export type ServerCounts = { online: number; local: number };

export function deriveServerCounts(servers: ServerRow[]): ServerCounts {
  return {
    online: servers.filter((server) => server.status === "online").length,
    local: servers.filter((server) => server.kind === "local").length,
  };
}

// Human-readable "last seen" label for a machine row.
export function formatLastSeen(lastSeenAt?: string) {
  return lastSeenAt ? new Date(lastSeenAt).toLocaleString() : "Not seen yet";
}
