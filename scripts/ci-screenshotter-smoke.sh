#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${HOSTLET_SCREENSHOTTER_TEST_IMAGE:-hostlet-screenshotter-ci}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/hostlet-screenshotter-smoke.XXXXXX")"
SMOKE_CONTAINER="hostlet-screenshotter-smoke-$$"
REDIRECT_CONTAINER="hostlet-screenshotter-redirect-$$"
FIXTURE_CONTAINER="hostlet-screenshotter-fixture-$$"
ROUTER_CONTAINER="hostlet-screenshotter-router-$$"
trap 'docker rm -f "${SMOKE_CONTAINER}" "${REDIRECT_CONTAINER}" "${FIXTURE_CONTAINER}" "${ROUTER_CONTAINER}" >/dev/null 2>&1 || true; rm -rf "${TMP_DIR}"' EXIT

if [ "${HOSTLET_SCREENSHOTTER_SKIP_BUILD:-0}" != "1" ]; then
  "${ROOT}/scripts/ci-docker-retry.sh" docker build -f "${ROOT}/apps/screenshotter/Dockerfile" -t "${IMAGE}" "${ROOT}"
fi

SMOKE_URL="$(python3 - <<'PY'
from urllib.parse import quote

html = """<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <style>
      body {
        margin: 0;
        min-height: 720px;
        font: 32px Arial, sans-serif;
        color: #052e2b;
        background:
          radial-gradient(circle at 18% 20%, #34d399 0 14%, transparent 15%),
          radial-gradient(circle at 82% 16%, #60a5fa 0 12%, transparent 13%),
          linear-gradient(135deg, #f8fafc 0%, #e0f2fe 42%, #d1fae5 100%);
      }
      main {
        padding: 72px;
      }
      h1 {
        margin: 0 0 24px;
        max-width: 760px;
        font-size: 72px;
        line-height: 0.95;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 22px;
        margin-top: 48px;
      }
      .card {
        min-height: 210px;
        border: 1px solid rgba(15, 23, 42, 0.14);
        border-radius: 18px;
        background: rgba(255, 255, 255, 0.78);
        box-shadow: 0 18px 45px rgba(15, 23, 42, 0.12);
        padding: 24px;
      }
      .bar {
        height: 18px;
        border-radius: 999px;
        margin-top: 18px;
        background: linear-gradient(90deg, #059669, #2563eb, #f59e0b);
      }
    </style>
  </head>
  <body>
    <main>
      <h1>Hostlet screenshotter smoke</h1>
      <p>Styled capture probe with enough visual entropy to exercise the production byte floor.</p>
      <section class="grid">
        <div class="card">Styled page<div class="bar"></div></div>
        <div class="card">Decoded layout<div class="bar"></div></div>
        <div class="card">Readable proof<div class="bar"></div></div>
      </section>
    </main>
  </body>
</html>"""

print("data:text/html," + quote(html))
PY
)"

# Keep the output in the container's writable layer and copy it out after the
# process exits. A bind mount here is unreliable on containerized CI runners:
# the Docker daemon may resolve the host path outside the runner container,
# where chmod from the job does not affect the mounted directory.
docker create --name "${SMOKE_CONTAINER}" \
  -e HOSTLET_BROWSER_SMOKE=1 \
  "${IMAGE}" \
  "${SMOKE_URL}" \
  /tmp/screenshot.webp >/dev/null
docker start --attach "${SMOKE_CONTAINER}"
docker cp "${SMOKE_CONTAINER}:/tmp/screenshot.webp" "${TMP_DIR}/screenshot.webp"
docker rm "${SMOKE_CONTAINER}" >/dev/null

python3 - "${TMP_DIR}/screenshot.webp" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = path.read_bytes()
if len(data) < 128 or not (data.startswith(b"RIFF") and data[8:12] == b"WEBP"):
    raise SystemExit("screenshotter did not produce a WebP")
PY

echo "screenshotter smoke passed"

