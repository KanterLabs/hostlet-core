"use client";

import { useEffect, type DependencyList } from "react";

export type VisibilityPollContext = {
  isActive: () => boolean;
};

export type VisibilityPollOptions = {
  intervalMs: number;
  enabled?: boolean;
  runImmediately?: boolean;
  deps?: DependencyList;
};

export function useVisibilityPoll(
  load: (context: VisibilityPollContext) => void | Promise<void>,
  {
    intervalMs,
    enabled = true,
    runImmediately = true,
    deps = [],
  }: VisibilityPollOptions,
) {
  useEffect(() => {
    if (!enabled) return;

    let active = true;
    let inFlight = false;
    const context: VisibilityPollContext = {
      isActive: () => active,
    };
    const run = () => {
      if (inFlight) return;
      inFlight = true;
      void Promise.resolve(load(context)).finally(() => {
        inFlight = false;
      });
    };

    if (runImmediately) run();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") run();
    }, intervalMs);

    return () => {
      active = false;
      window.clearInterval(timer);
    };
    // Callers choose dependencies explicitly so inline loaders do not restart polling every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, intervalMs, runImmediately, ...deps]);
}
