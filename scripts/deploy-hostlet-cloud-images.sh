#!/usr/bin/env bash
set -euo pipefail

ROOT="${HOSTLET_ROOT:-/srv/hostlet}"
ENV_FILE="${HOSTLET_ENV_FILE:-${ROOT}/.env}"
COMPOSE_FILE="${HOSTLET_COMPOSE_FILE:-${ROOT}/infra/docker-compose.prod.yml}"
PUBLIC_URL="${HOSTLET_PUBLIC_URL:-https://hostlet.cloud}"
SERVICES=(api web local-agent)

cd "${ROOT}"

if [ ! -f "${ENV_FILE}" ]; then
  echo "Missing env file: ${ENV_FILE}" >&2
  exit 1
fi

if [ ! -f "${COMPOSE_FILE}" ]; then
  echo "Missing compose file: ${COMPOSE_FILE}" >&2
  exit 1
fi

HOSTLET_IMAGE_TAG="$(grep -E '^HOSTLET_IMAGE_TAG=' "${ENV_FILE}" | tail -1 | cut -d= -f2- | sed -e 's/^"//' -e 's/"$//' || true)"
if [[ ! "${HOSTLET_IMAGE_TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "HOSTLET_IMAGE_TAG must be set in ${ENV_FILE} to a Hostlet release tag like v0.4.0" >&2
  exit 1
fi

compose() {
  docker compose --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" "$@"
}

image_revision() {
  local container_id="$1"
  local revision
  revision="$(docker inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "${container_id}" 2>/dev/null || true)"
  if [ "${revision}" = "<no value>" ]; then
    revision=""
  fi
  printf '%s' "${revision}"
}

echo "Current Hostlet service images:"
for service in "${SERVICES[@]}"; do
  container_id="$(compose ps -q "${service}" || true)"
  if [ -n "${container_id}" ]; then
    image_ref="$(docker inspect -f '{{.Config.Image}}' "${container_id}")"
    revision="$(image_revision "${container_id}")"
    echo "  ${service}: ${image_ref}${revision:+ (${revision})}"
  else
    echo "  ${service}: not running"
  fi
done

echo "Pulling Hostlet ${HOSTLET_IMAGE_TAG} images..."
compose pull "${SERVICES[@]}"

echo "Restarting Hostlet services without building on the VM..."
compose up -d --no-build "${SERVICES[@]}"

echo "Verifying containers..."
compose ps "${SERVICES[@]}"

echo "Verifying public health..."
for attempt in $(seq 1 30); do
  if curl -fsS "${PUBLIC_URL%/}/health" >/dev/null; then
    echo "Health check passed: ${PUBLIC_URL%/}/health"
    break
  fi
  if [ "${attempt}" = "30" ]; then
    echo "Health check failed: ${PUBLIC_URL%/}/health" >&2
    compose logs --tail=120 api web
    exit 1
  fi
  sleep 2
done

if curl -fsS "${PUBLIC_URL%/}/pricing" >/dev/null; then
  echo "Pricing page check passed: ${PUBLIC_URL%/}/pricing"
else
  echo "Pricing page check failed: ${PUBLIC_URL%/}/pricing" >&2
  compose logs --tail=120 web
  exit 1
fi

echo "Deployed image references:"
for service in "${SERVICES[@]}"; do
  container_id="$(compose ps -q "${service}")"
  image_ref="$(docker inspect -f '{{.Config.Image}}' "${container_id}")"
  revision="$(image_revision "${container_id}")"
  echo "  ${service}: ${image_ref}${revision:+ (${revision})}"
done

cat <<'EOF'

Rollback:
  1. Set HOSTLET_IMAGE_TAG in /srv/hostlet/.env to a previous vX.Y.Z tag.
  2. Re-run scripts/deploy-hostlet-cloud-images.sh.
  3. After recovery, keep cloud and self-host promotion on tagged releases.
EOF