FIXTURE_PORT=""
for port in 18090 18091 18092 18093 18094; do
  docker rm -f "${FIXTURE_CONTAINER}" >/dev/null 2>&1 || true
  if ! docker run -d --rm --network host --name "${FIXTURE_CONTAINER}" \
    --entrypoint node \
    "${IMAGE}" \
    -e "require('http').createServer((req, res) => { if (req.url === '/challenge') { res.writeHead(403, { 'Content-Type': 'text/html', 'cf-mitigated': 'challenge' }); res.end('<!doctype html><title>Checking your browser</title>'); } else if (req.url === '/error') { res.writeHead(503, { 'Content-Type': 'text/html' }); res.end('<!doctype html><title>Unavailable</title>'); } else if (req.url === '/sparse') { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end('<!doctype html><style>html,body{margin:0;height:100%;background:#123456}</style>'); } else { res.writeHead(404); res.end(); } }).listen(${port}, '127.0.0.1');" \
    >"${TMP_DIR}/fixture-container" 2>"${TMP_DIR}/fixture-start.log"; then
    continue
  fi
  if docker run --rm --network host --entrypoint node \
    "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${port}/sparse', (res) => process.exit(res.statusCode === 200 ? 0 : 1)).on('error', () => process.exit(1));" \
    >/dev/null 2>&1; then
    FIXTURE_PORT="${port}"
    break
  fi
done

if [ -z "${FIXTURE_PORT}" ]; then
  echo "screenshotter fixture server did not start on a Chromium-safe port"
  cat "${TMP_DIR}/fixture-start.log" 2>/dev/null || true
  exit 1
fi

if docker run --rm --network host "${IMAGE}" \
  "http://127.0.0.1:${FIXTURE_PORT}/challenge" /tmp/challenge.webp \
  >"${TMP_DIR}/challenge.log" 2>&1; then
  echo "screenshotter accepted a Cloudflare challenge response"
  cat "${TMP_DIR}/challenge.log"
  exit 1
fi
grep -q "cf-mitigated: challenge" "${TMP_DIR}/challenge.log"

if docker run --rm --network host "${IMAGE}" \
  "http://127.0.0.1:${FIXTURE_PORT}/error" /tmp/http-error.webp \
  >"${TMP_DIR}/http-error.log" 2>&1; then
  echo "screenshotter accepted an HTTP error response"
  cat "${TMP_DIR}/http-error.log"
  exit 1
fi
grep -q "navigation returned HTTP 503" "${TMP_DIR}/http-error.log"

if ! docker run --rm --network host \
  -e HOSTLET_SCREENSHOT_MIN_BYTES=100000 \
  "${IMAGE}" "http://127.0.0.1:${FIXTURE_PORT}/sparse" /tmp/sparse-manual.webp \
  >"${TMP_DIR}/sparse-manual.log" 2>&1; then
  echo "manual capture rejected a sparse but visually ready page"
  cat "${TMP_DIR}/sparse-manual.log"
  exit 1
fi
grep -q "retaining the manual capture" "${TMP_DIR}/sparse-manual.log"

if docker run --rm --network host \
  -e HOSTLET_BROWSER_SMOKE=1 \
  -e HOSTLET_SCREENSHOT_MIN_BYTES=100000 \
  "${IMAGE}" "http://127.0.0.1:${FIXTURE_PORT}/sparse" /tmp/sparse-smoke.webp \
  >"${TMP_DIR}/sparse-smoke.log" 2>&1; then
  echo "browser smoke accepted a sparse page below the byte floor"
  cat "${TMP_DIR}/sparse-smoke.log"
  exit 1
fi
grep -q "blank or near-blank" "${TMP_DIR}/sparse-smoke.log"

echo "screenshotter challenge and sparse-page regressions passed"

BLANK_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><style>html,body{margin:0;height:100%;background:#173746;color:#fff;font:16px Arial}.hud{padding:20px}</style><div class=hud>CONNECTING...</div>"""
print("data:text/html," + quote(html))
PY
)"
if docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${BLANK_URL}" /tmp/blank.webp \
  >"${TMP_DIR}/blank.log" 2>&1; then
  echo "browser smoke accepted a blank loading shell"
  exit 1
fi
grep -q "blank or near-blank" "${TMP_DIR}/blank.log"

