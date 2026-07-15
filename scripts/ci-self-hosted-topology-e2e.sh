deploy_generated_topology_app() {
  local create app_id deploy deployment_id detail frontend_port backend_port route_file delete job client_name server_name topology_config
  client_name="@hostlet-topology/client"
  server_name="@hostlet-topology/server"
  topology_config='{"schemaVersion":1,"mode":"auto","backendPathPrefixes":["/api","/socket.io"]}'
  if [ -n "${HOSTLET_TOPOLOGY_CANARY_REPO:-}" ]; then
    client_name="@patchwork/client"
    server_name="@patchwork/server"
    topology_config='{"schemaVersion":1,"mode":"selected","frontendSelector":"node:packages/client/package.json:@patchwork/client","backendSelector":"node:packages/server/package.json:@patchwork/server","backendPathPrefixes":["/api","/socket.io"]}'
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
    curl --retry 10 --retry-delay 1 --retry-all-errors -fsS "http://127.0.0.1:${frontend_port}/" | grep -q 'patchwork-v1'
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
  delete="$(curl -fsS -H "cookie: ${AUTH_COOKIE}" "${ORIGIN_CSRF[@]}" "${JSON_CT[@]}" -X DELETE "${BASE_URL}/api/apps/${app_id}")"
  job="$(printf '%s' "${delete}" | json_get jobId)"
  wait_job_status "${job}"
  expect_status 404 -H "cookie: ${AUTH_COOKIE}" "${BASE_URL}/api/apps/${app_id}"
  echo "generated frontend + WebSocket backend topology E2E passed"
}
