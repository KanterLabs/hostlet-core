"use client";

import { TourProvider, type TourStep } from "@/components/ui";

// Core dashboard tour. Anchors are data-tour attributes on the Nav links
// (rendered on both the desktop rail and mobile bar — the engine picks the
// visible one) and the overview metrics strip. Step 1 routes home so a
// Replay from /settings hops back before resolving.
const CORE_TOUR_STEPS: TourStep[] = [
  {
    id: "overview",
    target: '[data-tour="nav-overview"]',
    route: "/",
    title: "Welcome to Hostlet",
    body: "This is the control plane for deploying GitHub projects onto your own machine. Here's a quick look at the essentials.",
    placement: "right",
  },
  {
    id: "metrics",
    target: '[data-tour="overview-metrics"]',
    title: "Health at a glance",
    body: "Live counts for apps, active deploys, health checks, and machine status — they refresh as deployments run.",
    placement: "bottom",
  },
  {
    id: "apps",
    target: '[data-tour="nav-apps"]',
    title: "Create and deploy apps",
    body: "Each app is a GitHub repo deployed in Docker with its own route, health checks, and rollbacks. Start new projects here.",
    placement: "right",
  },
  {
    id: "machines",
    target: '[data-tour="nav-servers"]',
    title: "This machine",
    body: "Hostlet deploys apps onto the same machine that runs the control plane. Check its status and agent heartbeat here.",
    placement: "right",
  },
  {
    id: "logs",
    target: '[data-tour="nav-logs"]',
    title: "Follow every deployment",
    body: "Logs collect the latest build, health check, and routing output for each app.",
    placement: "right",
  },
  {
    id: "settings",
    target: '[data-tour="nav-settings"]',
    title: "Settings and connections",
    body: "Manage GitHub auth, Cloudflare DNS, updates, and cleanup. Replay this tour anytime from Settings.",
    placement: "right",
  },
];

export function CoreTour({ children }: { children: React.ReactNode }) {
  return (
    <TourProvider steps={CORE_TOUR_STEPS} autoStartPath="/">
      {children}
    </TourProvider>
  );
}