RUNTIME_ERROR_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><style>body{background:linear-gradient(45deg,#123,#acf);min-height:720px}</style><script>setTimeout(()=>{throw new Error('startup exploded')},0)</script>"""
print("data:text/html," + quote(html))
PY
)"
if docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${RUNTIME_ERROR_URL}" /tmp/error.webp \
  >"${TMP_DIR}/runtime-error.log" 2>&1; then
  echo "browser smoke accepted an uncaught page error"
  exit 1
fi
grep -q "uncaught page error" "${TMP_DIR}/runtime-error.log"

CAUGHT_EVAL_URL="$(python3 - <<'PY'
from urllib.parse import quote
html = """<!doctype html><meta http-equiv="Content-Security-Policy" content="script-src 'nonce-hostlet'"><style>html,body{margin:0;background:#e0f2fe}canvas{width:100%;height:720px}</style><canvas width=1280 height=720></canvas><script nonce=hostlet>try{new Function('return 1')()}catch{}const c=document.querySelector('canvas'),x=c.getContext('2d');for(let y=0;y<720;y+=8){for(let i=0;i<1280;i+=8){x.fillStyle=`hsl(${(i*17+y*29)%360} 75% ${35+(i*y)%45}%)`;x.fillRect(i,y,8,8)}}</script>"""
print("data:text/html," + quote(html))
PY
)"
docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" "${CAUGHT_EVAL_URL}" /tmp/caught-eval.webp \
  >"${TMP_DIR}/caught-eval.log" 2>&1

if ! docker run --rm -e HOSTLET_BROWSER_SMOKE=1 "${IMAGE}" \
  "data:application/json,%7B%22ok%22%3Atrue%7D" /tmp/non-html.webp \
  >"${TMP_DIR}/non-html.log" 2>&1; then
  echo "browser smoke did not skip a non-HTML endpoint"
  cat "${TMP_DIR}/non-html.log"
  exit 1
fi
grep -q "HOSTLET_BROWSER_SMOKE_SKIPPED_NON_HTML" "${TMP_DIR}/non-html.log"

echo "browser smoke readiness regressions passed"

