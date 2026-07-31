#!/usr/bin/env bash

# Shared disposable registry helpers for self-hosted deployment E2E suites.
# Callers provide ROOT, TMP_DIR, and REGISTRY_CONTAINER and own cleanup.

start_test_artifact_registry() {
  local auth_file="${TMP_DIR}/registry.htpasswd"
  local storage_dir="${TMP_DIR}/registry-data"
  local builder_password="ci-only-not-a-secret-builder-password"
  local runner_password="ci-only-not-a-secret-runner-password"
  mkdir -p "${storage_dir}"

  docker run --rm httpd:2.4-alpine \
    htpasswd -Bnb hostlet-builder "${builder_password}" >"${auth_file}"
  docker run --rm httpd:2.4-alpine \
    htpasswd -Bnb hostlet-runner "${runner_password}" >>"${auth_file}"
  chmod 0600 "${auth_file}"

  docker run -d --name "${REGISTRY_CONTAINER}" \
    --user "$(id -u):$(id -g)" \
    -p 127.0.0.1::5000 \
    -v "${ROOT}/infra/zot-config.json:/etc/zot/config.json:ro" \
    -v "${auth_file}:/etc/zot/htpasswd:ro" \
    -v "${storage_dir}:/var/lib/registry" \
    ghcr.io/project-zot/zot-linux-amd64:v2.1.18 \
    serve /etc/zot/config.json >/dev/null

  local registry_port
  registry_port="$(docker port "${REGISTRY_CONTAINER}" 5000/tcp | sed 's/.*://')"
  [ -n "${registry_port}" ] || {
    echo "could not discover test artifact registry port" >&2
    return 1
  }
  export HOSTLET_ARTIFACT_REGISTRY_URL="http://127.0.0.1:${registry_port}"
  export HOSTLET_ARTIFACT_REGISTRY_INTERNAL_URL="${HOSTLET_ARTIFACT_REGISTRY_URL}"
  export HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL="${HOSTLET_ARTIFACT_REGISTRY_URL}"
  export HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME=hostlet-builder
  export HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD="${builder_password}"
  export HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME=hostlet-runner
  export HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD="${runner_password}"

  for _ in $(seq 1 60); do
    if curl -fsS -u "hostlet-runner:${runner_password}" \
      "${HOSTLET_ARTIFACT_REGISTRY_URL}/v2/" >/dev/null 2>&1; then
      return 0
    fi
    if ! docker inspect "${REGISTRY_CONTAINER}" >/dev/null 2>&1; then
      echo "test artifact registry exited before becoming ready" >&2
      return 1
    fi
    sleep 1
  done
  docker logs "${REGISTRY_CONTAINER}" >&2 || true
  echo "timed out waiting for test artifact registry" >&2
  return 1
}
