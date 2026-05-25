# Deploying Apps With Hostlet

Hostlet supports three app shapes.

## Dockerfile Apps

Add a `Dockerfile` at the app root, set the container port in Hostlet, and make sure the container listens on that port. This is the best path for C++, Go, Rust, Python, PHP, custom Node servers, and any app with special system dependencies.

## Generated Node Apps

If no `Dockerfile` exists and Hostlet finds `package.json`, it can generate a Node image. It supports npm, pnpm, and yarn lockfiles. Hostlet detects Next.js, Vite, Astro, Nuxt, Remix, SvelteKit, and generic Node apps.

Next.js, Nuxt, Remix, and generic Node apps need a `start` script or a custom start command. Static Vite, Astro, and SvelteKit builds are served from `dist`.

## Compose Apps

Compose apps are for one public web service with private supporting services. Add `hostlet.yml` next to the Compose file:

```yaml
version: 1
runtime: compose
compose:
  file: compose.yaml
  web_service: web
  port: 3000
  health_path: /
```

Example `compose.yaml`:

```yaml
services:
  web:
    build: .
    depends_on:
      - redis
    environment:
      REDIS_URL: redis://redis:6379

  worker:
    build: .
    command: npm run worker
    depends_on:
      - redis

  redis:
    image: redis:7-alpine
    volumes:
      - redis-data:/data

volumes:
  redis-data:
```

Hostlet binds the web service to a loopback-only dynamic host port, health-checks it, and publishes the Caddy route after the health check passes. Supporting services stay private on the Compose network.

Supported Compose fields in 0.3.8 include `services`, `build`, `image`, `command`, `entrypoint`, `environment`, `env_file`, `depends_on`, `healthcheck`, `volumes`, named top-level `volumes`, and default networks.

Hostlet rejects fields that break its routing, rollback, cleanup, or security model: `ports`, `container_name`, `network_mode: host`, `privileged`, host PID/IPC, devices, and host bind mounts. Use named volumes for persistent data.

## Rollbacks And State

Dockerfile and generated Node apps roll back by routing to a previous successful container.

Compose apps roll back the application service definitions and images while reusing the same named volumes. Hostlet does not snapshot or roll back database contents. Run app migrations with that in mind.