# Router-origin integration fixture. The screenshotter receives the public
# canonical HTTPS URL (edge port) but must serve every canonical request from
# the private HTTP router port, with the canonical Host preserved. The edge
# listener intentionally returns 500 and records hits so a public-edge fallback
# is detectable.
ROUTER_INTERNAL_PORT=""
ROUTER_EDGE_PORT=""
for edge_port in 18110 18112 18114 18116 18118; do
  router_port=$((edge_port + 1))
  docker rm -f "${ROUTER_CONTAINER}" >/dev/null 2>&1 || true
  if ! docker run -d --rm --network host --name "${ROUTER_CONTAINER}" \
    --entrypoint node \
    "${IMAGE}" \
    -e "const http=require('http'); const edge=${edge_port}; const internal=${router_port}; const expectedHost='canonical.test:'+edge; const state={internal:0,edge:0,badHost:0,badPost:0,paths:[]}; const handler=(req,res)=>{ const isEdge=req.socket.localPort===edge; if(isEdge) state.edge++; else state.internal++; state.paths.push((isEdge?'edge':'internal')+':'+req.url); if(!isEdge && req.url!=='/stats' && req.headers.host!==expectedHost) state.badHost++; if(isEdge){res.writeHead(500,{'Content-Type':'text/html'}); return res.end('<!doctype html><title>public edge hit</title>');} if(req.url==='/redirect'){res.writeHead(302,{Location:'http://canonical.test:'+internal+'/final'}); return res.end();} if(req.url==='/final'){res.writeHead(200,{'Content-Type':'text/html'}); return res.end('<!doctype html><style>html,body{margin:0;min-height:720px;background:linear-gradient(135deg,#052e16,#0ea5e9);color:#fff;font:32px Arial,sans-serif}main{padding:72px}img{display:block;width:560px;height:180px;margin-top:30px;border:8px solid #fff;image-rendering:pixelated}#api{margin-top:20px}</style><main><h1>Canonical router fixture</h1><p>canonical origin and Host routing</p><img src=\"https://canonical.test:'+edge+'/asset-redirect\"><div id=api>loading</div><div id=post>loading</div><script>if(location.origin!==\"https://canonical.test:'+edge+'\") throw new Error(\"canonical origin lost\"); fetch(\"https://canonical.test:'+edge+'/api/data\").then(r=>r.json()).then(v=>{document.querySelector(\"#api\").textContent=v.value}); fetch(\"https://canonical.test:'+edge+'/api/echo\",{method:\"POST\",headers:{\"Content-Type\":\"application/json\"},body:JSON.stringify({probe:true})}).then(r=>r.json()).then(v=>{document.querySelector(\"#post\").textContent=v.value})</script></main>');} if(req.url==='/asset-redirect'){res.writeHead(302,{Location:'http://canonical.test:'+internal+'/asset.png'}); return res.end();} if(req.url==='/asset.png'){res.writeHead(200,{'Content-Type':'image/png'}); return res.end(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z1ZkAAAAASUVORK5CYII=','base64'));} if(req.url==='/api/data'){res.writeHead(200,{'Content-Type':'application/json'}); return res.end(JSON.stringify({value:'absolute HTTPS API served internally'}));} if(req.url==='/api/echo'){let body='';req.on('data',chunk=>body+=chunk);req.on('end',()=>{if(req.method!=='POST'||body!=='{\"probe\":true}')state.badPost++;res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({value:'POST body served internally'}))});return;} if(req.url==='/ssrf'){res.writeHead(302,{Location:'http://127.0.0.1:18109/private'}); return res.end();} if(req.url==='/fail'){res.writeHead(503,{'Content-Type':'text/html'}); return res.end('<!doctype html><title>internal failure</title>');} if(req.url==='/stats'){res.writeHead(200,{'Content-Type':'application/json'}); return res.end(JSON.stringify(state));} res.writeHead(404); res.end(); }; http.createServer(handler).listen(edge,'127.0.0.1'); http.createServer(handler).listen(internal,'127.0.0.1');" \
    >"${TMP_DIR}/router-container" 2>"${TMP_DIR}/router-start.log"; then
    continue
  fi
  if docker run --rm --network host --add-host canonical.test:127.0.0.1 --entrypoint node \
    "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${router_port}/stats',(res)=>process.exit(res.statusCode===200?0:1)).on('error',()=>process.exit(1));" \
    >/dev/null 2>&1; then
    ROUTER_INTERNAL_PORT="${router_port}"
    ROUTER_EDGE_PORT="${edge_port}"
    break
  fi
done

if [ -z "${ROUTER_INTERNAL_PORT}" ]; then
  echo "screenshotter router fixture server did not start on a Chromium-safe port"
  cat "${TMP_DIR}/router-start.log" 2>/dev/null || true
  exit 1
fi

ROUTER_TARGET="https://canonical.test:${ROUTER_EDGE_PORT}/redirect"
ROUTER_RUN_ARGS=(--rm --network host --add-host canonical.test:127.0.0.1 \
  -e HOSTLET_BROWSER_SMOKE=1 -e HOSTLET_SCREENSHOT_MIN_BYTES=1000 \
  -e "HOSTLET_SCREENSHOT_ROUTER_PORT=${ROUTER_INTERNAL_PORT}")

if ! docker run "${ROUTER_RUN_ARGS[@]}" "${IMAGE}" "${ROUTER_TARGET}" /tmp/router.webp \
  >"${TMP_DIR}/router-success.log" 2>&1; then
  echo "screenshotter internal-origin router capture failed"
  cat "${TMP_DIR}/router-success.log"
  exit 1
fi

if docker run "${ROUTER_RUN_ARGS[@]}" "${IMAGE}" \
  "https://canonical.test:${ROUTER_EDGE_PORT}/fail" /tmp/router-fail.webp \
  >"${TMP_DIR}/router-failure.log" 2>&1; then
  echo "screenshotter accepted an internal router HTTP 503"
  cat "${TMP_DIR}/router-failure.log"
  exit 1
fi
grep -q "navigation returned HTTP 503" "${TMP_DIR}/router-failure.log"

