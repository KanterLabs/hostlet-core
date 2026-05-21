"use client";

import { apiUrl } from "@/lib/api";
import { GitHubStatus } from "@/components/GitHubStatus";
import { GitBranch } from "lucide-react";

export default function Login() {
  return (
    <main className="flex min-h-screen items-center justify-center p-6">
      <section className="w-full max-w-sm rounded-lg border border-line bg-white p-6">
        <h1 className="text-2xl font-semibold">Hostlet</h1>
        <p className="muted mt-2">Sign in to deploy a GitHub repo on this machine or a VPS.</p>
        <div className="mt-5"><GitHubStatus /></div>
        <a className="button mt-6 w-full" href={`${apiUrl()}/auth/github/start`}><GitBranch size={16}/>Continue with GitHub</a>
      </section>
    </main>
  );
}
