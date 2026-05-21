import Link from "next/link";
import { Nav } from "@/components/Nav";
import { GitHubStatus } from "@/components/GitHubStatus";

export default function Dashboard() {
  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <div className="mb-8 flex items-center justify-between">
          <div>
            <h1 className="text-2xl font-semibold">Deployments</h1>
            <p className="muted">Deploy on this machine first. Add a VPS when you need one.</p>
          </div>
          <Link className="button" href="/apps/new">Create app</Link>
        </div>
        <div className="mb-6 max-w-2xl"><GitHubStatus /></div>
        <div className="grid gap-4 md:grid-cols-3">
          {["Use this machine", "Connect a repo", "Deploy safely"].map((title, i) => (
            <div key={title} className="rounded-lg border border-line bg-white p-5">
              <div className="mb-3 flex h-8 w-8 items-center justify-center rounded-full bg-panel text-sm font-semibold">{i + 1}</div>
              <h2 className="font-medium">{title}</h2>
              <p className="muted mt-2">{["The local agent builds and runs containers on the computer running Hostlet.", "Choose a GitHub repo and branch.", "Blue-green-lite deploys keep the current version live until health checks pass."][i]}</p>
            </div>
          ))}
        </div>
      </section>
    </main>
  );
}
