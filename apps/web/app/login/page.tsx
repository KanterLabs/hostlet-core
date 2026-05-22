"use client";

import { GitHubStatus } from "@/components/GitHubStatus";
import { GitHubDeviceFlow } from "@/components/GitHubDeviceFlow";
import { TerminalSquare } from "lucide-react";
import { IconFrame, Panel } from "@/components/ui";

export default function Login() {
  return (
    <main className="flex min-h-screen items-center justify-center bg-panel p-6">
      <Panel className="w-full max-w-md border-t-4 border-t-action p-6" padded={false}>
        <div className="flex items-center gap-3">
          <IconFrame icon={TerminalSquare} className="h-11 w-11 bg-action text-white ring-0" />
          <div>
            <h1 className="text-2xl font-semibold">Hostlet</h1>
            <p className="muted mt-1">Sign in to manage your self-hosted deployments.</p>
          </div>
        </div>
        <div className="mt-5"><GitHubStatus showConnect={false} /></div>
        <GitHubDeviceFlow className="mt-6" buttonLabel="Continue with GitHub" fullWidth />
      </Panel>
    </main>
  );
}
