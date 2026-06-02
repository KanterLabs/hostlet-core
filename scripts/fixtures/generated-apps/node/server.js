const http = require("http");
const fs = require("fs");

const port = Number(process.env.PORT || 3000);
const version = process.env.APP_VERSION || "v1";

http.createServer((req, res) => {
  if (req.url === "/health") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("ok");
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
