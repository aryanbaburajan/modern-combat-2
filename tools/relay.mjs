// Room-scoped virtual LAN. It never opens sockets to client-supplied hosts.
import { WebSocketServer, WebSocket } from "ws";

export function attachRelay(server) {
  const rooms = new Map();
  const wss = new WebSocketServer({ noServer: true, maxPayload: 192 * 1024, perMessageDeflate: false });
  const send = (peer, message) => {
    if (peer.ws.readyState === WebSocket.OPEN) {
      if (peer.ws.bufferedAmount > 2 * 1024 * 1024) return peer.ws.close(1013, "Slow client");
      peer.ws.send(JSON.stringify(message));
    }
  };
  const fdOK = n => Number.isInteger(n) && n >= 512 && n < 1024;
  const portOK = n => Number.isInteger(n) && n > 0 && n <= 65535;
  server.on("upgrade", (req, socket, head) => {
    let url;
    try { url = new URL(req.url, "http://localhost"); } catch { socket.destroy(); return; }
    const room = url.searchParams.get("room") || "";
    // A browser may only connect from the page served by this host (including
    // Tailscale Serve, whose forwarded Host differs from the local Host).
    const host = req.headers["x-forwarded-host"] || req.headers.host;
    let originOK = !req.headers.origin;
    try { if (req.headers.origin) originOK = new URL(req.headers.origin).host === host; } catch {}
    if (url.pathname !== "/relay" || !/^[A-Za-z0-9_-]{1,64}$/.test(room) || !originOK) {
      socket.end("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n"); return;
    }
    wss.handleUpgrade(req, socket, head, ws => wss.emit("connection", ws, room));
  });
  wss.on("connection", (ws, name) => {
    let room = rooms.get(name);
    if (!room) { room = new Map(); rooms.set(name, room); }
    let id = 2;
    while (room.has(`10.77.0.${id}`)) id++;
    if (id > 17) { ws.close(1008, "Room full (16 players)"); return; }
    const peer = { ws, ip: `10.77.0.${id}`, sockets: new Map(), alive: true };
    room.set(peer.ip, peer);
    const population = () => { for (const p of room.values()) send(p, { op: "peers", count: room.size }); };
    send(peer, { op: "hello", ip: peer.ip }); population();
    ws.on("pong", () => { peer.alive = true; });
    ws.on("error", () => {});
    ws.on("message", (raw, binary) => {
      if (binary) return ws.close(1003, "JSON required");
      let m;
      try { m = JSON.parse(raw.toString()); } catch { return ws.close(1008, "Invalid JSON"); }
      if (!m || !fdOK(m.fd)) return ws.close(1008, "Invalid socket");
      const s = peer.sockets.get(m.fd);
      if (m.op === "bind") {
        if (!portOK(m.port) || ![1, 2].includes(m.type)) return ws.close(1008, "Invalid bind");
        peer.sockets.set(m.fd, { port: m.port, type: m.type }); return;
      }
      if (!s) return;
      if (m.op === "listen") { if (s.type === 1) s.listening = true; return; }
      if (m.op === "connect") {
        const dst = room.get(m.ip);
        const target = dst && [...dst.sockets].find(([, v]) => v.type === 1 && v.listening && v.port === m.port);
        if (!target) { send(peer, { op: "refused", fd: m.fd }); return; }
        const token = `${peer.ip}:${m.fd}`;
        s.pending = { ip: dst.ip, fd: target[0] };
        send(dst, { op: "incoming", fd: target[0], ip: peer.ip, port: s.port, token }); return;
      }
      if (m.op === "accept") {
        if (typeof m.token !== "string") return;
        const [ip, clientFd] = m.token.split(":");
        const client = room.get(ip), remote = client?.sockets.get(Number(clientFd));
        if (!remote?.pending || remote.pending.ip !== peer.ip || s.type !== 1) return;
        const listener = peer.sockets.get(remote.pending.fd);
        if (!listener?.listening || listener.port !== s.port) return;
        s.remote = { ip, fd: Number(clientFd) };
        remote.remote = { ip: peer.ip, fd: m.fd }; delete remote.pending;
        send(client, { op: "connected", fd: Number(clientFd) }); return;
      }
      if (m.op === "close") {
        if (s.remote) {
          const dst=room.get(s.remote.ip), remote=dst?.sockets.get(s.remote.fd);
          if(dst) send(dst,{op:"eof",fd:s.remote.fd});
          if(remote) delete remote.remote;
        }
        peer.sockets.delete(m.fd); return;
      }
      if (m.op !== "data" || typeof m.data !== "string" || m.data.length > 90000 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(m.data)) return;
      if (s.type === 1) {
        const dst = s.remote && room.get(s.remote.ip);
        if (dst) send(dst, { op: "data", fd: s.remote.fd, ip: peer.ip, port: s.port, data: m.data });
      } else if (portOK(m.port)) {
        const broadcast = m.ip === "255.255.255.255" || m.ip === "10.77.0.255";
        for (const dst of room.values()) {
          if (broadcast ? dst === peer : dst.ip !== m.ip) continue;
          for (const [fd, socket] of dst.sockets) if (socket.type === 2 && socket.port === m.port)
            send(dst, { op: "data", fd, ip: peer.ip, port: s.port, data: m.data });
        }
      }
    });
    ws.on("close", () => {
      room.delete(peer.ip);
      for (const dst of room.values()) for (const [fd, s] of dst.sockets) {
        if (s.remote?.ip === peer.ip || s.pending?.ip === peer.ip) {
          send(dst, { op: "eof", fd }); delete s.remote; delete s.pending;
        }
      }
      if (!room.size) rooms.delete(name); else population();
    });
  });
  const heartbeat = setInterval(() => {
    for (const room of rooms.values()) for (const p of room.values()) {
      if (!p.alive) { p.ws.terminate(); continue; }
      p.alive = false; p.ws.ping();
    }
  }, 30000);
  heartbeat.unref();
  server.on("close", () => { clearInterval(heartbeat); for (const ws of wss.clients) ws.terminate(); wss.close(); });
  return wss;
}
