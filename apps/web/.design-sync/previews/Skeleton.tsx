import * as React from "react";
import { Skeleton } from "@hostlet/web";

export const LoadingRows = () => (
  <div style={{ maxWidth: 360, padding: 16, display: "flex", flexDirection: "column", gap: 10 }}>
    <Skeleton className="h-4 w-48" />
    <Skeleton className="h-4 w-64" />
    <Skeleton className="h-4 w-40" />
    <Skeleton className="h-4 w-56" />
  </div>
);

export const LoadingCard = () => (
  <div style={{ maxWidth: 280, padding: 16, background: "var(--surface, #fff)", borderRadius: 8, border: "1px solid var(--line, #e5e7eb)", display: "flex", flexDirection: "column", gap: 8 }}>
    <Skeleton className="h-3 w-20" />
    <Skeleton className="mt-1 h-6 w-24" />
    <Skeleton className="mt-1 h-4 w-32" />
  </div>
);
