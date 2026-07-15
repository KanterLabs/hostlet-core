import { createHash } from "node:crypto";
import { createServer } from "node:http";

const version = process.env.APP_VERSION || "v1";
const port = Number(process.env.PORT || 3000);
const server = createServer((request, response) => {
  if (request.url === "/api/version") {
    response.writeHead(200, { "content-type": "text/plain" });
    response.end(`backend-${version}`);
    return;
  }
  response.writeHead(404);
  response.end("not found");
});

server.on("upgrade", (request, socket, head) => {
  const key = request.headers["sec-websocket-key"];
  if (!key) return socket.destroy();
  const accept = createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  let pending = Buffer.from(head);
  const consume = (chunk = Buffer.alloc(0)) => {
    pending = Buffer.concat([pending, chunk]);
    if (pending.length < 6) return;
    const length = pending[1] & 0x7f;
    if (length > 125 || pending.length < 6 + length) return;
    const mask = pending.subarray(2, 6);
    const decoded = Buffer.alloc(length);
    for (let index = 0; index < length; index += 1) decoded[index] = pending[index + 6] ^ mask[index % 4];
    const payload = Buffer.from(`echo:${decoded.toString()}`);
    socket.write(Buffer.concat([Buffer.from([0x81, payload.length]), payload]));
    pending = pending.subarray(6 + length);
  };
  socket.on("data", consume);
  consume();
});

server.listen(port, "0.0.0.0");
