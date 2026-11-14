import { DurableObject } from 'cloudflare:workers';

const fdOK = n => Number.isInteger(n) && n >= 512 && n < 1024;
const portOK = n => Number.isInteger(n) && n > 0 && n <= 65535;
const validData = s => typeof s === 'string' && s.length <= 90000 && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(s);

// Each connection carries its virtual sockets so hibernation can restore a room.
export class Room extends DurableObject {
  peers() {
    return new Map(this.ctx.getWebSockets().map(ws => {
      const state = ws.deserializeAttachment();
      return [state.ip, { ws, ip: state.ip, sockets: new Map(state.sockets) }];
    }));
  }
  save(peers) {
    for (const p of peers.values()) p.ws.serializeAttachment({ ip: p.ip, sockets: [...p.sockets] });
  }
  send(peer, message) {
    if (peer?.ws.readyState === 1) {
      try { peer.ws.send(JSON.stringify(message)); } catch { /* close event handles cleanup */ }
    }
  }
  population(peers) { for (const peer of peers.values()) this.send(peer, { op: 'peers', count: peers.size }); }
  async fetch() {
    const peers = this.peers();
    let id = 2;
    while (peers.has(`10.77.0.${id}`)) id++;
    if (id > 17) return new Response('Room full', { status: 429 });
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    const peer = { ws: server, ip: `10.77.0.${id}`, sockets: new Map() };
    peers.set(peer.ip, peer);
    this.save(peers);
    this.send(peer, { op: 'hello', ip: peer.ip });
    this.population(peers);
    return new Response(null, { status: 101, webSocket: client });
  }
  webSocketMessage(ws, raw) {
    if (typeof raw !== 'string') return ws.close(1003, 'JSON required');
    if (raw.length > 192 * 1024) return ws.close(1009, 'Message too large');
    let m;
    try { m = JSON.parse(raw); } catch { return ws.close(1008, 'Invalid JSON'); }
    if (!m || !fdOK(m.fd)) return ws.close(1008, 'Invalid socket');
    const peers = this.peers(), peer = peers.get(ws.deserializeAttachment().ip);
    if (!peer) return ws.close(1008, 'Unknown peer');
    const s = peer.sockets.get(m.fd);
    if (m.op === 'bind') {
      if (!portOK(m.port) || ![1, 2].includes(m.type)) return ws.close(1008, 'Invalid bind');
      peer.sockets.set(m.fd, { port: m.port, type: m.type });
    } else if (!s) return;
    else if (m.op === 'listen') { if (s.type === 1) s.listening = true; }
    else if (m.op === 'connect') {
      const dst = peers.get(m.ip);
      const target = dst && [...dst.sockets].find(([, v]) => v.type === 1 && v.listening && v.port === m.port);
      if (!target) this.send(peer, { op: 'refused', fd: m.fd });
      else { s.pending = { ip: dst.ip, fd: target[0] }; this.send(dst, { op: 'incoming', fd: target[0], ip: peer.ip, port: s.port, token: `${peer.ip}:${m.fd}` }); }
    } else if (m.op === 'accept') {
      if (typeof m.token !== 'string') return;
      const [ip, clientFd] = m.token.split(':');
      const client = peers.get(ip), remote = client?.sockets.get(Number(clientFd));
      if (!remote?.pending || remote.pending.ip !== peer.ip || s.type !== 1) return;
      const listener = peer.sockets.get(remote.pending.fd);
      if (!listener?.listening || listener.port !== s.port) return;
      s.remote = { ip, fd: Number(clientFd) }; remote.remote = { ip: peer.ip, fd: m.fd }; delete remote.pending;
      this.send(client, { op: 'connected', fd: Number(clientFd) });
    } else if (m.op === 'close') {
      if (s.remote) {
        const dst = peers.get(s.remote.ip), remote = dst?.sockets.get(s.remote.fd);
        this.send(dst, { op: 'eof', fd: s.remote.fd });
        if (remote) delete remote.remote;
      }
      peer.sockets.delete(m.fd);
    } else if (m.op === 'data' && validData(m.data)) {
      if (s.type === 1) {
        const dst = s.remote && peers.get(s.remote.ip);
        if (dst) this.send(dst, { op: 'data', fd: s.remote.fd, ip: peer.ip, port: s.port, data: m.data });
      } else if (portOK(m.port)) {
        const broadcast = m.ip === '255.255.255.255' || m.ip === '10.77.0.255';
        for (const dst of peers.values()) {
          if (broadcast ? dst === peer : dst.ip !== m.ip) continue;
          for (const [fd, socket] of dst.sockets) if (socket.type === 2 && socket.port === m.port)
            this.send(dst, { op: 'data', fd, ip: peer.ip, port: s.port, data: m.data });
        }
      }
    }
    this.save(peers);
  }
  webSocketClose(ws) {
    const peers = this.peers();
    const ip = ws.deserializeAttachment()?.ip;
    peers.delete(ip);
    for (const dst of peers.values()) for (const [fd, socket] of dst.sockets) {
      if (socket.remote?.ip === ip || socket.pending?.ip === ip) {
        this.send(dst, { op: 'eof', fd }); delete socket.remote; delete socket.pending;
      }
    }
    this.save(peers); this.population(peers);
  }
  webSocketError(ws) { try { ws.close(1011, 'Connection error'); } catch {} }
}

export default {
  fetch(request, env) {
    const url = new URL(request.url);
    const room = url.searchParams.get('room') || '';
    const origins = (env.ALLOWED_ORIGINS || '').split(',').map(s => s.trim()).filter(Boolean);
    const origin = request.headers.get('Origin');
    if (url.pathname !== '/relay' || !/^[A-Za-z0-9_-]{1,64}$/.test(room) || !origin || !origins.includes(origin)) return new Response('Forbidden', { status: 403 });
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') return new Response('WebSocket required', { status: 426 });
    return env.ROOMS.getByName(room).fetch(request);
  },
};