# A failed capture must not poison routing state; the canonical page succeeds
# again, and the edge remains untouched throughout every attempt.
if ! docker run "${ROUTER_RUN_ARGS[@]}" "${IMAGE}" \
  "https://canonical.test:${ROUTER_EDGE_PORT}/final" /tmp/router-recovery.webp \
  >"${TMP_DIR}/router-recovery.log" 2>&1; then
  echo "screenshotter internal-origin router did not recover after a failed capture"
  cat "${TMP_DIR}/router-recovery.log"
  exit 1
fi

if docker run "${ROUTER_RUN_ARGS[@]}" "${IMAGE}" \
  "https://canonical.test:${ROUTER_EDGE_PORT}/ssrf" /tmp/router-ssrf.webp \
  >"${TMP_DIR}/router-ssrf.log" 2>&1; then
  echo "screenshotter followed an internal-origin SSRF redirect"
  cat "${TMP_DIR}/router-ssrf.log"
  exit 1
fi
grep -q "blocked request to" "${TMP_DIR}/router-ssrf.log"

ROUTER_STATS="$(docker run --rm --network host --add-host canonical.test:127.0.0.1 --entrypoint node \
  "${IMAGE}" \
  -e "require('http').get('http://127.0.0.1:${ROUTER_INTERNAL_PORT}/stats',(res)=>{let d='';res.on('data',(x)=>d+=x);res.on('end',()=>process.stdout.write(d))}).on('error',()=>process.exit(1));")"
python3 - "${ROUTER_STATS}" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats.get("edge") != 0:
    raise SystemExit(f"router fixture observed public-edge hits: {stats}")
if stats.get("badHost") != 0:
    raise SystemExit(f"router fixture observed invalid Host routing: {stats}")
if stats.get("badPost") != 0:
    raise SystemExit(f"router fixture lost the canonical POST method or body: {stats}")
if not any(path.endswith(":/asset-redirect") for path in stats.get("paths", [])):
    raise SystemExit(f"router fixture did not receive absolute HTTPS asset: {stats}")
if not any(path.endswith(":/api/data") for path in stats.get("paths", [])):
    raise SystemExit(f"router fixture did not receive absolute HTTPS API call: {stats}")
if not any(path.endswith(":/api/echo") for path in stats.get("paths", [])):
    raise SystemExit(f"router fixture did not receive canonical HTTPS POST: {stats}")
PY

echo "screenshotter internal-origin router, Host, redirect, SSRF, edge-isolation, and recovery regressions passed"

# Redirect Set-Cookie values from the private router must be visible on the
# next canonical redirect request, with browser URL/path/Secure matching.
COOKIE_EDGE_PORT=18130
COOKIE_ROUTER_PORT=18131
docker rm -f "${ROUTER_CONTAINER}" >/dev/null 2>&1 || true
docker run -d --rm --network host --name "${ROUTER_CONTAINER}" \
  --entrypoint node \
  "${IMAGE}" \
  -e "const http=require('http'); const edge=${COOKIE_EDGE_PORT}; const internal=${COOKIE_ROUTER_PORT}; const expectedHost='cookie.test:'+edge; const state={badCookie:0,edge:0}; const handler=(req,res)=>{if(req.socket.localPort===edge)state.edge++; if(req.url==='/stats'){res.writeHead(200,{'Content-Type':'application/json'}); return res.end(JSON.stringify(state));} if(req.socket.localPort===edge){res.writeHead(500); return res.end('edge');} if(req.url==='/redirect'){res.writeHead(302,{Location:'http://cookie.test:'+internal+'/final','Set-Cookie':'redirect_cookie=ready; Path=/; Secure; SameSite=Lax'}); return res.end();} if(req.url==='/final'){if(!String(req.headers.cookie||'').includes('redirect_cookie=ready'))state.badCookie++; res.writeHead(200,{'Content-Type':'text/html'}); return res.end('<!doctype html><style>html,body{margin:0;min-height:720px;background:linear-gradient(135deg,#082f49,#22d3ee);color:#fff;font:32px Arial}main{padding:72px}</style><main><h1>Redirect cookie fixture</h1><p>cookie replay</p></main>');} res.writeHead(404); res.end();}; http.createServer(handler).listen(edge,'127.0.0.1'); http.createServer(handler).listen(internal,'127.0.0.1');" \
  >"${TMP_DIR}/router-cookie-container" 2>"${TMP_DIR}/router-cookie-start.log"
