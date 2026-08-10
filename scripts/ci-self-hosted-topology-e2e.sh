deploy_generated_topology_app() {
  local create app_id deploy deployment_id detail frontend_port backend_port route_file delete job client_name server_name topology_config inspection frontend_html
  local health_before_repair checked_before_repair stale_frontend_port stale_backend_port repaired_detail ports_repaired repaired_route_hash repair_logs repair_log_count
  local health_after_repair checked_after_repair next_route_hash next_logs next_log_count
  local health_job_payload health_job_id restart_payload restart_job_id pause_payload pause_job_id resume_payload resume_job_id transition_route_hash current_port
  client_name="@hostlet-topology/client"
  server_name="@hostlet-topology/server"
  topology_config='{"schemaVersion":1,"mode":"auto","backendPathPrefixes":["/api","/socket.io"]}'
  if [ -n "${HOSTLET_TOPOLOGY_CANARY_REPO:-}" ]; then
    client_name="@patchwork/client"
    server_name="@patchwork/server"
    inspection="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/github/repo-inspect" --data "{\"repo_full_name\":\"${HOSTLET_TOPOLOGY_CANARY_REPO}\",\"branch\":\"${HOSTLET_TOPOLOGY_CANARY_SHA}\"}")"
    topology_config="$(printf '%s' "${inspection}" | python3 -c '
import json, sys
d=json.load(sys.stdin)
assert d.get("deployable") is True, d
assert d.get("runtimeKind") == "compose", d
assert d.get("rootDirectory") == ".", d
p=d.get("inferencePlan") or {}
assert p.get("readiness") == "ready", p
assert {s.get("name") for s in p.get("services", [])} == {"@patchwork/client", "@patchwork/server"}, p
cfg=(d.get("runtimeConfig") or {}).get("generatedTopology")
assert cfg and cfg.get("mode") == "auto", d
print(json.dumps(cfg, separators=(",", ":")))
')"
  fi
  create="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps" --data "$(cat <<JSON
{"name":"ci-topology-patchwork","repo_full_name":"${TOPOLOGY_REPO_FULL}","branch":"main","server_id":null,"container_port":80,"health_path":"/","domain":"patchwork.localhost","runtime_kind":"compose","hostlet_config_path":"hostlet.yml","root_directory":".","runtime_config":{"generatedTopology":${topology_config}},"memory_limit_mb":512,"cpu_limit":0.5,"public_exposure":false,"auto_deploy":false,"deploy_after_create":false,"env":[{"key":"APP_VERSION","value":"v1"}]}
JSON
)" )"
  app_id="$(printf '%s' "${create}" | json_get id)"
  CREATED_APP_IDS+=("${app_id}")
  deploy="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/deploy" --data "{\"commitSha\":\"${FIXTURE_SHAS[${TOPOLOGY_REPO_NAME}]}\"}")"
  deployment_id="$(printf '%s' "${deploy}" | json_get deploymentId)"
  wait_deployment_status "${deployment_id}"
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  read -r frontend_port backend_port < <(printf '%s' "${detail}" | python3 -c '
import json, sys
d=json.load(sys.stdin)
services={s["name"]:s for s in d.get("services", [])}
client, server=sys.argv[1:]
assert set(services) == {client, server}, services
print(services[client]["publishedPort"], services[server]["publishedPort"])
' "${client_name}" "${server_name}")
  if [ -n "${HOSTLET_TOPOLOGY_CANARY_REPO:-}" ]; then
    curl -fsS "http://127.0.0.1:${frontend_port}/" >/dev/null
    timeout 5 bash -c "</dev/tcp/127.0.0.1/${backend_port}"
  else
    frontend_html="$(curl --retry 10 --retry-delay 1 --retry-all-errors -fsS "http://127.0.0.1:${frontend_port}/")"
    printf '%s' "${frontend_html}" | grep -q 'patchwork-v1'
    printf '%s' "${frontend_html}" | grep -q 'wss://patchwork.localhost'
    curl --retry 10 --retry-delay 1 --retry-all-errors -fsS "http://127.0.0.1:${backend_port}/api/version" | grep -q '^backend-v1$'
    websocket_ok=0
    for _ in $(seq 1 5); do
      if node -e '
const ws = new WebSocket(process.argv[1]);
const timeout = setTimeout(() => { console.error(`WebSocket echo timed out for ${process.argv[1]}`); process.exit(2); }, 10000);
ws.onopen = () => ws.send("magic");
ws.onmessage = (event) => { clearTimeout(timeout); process.exit(event.data === "echo:magic" ? 0 : 3); };
ws.onerror = (event) => { console.error("WebSocket probe failed", event); process.exit(4); };
' "ws://127.0.0.1:${backend_port}/"; then
        websocket_ok=1
        break
      fi
      sleep 1
    done
    [ "${websocket_ok}" = "1" ]
  fi
  printf '%s' "${detail}" | python3 -c '
import json, sys
d=json.load(sys.stdin)
r=(d.get("latestDeployment") or {}).get("runtimeMetadata",{}).get("inferenceReceipt",{})
assert r.get("schemaVersion") == 1, r
assert r.get("repositoryModified") is False, r
assert {s.get("role") for s in r.get("services",[])} == {"frontend","backend"}, r
assert r.get("routing",{}).get("websocketsToBackend") is True, r
'
  route_file="${HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR}/app-${app_id}.caddy"
  grep -q 'header Connection \*Upgrade\*' "${route_file}"
  grep -q 'path /api /api/\* /socket.io /socket.io/\*' "${route_file}"
  grep -q "127.0.0.1:${frontend_port}" "${route_file}"
  grep -q "127.0.0.1:${backend_port}" "${route_file}"

  # Reproduce the multi-service route incident: both stored service ports are
  # stale and the shared route has been collapsed to a single backend proxy.
  # One recurring health pass must restore the split route and persist both
  # observed ports; the following pass must not rewrite it again.
  wait_app_health_checked_after "${app_id}" ""
  health_before_repair="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}/health")"
  checked_before_repair="$(printf '%s' "${health_before_repair}" | json_get lastCheckedAt)"
  stale_frontend_port=9
  stale_backend_port=10
  docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
