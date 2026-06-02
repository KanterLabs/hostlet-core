const port = Number(Bun.env.PORT || 3000);

Bun.serve({
  hostname: "0.0.0.0",
  port,
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/health") {
      return new Response("ok", { headers: { "content-type": "text/plain" } });
    }
    return new Response("hostlet-generated-bun", {
      headers: { "content-type": "text/plain" },
    });
  },
});
