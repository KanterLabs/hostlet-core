import { formatTimestamp } from "@/lib/time";

export type RuntimeLogLine = {
  timestamp?: string | null;
  service: string;
  stream: "stdout" | "stderr" | string;
  line: string;
};

export type RuntimeLogServiceError = {
  service: string;
  message: string;
};

export type RuntimeLogSnapshot = {
  deploymentId: string;
  capturedAt: string;
  truncated: boolean;
  lines: RuntimeLogLine[];
  unavailableServices: RuntimeLogServiceError[];
};

export function runtimeLogTimestamp(value?: string | null) {
  if (!value) return "no timestamp";
  const formatted = formatTimestamp(value, "time");
  return formatted === "unknown" ? value : formatted;
}

export function unavailableRuntimeServices(errors: readonly RuntimeLogServiceError[]) {
  if (errors.length === 0) return "";
  const services = errors.map((error) => error.service).filter(Boolean);
  if (services.length === 0) return "Some service logs were unavailable.";
  return `Logs were unavailable for ${services.join(", ")}.`;
}
