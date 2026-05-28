"use client";

import { FormEvent, useEffect, useState } from "react";
import { LockKeyhole, ShieldCheck, TerminalSquare } from "lucide-react";
import { apiUrl } from "@/lib/api";
import { IconFrame, Notice, Panel } from "@/components/ui";

type SetupStatus = {
  mode?: "self_hosted";
  setupRequired: boolean;
  unlocked: boolean;
};

export function AuthGate({ children }: { children: React.ReactNode }) {
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [setupToken, setSetupToken] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [locallyUnlocked, setLocallyUnlocked] = useState(false);

  async function refresh() {
    const res = await fetch(`${apiUrl()}/api/setup/status`, {
      credentials: "include",
      cache: "no-store",
    });
    if (!res.ok) throw new Error(await res.text());
    setStatus(await res.json());
  }

  useEffect(() => {
    refresh().catch((err) => setError(err.message || "Could not reach Hostlet API."));
  }, []);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError("");
    if (status?.setupRequired && password !== confirm) {
      setError("Passwords do not match.");
      return;
    }
    setSaving(true);
    const path = status?.setupRequired ? "/api/setup" : "/api/unlock";
    const headers: Record<string, string> = { "Content-Type": "application/json", "X-Hostlet-CSRF": "1" };
    if (status?.setupRequired && setupToken.trim()) {
      headers["X-Hostlet-Setup-Token"] = setupToken.trim();
    }
    const res = await fetch(`${apiUrl()}${path}`, {
      method: "POST",
      credentials: "include",
      headers,
      body: JSON.stringify({ password }),
    });
    setSaving(false);
    if (!res.ok) {
      setError(await res.text());
      return;
    }
    setPassword("");
    setConfirm("");
    setSetupToken("");
    setLocallyUnlocked(true);
    await refresh().catch(() => undefined);
  }

  if (status?.unlocked || locallyUnlocked) return <>{children}</>;

  if (!status) {
    return (
      <main className="flex min-h-screen items-center justify-center bg-panel p-6">
        <Panel className="w-full max-w-md border-t-4 border-t-action p-6" padded={false}>
          <AuthBrand />
          <Notice tone="neutral" className="mt-5" description="Checking control-plane security status..." />
          {error && <Notice tone="danger" className="mt-3" description={error} />}
        </Panel>
      </main>
    );
  }

  const setup = status.setupRequired;
  return (
    <main className="flex min-h-screen items-center justify-center bg-panel p-6">
      <Panel className="w-full max-w-md border-t-4 border-t-action p-6" padded={false}>
        <AuthBrand />
        <div className="mt-5 rounded-lg border border-line bg-surface-alt p-4">
          <div className="flex items-center gap-3">
            <IconFrame icon={setup ? ShieldCheck : LockKeyhole} className="h-9 w-9 rounded-md bg-surface" />
            <div>
              <h1 className="text-lg font-semibold">{setup ? "Secure Hostlet" : "Unlock Hostlet"}</h1>
              <p className="muted mt-1">
                {setup ? "Set the control-plane password for this machine." : "Enter the control-plane password."}
              </p>
            </div>
          </div>
        </div>
        <form onSubmit={submit} className="mt-5 space-y-3">
          <label className="block text-sm font-medium">
            Password
            <input
              className="mt-1"
              type="password"
              minLength={12}
              autoComplete={setup ? "new-password" : "current-password"}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
            />
          </label>
          {setup && (
            <label className="block text-sm font-medium">
              Confirm password
              <input
                className="mt-1"
                type="password"
                minLength={12}
                autoComplete="new-password"
                value={confirm}
                onChange={(event) => setConfirm(event.target.value)}
                required
              />
            </label>
          )}
          {setup && (
            <label className="block text-sm font-medium">
              Setup token
              <input
                className="mt-1"
                type="password"
                autoComplete="one-time-code"
                value={setupToken}
                onChange={(event) => setSetupToken(event.target.value)}
                placeholder="Required if configured"
              />
            </label>
          )}
          {error && <Notice tone="danger" description={error} />}
          <button className="w-full" disabled={saving || !status}>
            {saving ? "Saving..." : setup ? "Set password" : "Unlock"}
          </button>
        </form>
      </Panel>
    </main>
  );
}

function AuthBrand() {
  return (
    <div className="flex items-center gap-3">
      <IconFrame icon={TerminalSquare} className="h-11 w-11 bg-action text-white ring-0" />
      <div>
        <div className="text-xl font-semibold">Hostlet</div>
        <p className="muted mt-0.5">Self-hosted deployments</p>
      </div>
    </div>
  );
}
