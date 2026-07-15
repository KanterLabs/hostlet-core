const socket = new WebSocket(import.meta.env.VITE_WS_URL);
socket.addEventListener("message", (event) => console.log(event.data));
