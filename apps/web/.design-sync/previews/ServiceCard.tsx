import * as React from "react";
import { ServiceCard, ServiceStack } from "@hostlet/web";
import type { ServiceSummary } from "@hostlet/web";

const webService: ServiceSummary = {
  name: "api",
  role: "web",
  imageTag: "ghcr.io/acme/api:a3f9c12",
  targetPort: 8080,
  publishedPort: 443,
  status: "running",
  healthStatus: "healthy",
  lastCheckedAt: "2026-06-20T14:22:00Z",
  lastHealthyAt: "2026-06-20T14:22:00Z",
};

const dbService: ServiceSummary = {
  name: "postgres",
  role: "backing",
  imageTag: "postgres:16-alpine",
  containerName: "acme-postgres-1",
  targetPort: 5432,
  status: "running",
  healthStatus: null,
};

const redisService: ServiceSummary = {
  name: "redis",
  role: "backing",
  imageTag: "redis:7-alpine",
  containerName: "acme-redis-1",
  targetPort: 6379,
  status: "running",
  healthStatus: null,
};

const exitedService: ServiceSummary = {
  name: "worker",
  role: "backing",
  imageTag: "ghcr.io/acme/worker:a3f9c12",
  containerName: "acme-worker-1",
  status: "exited",
  healthStatus: null,
};

export const WebService = () => (
  <div style={{ maxWidth: 520 }}>
    <ServiceCard service={webService} />
  </div>
);

export const BackingService = () => (
  <div style={{ maxWidth: 520 }}>
    <ServiceCard service={dbService} />
  </div>
);

export const ServiceStackFull = () => (
  <div style={{ maxWidth: 560 }}>
    <ServiceStack services={[webService, dbService, redisService]} />
  </div>
);

export const ServiceStackDegraded = () => (
  <div style={{ maxWidth: 560 }}>
    <ServiceStack services={[webService, dbService, exitedService]} />
  </div>
);
