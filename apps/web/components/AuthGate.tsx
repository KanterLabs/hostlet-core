"use client";

import { FormEvent, useEffect, useState } from "react";
import { LockKeyhole } from "lucide-react";
import { apiUrl } from "@/lib/api";

type SetupStatus = {
  setupRequired: boolean;
  unlocked: boolean;
};

const UNLOCK_STORAGE_KEY = "hostlet:control-plane-unlocked";

export function AuthGate({ children }: { children: React.ReactNode }) {
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
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
    if (window.localStorage.getItem(UNLOCK_STORAGE_KEY) === "true") {
      setLocallyUnlocked(true);
    }
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
    const res = await fetch(`${apiUrl()}${path}`, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ password }),
    });
    setSaving(false);
    if (!res.ok) {
      setError(await res.text());
      return;
    }
    setPassword("");
    setConfirm("");
    window.localStorage.setItem(UNLOCK_STORAGE_KEY, "true");
    setLocallyUnlocked(true);
  }

  if (status?.unlocked || locallyUnlocked) return <>{children}</>;

  if (!status) {
    return (
      <main className="flex min-h-screen items-center justify-center p-6">
        <section className="w-full max-w-sm rounded-lg border border-line bg-white p-6">
          <div className="mb-5 flex items-center gap-3">
            <div className="rounded-md border border-line bg-panel p-2 text-ink">
              <LockKeyhole size={18} />
            </div>
            <div>
              <h1 className="text-lg font-semibold">Checking Hostlet</h1>
              <p className="muted mt-1">Loading control-plane security status.</p>
            </div>
          </div>
          {error && <p className="text-sm text-red-700">{error}</p>}
        </section>
      </main>
    );
  }

  const setup = status.setupRequired;
  return (
    <main className="flex min-h-screen items-center justify-center p-6">
      <section className="w-full max-w-sm rounded-lg border border-line bg-white p-6">
        <div className="mb-5 flex items-center gap-3">
          <div className="rounded-md border border-line bg-panel p-2 text-ink">
            <LockKeyhole size={18} />
          </div>
          <div>
            <h1 className="text-lg font-semibold">{setup ? "Secure Hostlet" : "Unlock Hostlet"}</h1>
            <p className="muted mt-1">
              {setup ? "Set the control-plane password for this machine." : "Enter the control-plane password."}
            </p>
          </div>
        </div>
        <form onSubmit={submit} className="space-y-3">
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
          {error && <p className="text-sm text-red-700">{error}</p>}
          <button className="w-full" disabled={saving || !status}>
            {saving ? "Saving..." : setup ? "Set password" : "Unlock"}
          </button>
        </form>
      </section>
    </main>
  );
}