COOKIE_RUN_ARGS=(--rm --network host --add-host cookie.test:127.0.0.1 \
  -e HOSTLET_BROWSER_SMOKE=1 -e HOSTLET_SCREENSHOT_MIN_BYTES=1000 \
  -e "HOSTLET_SCREENSHOT_ROUTER_PORT=${COOKIE_ROUTER_PORT}")
if ! docker run "${COOKIE_RUN_ARGS[@]}" "${IMAGE}" \
  "https://cookie.test:${COOKIE_EDGE_PORT}/redirect" /tmp/router-cookie.webp \
  >"${TMP_DIR}/router-cookie.log" 2>&1; then
  echo "screenshotter internal redirect-cookie capture failed"
  cat "${TMP_DIR}/router-cookie.log"
  exit 1
fi
COOKIE_STATS="$(docker run --rm --network host --add-host cookie.test:127.0.0.1 --entrypoint node "${IMAGE}" \
  -e "require('http').get('http://127.0.0.1:${COOKIE_ROUTER_PORT}/stats',(res)=>{let d='';res.on('data',(x)=>d+=x);res.on('end',()=>process.stdout.write(d))}).on('error',()=>process.exit(1));")"
python3 - "${COOKIE_STATS}" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats.get("badCookie") != 0 or stats.get("edge") != 0:
    raise SystemExit(f"redirect cookie was not replayed internally: {stats}")
PY
echo "screenshotter internal redirect-cookie replay regression passed"

# WebSocketRoute does not expose the browser handshake Origin or resolved peer
# address, so router-mode WebSockets intentionally fail closed. Keep the old
# forwarding fixture disabled until Playwright exposes those semantics.
if false; then
docker rm -f "${ROUTER_CONTAINER}" >/dev/null 2>&1 || true
docker run -d --rm --network host --name "${ROUTER_CONTAINER}" \
  --entrypoint node \
  "${IMAGE}" \
  -e "const http=require('http'); const net=require('net'); const {WebSocketServer}=require('ws'); const edge=${ROUTER_EDGE_PORT}; const internal=${ROUTER_INTERNAL_PORT}; const expectedHost='canonical.test:'+edge; const expectedOrigin='https://'+expectedHost; const state={edge:0,ws:0,badHost:0,badWs:0}; const png='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Z1ZkAAAAASUVORK5CYII='; const server=http.createServer((req,res)=>{ if(req.url==='/stats'){res.writeHead(200,{'Content-Type':'application/json'}); return res.end(JSON.stringify(state));} if(req.headers.host!==expectedHost) state.badHost++; res.writeHead(200,{'Content-Type':'text/html'}); res.end('<!doctype html><style>html,body{margin:0;min-height:720px;background:linear-gradient(135deg,#172554,#7e22ce);color:white;font:32px Arial}main{padding:72px}img{width:320px;height:180px;image-rendering:pixelated}</style><main><h1>Internal WebSocket proof</h1><div id=status>connecting</div><img id=proof><script>document.cookie=\"router_cookie=ready; Secure; SameSite=Lax; Path=/\"; const socket=new WebSocket(\"wss://canonical.test:'+edge+'/ws\",\"hostlet-v1\"); socket.onopen=()=>socket.send(\"ping\"); socket.onmessage=event=>{if(event.data!==\"pong\")throw new Error(\"bad websocket response\");document.querySelector(\"#status\").textContent=\"connected\";document.querySelector(\"#proof\").src=\"data:image/png;base64,'+png+'\"}</script></main>'); }); const wss=new WebSocketServer({noServer:true}); server.on('upgrade',(req,socket,head)=>{ const protocol=req.headers['sec-websocket-protocol']||''; const cookie=req.headers.cookie||''; if(req.headers.host!==expectedHost||req.headers.origin!==expectedOrigin||protocol!=='hostlet-v1'||!cookie.includes('router_cookie=ready'))state.badWs++; wss.handleUpgrade(req,socket,head,ws=>{state.ws++;ws.on('message',message=>{if(message.toString()==='ping')ws.send('pong')})})}); server.listen(internal,'127.0.0.1'); net.createServer(socket=>{state.edge++;socket.destroy()}).listen(edge,'127.0.0.1');" \
  >"${TMP_DIR}/router-ws-container" 2>"${TMP_DIR}/router-ws-start.log"

