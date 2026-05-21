# Hostlet

Hostlet is a minimal self-hosted deployment platform. By default it deploys Dockerized GitHub apps on the same machine running Hostlet. You can also connect a VPS with the remote agent for public HTTPS deployments.

## Local setup

1. Copy `.env.example` to `.env` and set GitHub OAuth values.
2. Run `pnpm dev` from the repo root, or run `docker compose -f infra/docker-compose.yml up --build`.
3. Open `http://localhost:3000`.
4. Sign in with GitHub, create an app, and deploy to `This machine`.
5. Add a VPS only when you want a separate remote deploy target.

## GitHub OAuth

Create an OAuth app with callback URL:

`http://localhost:8080/auth/github/callback`

Set `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET`.

## GitHub webhooks

Point repository webhooks at:

`http://localhost:8080/webhooks/github`

Use the same secret as `GITHUB_WEBHOOK_SECRET`. Push events trigger deployments when repo and branch match an app.

## Local deploy target

Docker Compose starts a `local-agent` service. It connects back to the API and uses the host Docker socket to build and run deployed app containers on the same computer.

Local apps skip Caddy and use a loopback port such as:

`http://127.0.0.1:23000`

VPS apps still use the install script, Caddy, and HTTPS routing.
