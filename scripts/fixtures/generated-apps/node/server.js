const http = require("http");
const fs = require("fs");
const net = require("net");

const port = Number(process.env.PORT || 3000);
const version = process.env.APP_VERSION || "v1";

http.createServer((req, res) => {
  if (req.url === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok");
    return;
  }
  // /db proves multi-service wiring: open a TCP connection to the host:port in
  // the injected DATABASE_URL (a managed add-on reachable only on the internal
  // compose network). No DB driver needed — a successful connect is the proof.
  if (req.url === "/db") {
    const url = process.env.DATABASE_URL;
    if (!url) {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("db-none");
      return;
    }
    let host = "";
    let dbPort = 5432;
    try {
      const parsed = new URL(url);
      host = parsed.hostname;
      dbPort = Number(parsed.port || 5432);
    } catch {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("db-badurl");
      return;
    }
    const socket = net.connect(dbPort, host);
    let done = false;
    const finish = (body) => {
      if (done) return;
      done = true;
      socket.destroy();
      res.writeHead(200, { "content-type": "text/plain" });
      res.end(body);
    };
    socket.setTimeout(2000);
    socket.on("connect", () => finish("db-ok"));
    socket.on("timeout", () => finish("db-timeout"));
    socket.on("error", () => finish("db-fail"));
    return;
  }
  let persisted = "no-data";
  try {
    fs.mkdirSync("/data", { recursive: true });
    const marker = "/data/hostlet-ci-version";
    if (!fs.existsSync(marker)) fs.writeFileSync(marker, version);
    persisted = fs.readFileSync(marker, "utf8");
  } catch {
    persisted = "no-data";
  }
  res.writeHead(200, { "content-type": "text/plain" });
  res.end(`hostlet-ci-${version}-${persisted}`);
}).listen(port, "0.0.0.0");
