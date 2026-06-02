const http = require("http");

const port = Number(process.env.PORT || 3000);

http
  .createServer((req, res) => {
    if (req.url === "/health") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("ok");
      return;
    }
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("hostlet-generated-yarn");
  })
  .listen(port, "0.0.0.0");
