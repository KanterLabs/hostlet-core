// Types, count helpers, and "last seen" formatting for the machines (servers) list.

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