UPDATE deployments
SET published_port=${stale_frontend_port}
WHERE id='${deployment_id}';
UPDATE deployment_services
SET published_port=CASE
  WHEN service_name='${client_name}' THEN ${stale_frontend_port}
  WHEN service_name='${server_name}' THEN ${stale_backend_port}
  ELSE published_port
END
WHERE deployment_id='${deployment_id}';
SQL
  printf '# hostlet-route-key: app-%s\n# hostlet-domain: patchwork.localhost\npatchwork.localhost {\n  reverse_proxy 127.0.0.1:%s\n}\n' \
    "${app_id}" "${stale_backend_port}" >"${route_file}"

  wait_app_health_checked_after "${app_id}" "${checked_before_repair}"
  ports_repaired=0
  for _ in $(seq 1 20); do
    repaired_detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
    if printf '%s' "${repaired_detail}" | python3 -c '
import json, sys
detail=json.load(sys.stdin)
client_name, server_name, frontend_port, backend_port=sys.argv[1:]
services={service["name"]:service for service in detail.get("services", [])}
assert int(detail["currentDeployment"]["publishedPort"]) == int(frontend_port), detail
assert int(services[client_name]["publishedPort"]) == int(frontend_port), services
assert int(services[server_name]["publishedPort"]) == int(backend_port), services
' "${client_name}" "${server_name}" "${frontend_port}" "${backend_port}" 2>/dev/null; then
      ports_repaired=1
      break
    fi
    sleep 1
  done
  if [ "${ports_repaired}" != 1 ]; then
    echo "generated topology health repair did not persist both observed ports" >&2
    printf '%s\n' "${repaired_detail}" >&2
    exit 1
  fi
  grep -q 'header Connection \*Upgrade\*' "${route_file}"
  grep -q 'path /api /api/\* /socket.io /socket.io/\*' "${route_file}"
  grep -q "127.0.0.1:${frontend_port}" "${route_file}"
  grep -q "127.0.0.1:${backend_port}" "${route_file}"
  repaired_route_hash="$(sha256sum "${route_file}" | awk '{print $1}')"
  repair_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
  repair_log_count="$(printf '%s' "${repair_logs}" | awk '{ count += gsub(/Detected Docker-published port drift for/, "") } END { print count + 0 }')"
  if [ "${repair_log_count}" -ne 1 ]; then
    echo "expected one initial generated-topology drift repair log, got ${repair_log_count}" >&2
    exit 1
  fi

  health_after_repair="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}/health")"
  checked_after_repair="$(printf '%s' "${health_after_repair}" | json_get lastCheckedAt)"
  wait_app_health_checked_after "${app_id}" "${checked_after_repair}"
  next_route_hash="$(sha256sum "${route_file}" | awk '{print $1}')"
  if [ "${next_route_hash}" != "${repaired_route_hash}" ]; then
    echo "generated-topology route changed after a no-drift health pass" >&2
    exit 1
  fi
  next_logs="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/deployments/${deployment_id}/logs")"
  next_log_count="$(printf '%s' "${next_logs}" | awk '{ count += gsub(/Detected Docker-published port drift for/, "") } END { print count + 0 }')"
  if [ "${next_log_count}" -ne "${repair_log_count}" ]; then
    echo "expected no additional generated-topology drift log without new drift; got ${next_log_count} after ${repair_log_count}" >&2
    exit 1
  fi

  # Queued interactive jobs carry an older, single-target payload shape. Give
  # each probe path a stale frontend port: the interactive job and recurring
  # health pass intentionally race, but whichever observes the drift must
  # converge through the current split writer and never collapse this route.
  docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
