"use client";

import type React from "react";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { usePathname, useRouter } from "next/navigation";
import { cx } from "@/components/ui/cx";

/**
 * Spotlight product tour engine. A `TourProvider` wraps the app shell with a
 * per-repo step list; nothing renders until a run starts (no SSR/hydration
 * surface). Each step dims the page with a box-shadow cutout over the target
 * element and shows a tooltip card with Skip / Back / Next. Runs auto-start
 * once per browser on `autoStartPath` (localStorage-tracked), can be forced
 * with `?tour=1` or `useTour().start({ force: true })`.
 */

export type TourStep = {
  id: string;
  /** CSS selector, e.g. '[data-tour="nav-apps"]'. First *visible* match wins. */
  target: string;
  title: string;
  body: string;
  /** Preferred card side; auto-flips through bottom/top/right/left to fit. */
  placement?: "top" | "bottom" | "left" | "right";
  /** Optional route to push before resolving the target (cross-page steps). */
  route?: string;
};

type TourDismissReason = "completed" | "skipped";

type TourContextValue = {
  active: boolean;
  stepIndex: number;
  start: (opts?: { force?: boolean }) => void;
  stop: (reason: TourDismissReason) => void;
  next: () => void;
  back: () => void;
};

const DEFAULT_STORAGE_KEY = "hostlet-tour";
const RESOLVE_TIMEOUT_MS = 1500;
const AUTO_START_DELAY_MS = 500;
const SPOTLIGHT_PAD = 6;
const GUTTER = 8;
const MOBILE_BREAKPOINT = 640;
const DIM_COLOR = "rgba(10,10,10,0.55)";

const TourContext = createContext<TourContextValue | null>(null);

/** True when a completed/skipped record exists at this version or newer. */
export function tourSeen(storageKey: string, version: number): boolean {
  try {
    const raw = localStorage.getItem(storageKey);
    if (!raw) return false;
    const record = JSON.parse(raw) as { v?: unknown };
    return typeof record.v === "number" && record.v >= version;
  } catch {
    return false;
  }
}

export function markTourSeen(storageKey: string, version: number): void {
  try {
    localStorage.setItem(storageKey, JSON.stringify({ v: version, completedAt: new Date().toISOString() }));
  } catch {
    // private mode / storage disabled — worst case the tour re-offers next visit.
  }
}

export function useTour() {
  const context = useContext(TourContext);
  if (!context) throw new Error("useTour must be used inside TourProvider");
  return context;
}

