# Hostlet Cloud Migration

This public repo is the open-source Hostlet Core project. Hosted-service code belongs in a private `hostlet-cloud` repo.

## Move To Private Repo

- cloud runtime mode, cloud-only API routes, and hosted-account/session logic
- GitHub App install/auth/repository access code
- Stripe checkout, portal, subscription, webhook, usage, and plan logic
- hosted-service migrations and seed data
- hosted pricing/usage UI
- Cloudflare production tunnel config and provider credentials
- production GCP VM runbooks, deployment scripts, and private customer operations docs

The current cloud-bearing files were locally staged for migration at:

```text
/home/shane/kanterman/projects/hostlet-cloud-staging
```

Do not commit that staging directory to the public repo.

## Recommended Private Repo Shape

```text
hostlet-cloud/
  vendor/hostlet-core/        # git submodule pinned to a core tag
  apps/cloud-api/
  apps/cloud-web/
  infra/
    docker-compose.prod.yml
    env.prod.example
    gcp-prod-cutover.md
  scripts/
    deploy-hostlet-cloud-images.sh
    ci-cloud-api-smoke.sh
    ci-cloud-api-e2e.sh
  docs/
    operations.md
```

## Initial Setup

```bash
cd /home/shane/kanterman/projects
gh repo create ShaneKanterman04/hostlet-cloud --private --description "Private Hostlet Cloud hosted-service layer" --clone=false
git clone git@github.com:ShaneKanterman04/hostlet-cloud.git hostlet-cloud
cd hostlet-cloud
git submodule add git@github.com:ShaneKanterman04/hostlet-core.git vendor/hostlet-core
git submodule update --init --recursive
git commit -m "Add hostlet-core submodule"
```

## Production GCP Cutover

The production GCP VM should run the private cloud checkout from `/srv/hostlet-cloud`. Keep the first cutover on compose project name `infra` so existing Docker volumes are reused.

```bash
ssh <gcp-prod-host>
sudo mkdir -p /srv/hostlet-cloud
sudo chown "$USER:$USER" /srv/hostlet-cloud
git clone git@github.com:ShaneKanterman04/hostlet-cloud.git /srv/hostlet-cloud
cd /srv/hostlet-cloud
git submodule update --init --recursive
cp infra/env.prod.example .env
# Fill .env from the production secret source. Do not commit it.
docker compose --env-file .env -f infra/docker-compose.prod.yml -p infra config >/dev/null
docker compose --env-file .env -f infra/docker-compose.prod.yml -p infra pull
# Take a VM/disk snapshot and database backup before changing running services.
docker compose --env-file .env -f infra/docker-compose.prod.yml -p infra up -d --no-build
```

Rollback to the old checkout if needed:

```bash
cd /srv/hostlet
docker compose --env-file .env -f infra/docker-compose.prod.yml -p infra up -d --no-build
```

If private cloud migrations are forward-only, rollback requires restoring the pre-cutover database backup or VM snapshot.
