"use client";

import { GitHubStatus } from "@/components/GitHubStatus";
import { GitHubDeviceFlow } from "@/components/GitHubDeviceFlow";
import { GitBranch, TerminalSquare } from "lucide-react";
import { useEffect, useState } from "react";
import { apiUrl } from "@/lib/api";
import { IconFrame, Notice, Panel } from "@/components/ui";

type SetupStatus = {
  mode?: "self_hosted" | "cloud";
};

export default function Login() {
  const [mode, setMode] = useState<"self_hosted" | "cloud" | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    fetch(`${apiUrl()}/api/setup/status`, { credentials: "include", cache: "no-store" })
      .then((response) => response.json())
      .then((status: SetupStatus) => setMode(status.mode || "self_hosted"))
      .catch(() => setMode("self_hosted"));
  }, []);

  function startCloudLogin() {
    window.location.assign(`${apiUrl()}/auth/github/oauth/start`);
  }

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    setError(params.get("error") || "");
  }, []);

  const cloud = mode === "cloud";

  return (
    <main className="flex min-h-screen items-center justify-center bg-panel p-6">
      <Panel className="w-full max-w-md border-t-4 border-t-action p-6" padded={false}>
        <div className="flex items-center gap-3">
          <IconFrame icon={TerminalSquare} className="h-11 w-11 bg-action text-white ring-0" />
          <div>
            <h1 className="text-2xl font-semibold">Hostlet</h1>
            <p className="muted mt-1">{cloud ? "Sign in to Hostlet Cloud." : "Sign in to manage your Hostlet deployments."}</p>
          </div>
        </div>
        {error && <Notice tone="danger" className="mt-5" description={error} />}
        {cloud ? (
          <button className="mt-6 w-full" onClick={startCloudLogin} disabled={!mode}>
            <GitBranch size={16} />
            Continue with GitHub
          </button>
        ) : (
          <>
            <div className="mt-5"><GitHubStatus showConnect={false} /></div>
            <GitHubDeviceFlow className="mt-6" buttonLabel="Continue with GitHub" fullWidth />
          </>
        )}
      </Panel>
    </main>
  );
}