export function TourProvider({
  steps,
  storageKey = DEFAULT_STORAGE_KEY,
  version = 1,
  autoStartPath,
  onDismiss,
  children,
}: {
  steps: TourStep[];
  storageKey?: string;
  /** Bump to re-show the tour after big UI changes. */
  version?: number;
  /** Auto-start once on this pathname if no seen record exists. */
  autoStartPath?: string;
  onDismiss?: (reason: "completed" | "skipped") => void;
  children: React.ReactNode;
}) {
  const pathname = usePathname();
  const [active, setActive] = useState(false);
  const [stepIndex, setStepIndex] = useState(0);
  const activeRef = useRef(false);
  // Direction of travel (Next = 1, Back = -1) so a missing target skips onward
  // the same way the user was already moving.
  const directionRef = useRef<1 | -1>(1);
  const anyResolvedRef = useRef(false);
  const dismissedRef = useRef(false);
  const autoAttemptedRef = useRef(false);
  const forceCheckedRef = useRef(false);
  const onDismissRef = useRef(onDismiss);
  useEffect(() => {
    onDismissRef.current = onDismiss;
  }, [onDismiss]);

  const begin = useCallback(
    (force: boolean) => {
      if (steps.length === 0) return;
      if (!force && tourSeen(storageKey, version)) return;
      dismissedRef.current = false;
      anyResolvedRef.current = false;
      directionRef.current = 1;
      activeRef.current = true;
      setStepIndex(0);
      setActive(true);
    },
    [steps.length, storageKey, version],
  );

  const start = useCallback((opts?: { force?: boolean }) => begin(opts?.force === true), [begin]);

  // Both completed and skipped mark the tour seen; onDismiss fires once per run.
  const stop = useCallback(
    (reason: TourDismissReason) => {
      if (!activeRef.current) return;
      activeRef.current = false;
      markTourSeen(storageKey, version);
      if (!dismissedRef.current) {
        dismissedRef.current = true;
        onDismissRef.current?.(reason);
      }
      setActive(false);
    },
    [storageKey, version],
  );

  // End a run that never resolved a single target (e.g. auto-start raced a
  // page that lost its anchors) WITHOUT burning the seen record or onDismiss.
  const abort = useCallback(() => {
    activeRef.current = false;
    setActive(false);
  }, []);

  const next = useCallback(() => {
    directionRef.current = 1;
    if (stepIndex >= steps.length - 1) stop("completed");
    else setStepIndex(stepIndex + 1);
  }, [stepIndex, steps.length, stop]);

  const back = useCallback(() => {
    directionRef.current = -1;
    if (stepIndex > 0) setStepIndex(stepIndex - 1);
  }, [stepIndex]);

  const skip = useCallback(() => stop("skipped"), [stop]);

  const handleResolved = useCallback(() => {
    anyResolvedRef.current = true;
  }, []);

  // A step's target never appeared: skip it in the direction of travel
  // (bouncing forward off the front). Falling off the end completes the run,
  // unless nothing ever resolved — then abort without marking seen.
  const handleMissing = useCallback(() => {
    let target = stepIndex + directionRef.current;
    if (target < 0) {
      directionRef.current = 1;
      target = stepIndex + 1;
    }
    if (target >= steps.length) {
      if (anyResolvedRef.current) stop("completed");
      else abort();
      return;
    }
    setStepIndex(target);
  }, [stepIndex, steps.length, stop, abort]);

  // ?tour=1 force-start. Read from window.location, NOT useSearchParams —
  // that would force a CSR bailout on statically rendered pages. Strip the
  // param so reloads and copied links don't re-trigger.
  useEffect(() => {
    if (forceCheckedRef.current) return;
    forceCheckedRef.current = true;
    const params = new URLSearchParams(window.location.search);
    if (params.get("tour") !== "1") return;
    params.delete("tour");
    const query = params.toString();
    window.history.replaceState(
      window.history.state,
      "",
      window.location.pathname + (query ? `?${query}` : "") + window.location.hash,
    );
    begin(true);
  }, [begin]);

  // First-visit auto-start, delayed so the page's targets exist. Attempted at
  // most once per mount; a cancelled timer (navigated away early) doesn't count.
  useEffect(() => {
    if (!autoStartPath || pathname !== autoStartPath) return;
    if (autoAttemptedRef.current || activeRef.current) return;
    if (tourSeen(storageKey, version)) return;
    const timer = window.setTimeout(() => {
      autoAttemptedRef.current = true;
      if (!activeRef.current) begin(false);
    }, AUTO_START_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [pathname, autoStartPath, storageKey, version, begin]);

  const value = useMemo(
    () => ({ active, stepIndex, start, stop, next, back }),
    [active, stepIndex, start, stop, next, back],
  );

  const step = active ? steps[stepIndex] : undefined;

  return (
    <TourContext.Provider value={value}>
      {children}
      {step && (
        <TourOverlay
          step={step}
          stepIndex={stepIndex}
          total={steps.length}
          onNext={next}
          onBack={back}
          onSkip={skip}
          onResolved={handleResolved}
          onMissing={handleMissing}
        />
      )}
    </TourContext.Provider>
  );
}

type SpotRect = { top: number; left: number; width: number; height: number };

type CardPos = { mode: "float"; top: number; left: number } | { mode: "sheet"; side: "top" | "bottom" };

// First *visible* match: both repos render nav items twice (desktop rail +
// mobile bar), so a naive querySelector can land on a hidden zero-rect node.
function findVisibleTarget(selector: string): HTMLElement | null {
  let matches: NodeListOf<Element>;
  try {
    matches = document.querySelectorAll(selector);
  } catch {
    return null;
  }
  for (const el of matches) {
    if (el instanceof HTMLElement && el.getClientRects().length > 0) return el;
  }
  return null;
}

function spotlightRect(el: HTMLElement): SpotRect {
  const rect = el.getBoundingClientRect();
  return {
    top: rect.top - SPOTLIGHT_PAD,
    left: rect.left - SPOTLIGHT_PAD,
    width: rect.width + SPOTLIGHT_PAD * 2,
    height: rect.height + SPOTLIGHT_PAD * 2,
  };
}

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

function targetInside(node: EventTarget | null, element: HTMLElement | null): boolean {
  return !!element && node instanceof Node && element.contains(node);
}

function isInteractiveTarget(node: EventTarget | null): boolean {
  if (!(node instanceof HTMLElement)) return false;
  return !!node.closest(
    'a, button, input, textarea, select, [contenteditable="true"], [role="button"], [role="link"]',
  );
}

// Try [preferred, bottom, top, right, left]; first side where the card fits
// with an 8px gutter wins, clamped on the cross axis. Nothing fits → pin
// below the target, fully clamped on-screen.
function floatPosition(
  rect: SpotRect,
  preferred: TourStep["placement"],
  cardWidth: number,
  cardHeight: number,
  viewportWidth: number,
  viewportHeight: number,
): { top: number; left: number } {
  const candidates: Array<NonNullable<TourStep["placement"]>> = [];
  for (const side of [preferred, "bottom", "top", "right", "left"] as const) {
    if (side && !candidates.includes(side)) candidates.push(side);
  }
  const centerX = rect.left + rect.width / 2;
  const centerY = rect.top + rect.height / 2;
  const crossX = clamp(centerX - cardWidth / 2, GUTTER, viewportWidth - cardWidth - GUTTER);
  const crossY = clamp(centerY - cardHeight / 2, GUTTER, viewportHeight - cardHeight - GUTTER);
  for (const side of candidates) {
    if (side === "bottom") {
      const top = rect.top + rect.height + GUTTER;
      if (top + cardHeight <= viewportHeight - GUTTER) return { top, left: crossX };
    } else if (side === "top") {
      const top = rect.top - GUTTER - cardHeight;
      if (top >= GUTTER) return { top, left: crossX };
    } else if (side === "right") {
      const left = rect.left + rect.width + GUTTER;
      if (left + cardWidth <= viewportWidth - GUTTER) return { top: crossY, left };
    } else {
      const left = rect.left - GUTTER - cardWidth;
      if (left >= GUTTER) return { top: crossY, left };
    }
  }
  return {
    top: clamp(rect.top + rect.height + GUTTER, GUTTER, viewportHeight - cardHeight - GUTTER),
    left: crossX,
  };
}

function TourOverlay({
  step,
  stepIndex,
  total,
  onNext,
  onBack,
  onSkip,
  onResolved,
  onMissing,
}: {
  step: TourStep;
  stepIndex: number;
  total: number;
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
  onResolved: () => void;
  onMissing: () => void;
}) {
  const router = useRouter();
  const [rect, setRect] = useState<SpotRect | null>(null);
  const [cardPos, setCardPos] = useState<CardPos | null>(null);
  // Bumped when the current target unmounts mid-step; re-runs the resolver.
  const [resolveNonce, setResolveNonce] = useState(0);
  const cardRef = useRef<HTMLDivElement>(null);
  const primaryRef = useRef<HTMLButtonElement>(null);
  const targetElRef = useRef<HTMLElement | null>(null);
  const [reduceMotion] = useState(() => prefersReducedMotion());

  // Resolve the step's target, polling via rAF for up to ~1.5s to cover
  // post-navigation and data-dependent renders. The previous spotlight rect
  // stays up while polling so the cutout glides once the new rect lands.
  useEffect(() => {
    targetElRef.current = null;
    if (step.route && window.location.pathname !== step.route) router.push(step.route);
    let cancelled = false;
    let raf = 0;
    const deadline = performance.now() + RESOLVE_TIMEOUT_MS;
    const tick = () => {
      if (cancelled) return;
      const onRoute = !step.route || window.location.pathname === step.route;
      const el = onRoute ? findVisibleTarget(step.target) : null;
      if (el) {
        targetElRef.current = el;
        onResolved();
        el.scrollIntoView({ block: "center", behavior: prefersReducedMotion() ? "auto" : "smooth" });
        setRect(spotlightRect(el));
        return;
      }
      if (performance.now() >= deadline) {
        onMissing();
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => {
      cancelled = true;
      if (raf) cancelAnimationFrame(raf);
    };
  }, [step, stepIndex, resolveNonce, router, onResolved, onMissing]);

  // Re-measure on resize/scroll (capture catches nested scroll containers),
  // rAF-throttled. A vanished target re-enters the resolver, which
  // auto-advances if it never comes back.
  useEffect(() => {
    let raf = 0;
    const schedule = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        const el = targetElRef.current;
        if (!el) return;
        if (!el.isConnected || el.getClientRects().length === 0) {
          targetElRef.current = null;
          setResolveNonce((n) => n + 1);
          return;
        }
        setRect(spotlightRect(el));
      });
    };
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    return () => {
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      if (raf) cancelAnimationFrame(raf);
    };
  }, []);

  // Position the card pre-paint. Mobile uses a sheet (needs no measuring);
  // desktop measures the rendered card. `cardPos?.mode` in the deps re-runs
  // the measurement once after a sheet<->float switch changes the card's size.
  useLayoutEffect(() => {
    if (!rect) {
      setCardPos(null);
      return;
    }
    const card = cardRef.current;
    if (!card) return;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    if (viewportWidth < MOBILE_BREAKPOINT) {
      // Bottom sheet, unless the target sits in the bottom 40% of the
      // viewport (e.g. the mobile nav bar) — then the sheet flips to the top.
      const side = rect.top + rect.height > viewportHeight * 0.6 ? "top" : "bottom";
      setCardPos({ mode: "sheet", side });
      return;
    }
    const pos = floatPosition(rect, step.placement, card.offsetWidth, card.offsetHeight, viewportWidth, viewportHeight);
    setCardPos({ mode: "float", top: pos.top, left: pos.left });
  }, [rect, step, cardPos?.mode]);

  // Save focus at run start, restore when the run ends (overlay unmount).
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    return () => {
      previous?.focus?.();
    };
  }, []);

  // Keep focus on the primary button whenever it isn't already in the card
  // (run start, or a step change that disabled the focused button).
  const cardReady = cardPos !== null;
  useEffect(() => {
    if (!cardReady) return;
    const card = cardRef.current;
    if (card && card.contains(document.activeElement)) return;
    primaryRef.current?.focus({ preventScroll: true });
  }, [cardReady, stepIndex]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onSkip();
        return;
      }
      const insideCard = targetInside(event.target, cardRef.current);
      if (!insideCard && isInteractiveTarget(event.target)) return;
      if (event.key === "ArrowRight") {
        event.preventDefault();
        onNext();
        return;
      }
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        onBack();
        return;
      }
      if (event.key === "Enter") {
        // A focused tour button handles Enter natively (Skip/Back included).
        if (insideCard) return;
        event.preventDefault();
        onNext();
        return;
      }
      if (event.key === "Tab") {
        const card = cardRef.current;
        if (!card) return;
        const buttons = Array.from(card.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
        if (buttons.length === 0) return;
        const first = buttons[0];
        const last = buttons[buttons.length - 1];
        const current = document.activeElement;
        if (event.shiftKey) {
          if (current === first || !card.contains(current)) {
            event.preventDefault();
            last.focus();
          }
        } else if (current === last || !card.contains(current)) {
          event.preventDefault();
          first.focus();
        }
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onNext, onBack, onSkip]);

  const transition = reduceMotion
    ? undefined
    : "top 260ms ease, left 260ms ease, width 260ms ease, height 260ms ease";
  const sheet = cardPos?.mode === "sheet" ? cardPos : null;
  const cardStyle: React.CSSProperties = {};
  if (!cardPos) cardStyle.visibility = "hidden";
  if (cardPos?.mode === "float") {
    cardStyle.top = cardPos.top;
    cardStyle.left = cardPos.left;
    if (transition) cardStyle.transition = "top 260ms ease, left 260ms ease";
  }

  return (
    <>
      <div aria-hidden="true" className="fixed inset-0 z-[59] cursor-default" />
      {rect ? (
        <div
          aria-hidden="true"
          className="pointer-events-none fixed z-[60] rounded-lg"
          style={{
            top: rect.top,
            left: rect.left,
            width: rect.width,
            height: rect.height,
            boxShadow: `0 0 0 200vmax ${DIM_COLOR}`,
            transition,
          }}
        />
      ) : (
        // Plain dim while the first target resolves; swaps to the cutout.
        <div aria-hidden="true" className="pointer-events-none fixed inset-0 z-[60] bg-neutral-950/55" />
      )}
      {rect && (
        <div
          ref={cardRef}
          role="dialog"
          aria-modal="true"
          aria-labelledby="tour-step-title"
          aria-describedby="tour-step-body"
          className={cx(
            "fixed z-[60] rounded-lg border border-line bg-surface p-4 shadow-xl",
            sheet ? "inset-x-3" : "w-80",
            sheet && (sheet.side === "top" ? "top-3" : "bottom-3"),
          )}
          style={cardStyle}
        >
          <div className="data-label">
            Step {stepIndex + 1} of {total}
          </div>
          <h2 id="tour-step-title" className="mt-1 text-sm font-semibold">
            {step.title}
          </h2>
          <p id="tour-step-body" className="muted mt-1">
            {step.body}
          </p>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-2">
            <button type="button" className="button-secondary compact" onClick={onSkip}>
              Skip tour
            </button>
            <div className="flex gap-2">
              <button type="button" className="button-secondary compact" disabled={stepIndex === 0} onClick={onBack}>
                Back
              </button>
              <button ref={primaryRef} type="button" className="button compact" onClick={onNext}>
                {stepIndex === total - 1 ? "Done" : "Next"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
