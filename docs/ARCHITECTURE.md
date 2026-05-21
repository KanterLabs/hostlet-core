# Architecture

Hostlet has three pieces:

- `apps/web`: Next.js dashboard for login, servers, apps, deployments, logs, and settings.
- `apps/api`: Rust Axum control plane for OAuth, CRUD, webhooks, deployments, log streaming, and agent communication.
- `apps/agent`: Rust deployment agent. In local mode it builds and runs containers on the Hostlet machine. In remote mode it runs on a VPS, updates Caddy, streams logs, and rolls back.

The control plane stores users, GitHub accounts, servers, apps, encrypted environment variables, deployments, logs, rollback events, agent sessions, and webhook events in PostgreSQL.

Deployment flow:

1. Manual deploy or GitHub push creates a deployment record.
2. API signs a deploy job and sends it to the online local or remote agent.
3. Agent verifies the signature, clones or pulls the repo, builds the Docker image, and starts a new container.
4. Agent health-checks the new container.
5. If healthy, the local agent reports the loopback URL. A remote VPS agent updates the Caddy route and reports success.
6. If unhealthy, agent removes only the failed new container and leaves the current app running.

Rollback flow:

1. API finds the previous successful deployment.
2. API sends a signed rollback job.
3. Agent updates Caddy routing to the previous container.
4. If routing fails, the current working app remains unchanged.