for _ in 1 2 3 4 5; do
  if docker run --rm --network host --entrypoint node "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${ROUTER_INTERNAL_PORT}/stats',(res)=>process.exit(res.statusCode===200?0:1)).on('error',()=>process.exit(1));" \
    >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! docker run "${ROUTER_RUN_ARGS[@]}" "${IMAGE}" \
  "https://canonical.test:${ROUTER_EDGE_PORT}/" /tmp/router-ws.webp \
  >"${TMP_DIR}/router-ws.log" 2>&1; then
  echo "screenshotter canonical WebSocket forwarding failed"
  cat "${TMP_DIR}/router-ws.log"
  exit 1
fi

ROUTER_WS_STATS="$(docker run --rm --network host --entrypoint node "${IMAGE}" \
  -e "require('http').get('http://127.0.0.1:${ROUTER_INTERNAL_PORT}/stats',(res)=>{let d='';res.on('data',(x)=>d+=x);res.on('end',()=>process.stdout.write(d))}).on('error',()=>process.exit(1));")"
python3 - "${ROUTER_WS_STATS}" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats.get("badHost") != 0 or stats.get("badWs") != 0:
    raise SystemExit(f"WebSocket fixture lost Host, Origin, cookie, or subprotocol: {stats}")
if stats.get("ws", 0) < 1:
    raise SystemExit(f"WebSocket fixture was not reached: {stats}")
PY

echo "screenshotter canonical WebSocket origin, cookie, subprotocol, and DNS-pinned routing regression passed"
fi
echo "screenshotter router-mode WebSocket fail-closed regression policy applied"

# Exercise that policy: a canonical WSS request must be closed by Playwright
# before Chromium reaches either the public-edge socket or the internal HTTP
# router. The page handles the expected socket error and remains capturable.
WS_EDGE_PORT=18140
WS_ROUTER_PORT=18141
docker rm -f "${ROUTER_CONTAINER}" >/dev/null 2>&1 || true
docker run -d --rm --network host --name "${ROUTER_CONTAINER}" \
  --entrypoint node \
  "${IMAGE}" \
  -e "const http=require('http');const state={edge:0,edgeUpgrade:0,routerUpgrade:0};const host='canonical-ws.test:'+${WS_EDGE_PORT};const handler=(req,res)=>{if(req.url==='/stats'){res.writeHead(200,{'Content-Type':'application/json'});return res.end(JSON.stringify(state));}if(req.headers.host!==host){res.writeHead(400);return res.end('bad host');}res.writeHead(200,{'Content-Type':'text/html'});res.end('<!doctype html><style>html,body{margin:0;min-height:720px;background:#172554;color:#fff;font:32px Arial}main{padding:72px}</style><main><h1>WebSocket fail closed</h1><p id=status>waiting</p><script>const ws=new WebSocket(\"ws://'+host+'/ws\");ws.onerror=()=>document.querySelector(\"#status\").textContent=\"blocked safely\"</script></main>')};const router=http.createServer(handler);router.on('upgrade',(_req,socket)=>{state.routerUpgrade++;socket.destroy()});router.listen(${WS_ROUTER_PORT},'127.0.0.1');const edge=http.createServer((_req,res)=>{state.edge++;res.writeHead(500);res.end('edge')});edge.on('upgrade',(_req,socket)=>{state.edgeUpgrade++;socket.destroy()});edge.listen(${WS_EDGE_PORT},'127.0.0.1');" \
  >"${TMP_DIR}/router-ws-block-container" 2>"${TMP_DIR}/router-ws-block-start.log"
