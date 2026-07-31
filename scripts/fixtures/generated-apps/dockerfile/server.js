const http = require("http");
const port = Number(process.env.PORT || 3000);

http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/plain" });
  response.end(request.url === "/health" ? "ok" : "hostlet-remote-dockerfile");
}).listen(port, "0.0.0.0");