UPDATE deployments SET published_port=${stale_frontend_port} WHERE id='${deployment_id}';
SQL
  health_job_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/health/check-now" --data '{}')"
  health_job_id="$(printf '%s' "${health_job_payload}" | json_get jobId)"
  wait_job_status "${health_job_id}"
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  current_port="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  if [ "${current_port}" != "${frontend_port}" ]; then
    echo "health-check repair persisted frontend port ${current_port}, expected ${frontend_port}" >&2
    exit 1
  fi

  docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
UPDATE deployments SET published_port=${stale_frontend_port} WHERE id='${deployment_id}';
SQL
  restart_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/restart" --data '{}')"
  restart_job_id="$(printf '%s' "${restart_payload}" | json_get jobId)"
  wait_job_status "${restart_job_id}"
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  current_port="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  if [ "${current_port}" != "${frontend_port}" ]; then
    echo "restart repair persisted frontend port ${current_port}, expected ${frontend_port}" >&2
    exit 1
  fi

  pause_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/pause")"
  pause_job_id="$(printf '%s' "${pause_payload}" | json_get jobId)"
  wait_job_status "${pause_job_id}"
  docker exec -i "${POSTGRES_CONTAINER}" psql -U hostlet -d hostlet >/dev/null <<SQL
UPDATE deployments SET published_port=${stale_frontend_port} WHERE id='${deployment_id}';
SQL
  resume_payload="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" -X POST "${BASE_URL}/api/apps/${app_id}/resume")"
  resume_job_id="$(printf '%s' "${resume_payload}" | json_get jobId)"
  wait_job_status "${resume_job_id}"
  detail="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}")"
  current_port="$(printf '%s' "${detail}" | json_get currentDeployment.publishedPort)"
  if [ "${current_port}" != "${frontend_port}" ]; then
    echo "resume repair persisted frontend port ${current_port}, expected ${frontend_port}" >&2
    exit 1
  fi
  transition_route_hash="$(sha256sum "${route_file}" | awk '{print $1}')"
  if [ "${transition_route_hash}" != "${repaired_route_hash}" ]; then
    echo "interactive health transitions changed the repaired split route" >&2
    exit 1
  fi
  curl -fsS "http://127.0.0.1:${frontend_port}/" >/dev/null
  timeout 5 bash -c "</dev/tcp/127.0.0.1/${backend_port}"

  delete="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${app_id}")"
  job="$(printf '%s' "${delete}" | json_get jobId)"
  wait_job_status "${job}"
  expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}"
  echo "generated frontend + WebSocket backend topology E2E passed"
}