for _ in 1 2 3 4 5; do
  if docker run --rm --network host --entrypoint node "${IMAGE}" \
    -e "require('http').get('http://127.0.0.1:${WS_ROUTER_PORT}/stats',(res)=>process.exit(res.statusCode===200?0:1)).on('error',()=>process.exit(1));" \
    >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
if ! docker run --rm --network host --add-host canonical-ws.test:127.0.0.1 \
  -e HOSTLET_BROWSER_SMOKE=1 -e HOSTLET_SCREENSHOT_MIN_BYTES=1000 \
  -e "HOSTLET_SCREENSHOT_ROUTER_PORT=${WS_ROUTER_PORT}" \
  "${IMAGE}" "http://canonical-ws.test:${WS_EDGE_PORT}/" /tmp/router-ws-block.webp \
  >"${TMP_DIR}/router-ws-block.log" 2>&1; then
  echo "screenshotter failed while blocking a router-mode WebSocket"
  cat "${TMP_DIR}/router-ws-block.log"
  exit 1
fi
WS_BLOCK_STATS="$(docker run --rm --network host --entrypoint node "${IMAGE}" \
  -e "require('http').get('http://127.0.0.1:${WS_ROUTER_PORT}/stats',(res)=>{let d='';res.on('data',x=>d+=x);res.on('end',()=>process.stdout.write(d))}).on('error',()=>process.exit(1));")"
python3 - "${WS_BLOCK_STATS}" <<'PY'
import json
import sys

stats = json.loads(sys.argv[1])
if stats.get("edge") != 0 or stats.get("edgeUpgrade") != 0 or stats.get("routerUpgrade") != 0:
    raise SystemExit(f"router-mode WebSocket completed a handshake: {stats}")
PY
echo "screenshotter router-mode WebSocket fail-closed handshake regression passed"

run_redirect_block_test() {
  local label="$1"
  local location="$2"
  local redirect_port=""
  for port in 18080 18082 18083 18084 18085; do
    docker rm -f "${REDIRECT_CONTAINER}" >/dev/null 2>&1 || true
    if ! docker run -d --rm --network host --name "${REDIRECT_CONTAINER}" \
      --entrypoint node \
      "${IMAGE}" \
      -e "require('http').createServer((_, res) => { res.writeHead(302, { Location: '${location}' }); res.end(); }).listen(${port}, '127.0.0.1');" \
      > "${TMP_DIR}/redirect-container" 2> "${TMP_DIR}/redirect-start.log"; then
      continue
    fi
    if docker run --rm --network host --entrypoint node \
      "${IMAGE}" \
      -e "require('http').get('http://127.0.0.1:${port}/', (res) => process.exit(res.statusCode === 302 ? 0 : 1)).on('error', () => process.exit(1));" \
      >/dev/null 2>&1; then
      redirect_port="${port}"
      break
    fi
  done

  if [ -z "${redirect_port}" ]; then
    echo "redirect server did not start on a Chromium-safe port"
    cat "${TMP_DIR}/redirect-start.log" 2>/dev/null || true
    exit 1
  fi

  if docker run --rm --network host \
    "${IMAGE}" \
    "http://127.0.0.1:${redirect_port}/" \
    /out/blocked.jpg > "${TMP_DIR}/ssrf-${label}.log" 2>&1; then
    echo "SSRF regression: screenshotter followed ${label} redirect"
    cat "${TMP_DIR}/ssrf-${label}.log"
    exit 1
  fi

  if ! grep -q "blocked request to" "${TMP_DIR}/ssrf-${label}.log"; then
    echo "SSRF regression: expected 'blocked request to' marker not found for ${label}"
    cat "${TMP_DIR}/ssrf-${label}.log"
    exit 1
  fi
}

# SSRF regression: a tenant app can 302-redirect the host-networked browser to a
# host-local origin. The target origins are allowed, but the redirect hop to a
# different loopback origin must be blocked. --network host is intentional here
# so the negative test exercises the real production posture.
run_redirect_block_test "loopback" "http://127.0.0.1:18081/"
run_redirect_block_test "mapped-ipv6" "http://[::ffff:7f00:1]:18081/"

echo "screenshotter SSRF regression passed"
