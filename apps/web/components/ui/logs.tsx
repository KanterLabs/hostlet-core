"use client";

import type React from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownToLine, ChevronDown, WrapText, XCircle } from "lucide-react";
import { CopyButton } from "@/components/ui/actions";
import { cx } from "@/components/ui/cx";

const ERROR_PATTERN = /\b(error|fatal|exception|failed|cannot|not found|denied|panic|traceback)\b/i;
const COMMAND_ECHO_PATTERN = /^(?:[a-z]+:\s*)?\$\s+/i;

export function firstErrorLine(lines: readonly string[]) {
  return lines.find((line) => !COMMAND_ECHO_PATTERN.test(line) && ERROR_PATTERN.test(line)) || "";
}

function parseLogLine(raw: string): { stream: "stdout" | "stderr"; text: string } {
  if (raw.startsWith("stdout: ")) return { stream: "stdout", text: raw.slice(8) };
  if (raw.startsWith("stderr: ")) return { stream: "stderr", text: raw.slice(8) };
  return { stream: "stdout", text: raw };
}

export function LogViewer({
  lines,
  emptyText = "Waiting for logs...",
  highlightFirstError = true,
  wrapMobile = true,
  className = "",
  toolbar,
  title,
  collapsible,
  defaultCollapsed,
}: {
  lines: readonly string[];
  emptyText?: string;
  highlightFirstError?: boolean;
  wrapMobile?: boolean;
  className?: string;
  toolbar?: React.ReactNode;
  title?: React.ReactNode;
  collapsible?: boolean;
  defaultCollapsed?: boolean;
}) {
  const logRef = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const [collapsed, setCollapsed] = useState(defaultCollapsed ?? false);
  const [wrap, setWrap] = useState(wrapMobile);

  const parsed = useMemo(() => lines.map(parseLogLine), [lines]);
  const cleanText = useMemo(() => parsed.map((p) => p.text).join("\n"), [parsed]);
  const firstError = highlightFirstError ? firstErrorLine(lines) : "";

  useEffect(() => {
    const el = logRef.current;
    if (el && following) el.scrollTop = el.scrollHeight;
  }, [cleanText, following]);

  function onLogScroll() {
    const el = logRef.current;
    if (!el) return;
    setFollowing(el.scrollHeight - el.scrollTop - el.clientHeight < 48);
  }

  function jumpToLatest() {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    setFollowing(true);
  }

  if (collapsible && collapsed) {
    return (
      <div className={className}>
        <div className="flex items-center justify-between rounded-lg border border-line bg-surface px-3 py-2 text-sm">
          <span className="font-medium">{title || "Build logs"} · {lines.length} lines</span>
          <button type="button" className="button-secondary compact" onClick={() => setCollapsed(false)}>
            <ChevronDown size={13} />Show
          </button>
        </div>
      </div>
    );
  }

  const wrapClass = wrap
    ? "[overflow-wrap:anywhere] [white-space:pre-wrap] md:[overflow-wrap:normal] md:[white-space:pre]"
    : "whitespace-pre";

  return (
    <div className={className}>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2 text-xs text-muted">
        <div className="flex items-center gap-2 font-medium text-ink">
          {collapsible && (
            <button
              type="button"
              className="button-secondary compact"
              onClick={() => setCollapsed(true)}
              aria-label="Collapse logs"
            >
              <ChevronDown size={13} />
            </button>
          )}
          {title}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {toolbar}
          <CopyButton value={cleanText} label="Copy logs" copiedLabel="Copied" className="button-secondary compact" />
          <button
            type="button"
            className="button-secondary compact"
            onClick={() => setWrap((w) => !w)}
            aria-pressed={wrap}
          >
            <WrapText size={13} />{wrap ? "Wrap" : "No wrap"}
          </button>
          <span>{lines.length} lines</span>
          {following ? (
            <span className="text-action">following</span>
          ) : (
            <button type="button" className="button-secondary compact" onClick={jumpToLatest}>
              <ArrowDownToLine size={13} />Jump to latest
            </button>
          )}
        </div>
      </div>
      {firstError && (
        <div className="mb-3 rounded-md border border-red-300 bg-red-50 p-3">
          <div className="mb-1 flex items-center gap-2 text-sm font-medium text-red-800">
            <XCircle size={15} />First error in the log
          </div>
          <pre className="overflow-x-auto whitespace-pre-wrap break-words font-mono text-xs text-red-900">{firstError}</pre>
        </div>
      )}
      <div
        ref={logRef}
        onScroll={onLogScroll}
        className="min-h-[220px] max-h-[52vh] max-w-full overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-4 font-mono text-sm leading-6 text-green-100 shadow-sm shadow-neutral-950/20 md:max-h-[68vh]"
      >
        {lines.length === 0 ? (
          <div className="text-muted">{emptyText}</div>
        ) : (
          parsed.map(({ stream, text }, index) => {
            const isEcho = COMMAND_ECHO_PATTERN.test(text);
            const isError = !isEcho && ERROR_PATTERN.test(text);
            const accent = stream === "stderr" ? "border-l-2 border-amber-500/70 pl-2" : "";
            const color = isEcho
              ? "font-semibold text-emerald-200"
              : isError
              ? "text-red-300"
              : stream === "stderr"
              ? "text-amber-200/90"
              : "text-green-100";
            return (
              <div
                key={index}
                data-stream={stream}
                className={cx(wrapClass, accent, color, "min-h-[1.25rem]")}
              >
                {text}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
