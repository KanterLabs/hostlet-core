"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { CheckCircle2, CreditCard, Gauge, LogIn, TerminalSquare } from "lucide-react";
import { api } from "@/lib/api";
import { Notice, Panel, StatusPill } from "@/components/ui";
import { planLabel } from "@/components/CloudUsagePanel";

type PlanCode = "student" | "starter" | "pro";
type SessionPayload = {
  mode: "self_hosted" | "cloud";
  authenticated?: boolean;
  cloud?: {
    billingActive: boolean;
    githubInstalled: boolean;
    nextStep: "login" | "install_github" | "billing" | "ready";
    planCode?: string | null;
    subscriptionStatus?: string | null;
  } | null;
};

const plans: Array<{ code: PlanCode; price: string; apps: number; note: string }> = [
  { code: "student", price: "$4", apps: 1, note: "For one course project or small app." },
  { code: "starter", price: "$9", apps: 2, note: "For a couple of production apps." },
  { code: "pro", price: "$19", apps: 4, note: "For multiple launches and client projects." },
];

export default function Pricing() {
  const [session, setSession] = useState<SessionPayload | null>(null);
  const [busy, setBusy] = useState<PlanCode | "portal" | "">("");
  const [message, setMessage] = useState("");

  useEffect(() => {
    api<SessionPayload>("/api/session").then(setSession).catch(() => setSession(null));
  }, []);

  const currentPlan = session?.cloud?.planCode || null;
  const subscribed = !!session?.cloud?.billingActive;

  async function startCheckout(plan: PlanCode) {
    if (!session?.authenticated) {
      window.location.assign("/login");
      return;
    }
    setBusy(plan);
    setMessage(`Opening ${planLabel(plan)} checkout...`);
    try {
      const result = await api<{ url?: string | null }>("/api/cloud/billing/checkout", {
        method: "POST",
        body: JSON.stringify({ plan }),
      });
      if (!result.url) throw new Error("Stripe did not return a checkout URL.");
      window.location.assign(result.url);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Checkout could not be opened.");
      setBusy("");
    }
  }

  async function openBillingPortal() {
    setBusy("portal");
    setMessage("Opening subscription portal...");
    try {
      const result = await api<{ url?: string | null }>("/api/cloud/billing/portal", {
        method: "POST",
        body: "{}",
      });
      if (!result.url) throw new Error("Stripe did not return a billing portal URL.");
      window.location.assign(result.url);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Billing portal could not be opened.");
      setBusy("");
    }
  }

  return (
    <main className="min-h-screen bg-panel text-ink">
      <header className="border-b border-line bg-surface">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-3 px-4 py-4">
          <Link href="/" className="flex items-center gap-3">
            <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-action text-white">
              <TerminalSquare size={19} />
            </span>
            <span>
              <span className="block font-semibold leading-5">Hostlet Cloud</span>
              <span className="text-xs font-medium text-muted">simple app-based pricing</span>
            </span>
          </Link>
          <div className="flex flex-wrap items-center gap-2">
            {subscribed && currentPlan && <StatusPill status="connected" label={`${planLabel(currentPlan)} active`} />}
            <Link className="button-secondary" href={session?.authenticated ? "/" : "/login"}>
              {session?.authenticated ? "Dashboard" : "Log in"}
            </Link>
          </div>
        </div>
      </header>

      <section className="mx-auto max-w-6xl px-4 py-10">
        <div className="mb-7 max-w-2xl">
          <div className="eyebrow mb-2">Pricing</div>
          <h1 className="text-3xl font-semibold sm:text-4xl">Hostlet Cloud plans</h1>
          <p className="muted mt-3">Pick by the number of apps you want to run. CPU and memory stay managed by Hostlet Cloud.</p>
        </div>

        <div className="grid gap-4 md:grid-cols-3">
          {plans.map((plan) => {
            const isCurrent = currentPlan === plan.code && subscribed;
            return (
              <Panel key={plan.code} className={isCurrent ? "border-action" : undefined}>
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h2 className="text-xl font-semibold">{planLabel(plan.code)}</h2>
                    <p className="muted mt-1">{plan.note}</p>
                  </div>
                  {isCurrent && <StatusPill status="connected" label="current" />}
                </div>
                <div className="mt-5 flex items-end gap-1">
                  <span className="text-4xl font-semibold">{plan.price}</span>
                  <span className="pb-1 text-muted">/mo</span>
                </div>
                <div className="mt-5 rounded-lg border border-line bg-surface-alt p-4">
                  <div className="flex items-center gap-2 font-medium">
                    <Gauge size={17} />
                    {plan.apps} app{plan.apps === 1 ? "" : "s"} included
                  </div>
                </div>
                <ul className="mt-5 space-y-2 text-sm">
                  <li className="flex gap-2"><CheckCircle2 size={16} className="mt-0.5 text-action" />Always-on Hostlet Cloud deploys</li>
                  <li className="flex gap-2"><CheckCircle2 size={16} className="mt-0.5 text-action" />Managed hostlet.cloud URLs</li>
                  <li className="flex gap-2"><CheckCircle2 size={16} className="mt-0.5 text-action" />GitHub repo deployments</li>
                </ul>
                <button
                  className="mt-6 w-full"
                  disabled={!!busy}
                  onClick={isCurrent || subscribed ? openBillingPortal : () => startCheckout(plan.code)}
                >
                  {isCurrent || subscribed ? <CreditCard size={16} /> : session?.authenticated ? <CreditCard size={16} /> : <LogIn size={16} />}
                  {busy === plan.code || (busy === "portal" && (isCurrent || subscribed))
                    ? "Opening..."
                    : isCurrent
                      ? "Manage subscription"
                      : subscribed
                        ? "Change plan"
                        : session?.authenticated
                          ? `Start ${planLabel(plan.code)}`
                          : "Log in to start"}
                </button>
              </Panel>
            );
          })}
        </div>

        {message && (
          <Notice
            tone={message.toLowerCase().includes("could not") || message.toLowerCase().includes("did not") || message.toLowerCase().includes("failed") ? "danger" : "neutral"}
            className="mt-5"
            description={message}
          />
        )}
      </section>
    </main>
  );
}
