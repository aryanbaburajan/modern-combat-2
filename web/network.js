// Virtual AF_INET sockets. Browser messages are serviced between engine frames;
// game sockets are nonblocking, just like the original LAN implementation.
export class Network {
  constructor(memory, report = () => {}) {
    this.memory = memory;
    this.report = report;
    this.sockets = new Map();
    this.ip = "127.0.0.1";
    this.online = false;
    this.nextPort = 40000;
  }

  async open(room, pageURL, relayURL) {
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(room)) throw Error("Room must use 1–64 letters, numbers, underscores or hyphens");
    const url = new URL(relayURL || "/relay", pageURL);
    if (url.protocol === "https:") url.protocol = "wss:";
    else if (url.protocol === "http:") url.protocol = "ws:";
    if (!["ws:", "wss:"].includes(url.protocol)) throw Error("Invalid room relay URL");
    url.searchParams.set("room", room);
    const ws = this.ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { ws.close(); reject(Error("Room relay timed out")); }, 10000);
      ws.onmessage = ({ data }) => {
        const m = JSON.parse(data);
        if (m.op === "hello") {
          this.ip = m.ip; this.online = true; clearTimeout(timer); resolve();
          this.report(`Room ${room}: connected as ${this.ip}`);
        } else this.receive(m);
      };
      ws.onerror = () => { clearTimeout(timer); reject(Error("Cannot connect to room relay; check the server and reload")); };
      ws.onclose = () => {
        clearTimeout(timer);
        if (!this.online) reject(Error("Room relay closed"));
        this.online = false;
        for (const s of this.sockets.values()) { s.eof = true; s.error = 104; }
        this.report("Room disconnected. Reload to reconnect.");
      };
    });
  }

  emit(message) {
    if (!this.online || this.ws.bufferedAmount > 2 * 1024 * 1024) return false;
    this.ws.send(JSON.stringify(message)); return true;
  }

  receive(m) {
    if (m.op === "peers") { this.report(`Room connected — ${m.count} player${m.count === 1 ? "" : "s"}`); return; }
    const s = this.sockets.get(m.fd);
    if (!s) return;
    if (m.op === "connected") { s.connected = true; s.connecting = false; }
    else if (m.op === "refused") { s.error = 111; s.connecting = false; }
    else if (m.op === "eof") { s.eof = true; s.connecting = false; }
    else if (m.op === "incoming") { if (s.pending.length < s.backlog) s.pending.push(m); }
    else if (m.op === "data") {
      const data = Uint8Array.from(atob(m.data), c => c.charCodeAt(0));
      if (s.bytes + data.length > 2 * 1024 * 1024) {
        if (s.type === 1) { s.error = 105; s.eof = true; this.emit({ op: "close", fd: m.fd }); }
        return;
      }
      s.queue.push({ data, ip: m.ip, port: m.port }); s.bytes += data.length;
    }
  }

  socket(type) {
    for (let fd = 512; fd < 1024; fd++) if (!this.sockets.has(fd)) {
      this.sockets.set(fd, { type, port: 0, queue: [], bytes: 0, pending: [], backlog: 8, flags: 0x800 }); return fd;
    }
    return -24;
  }

  bind(fd, port) {
    const s = this.sockets.get(fd);
    if (s.port) return -22;
    if (!port) {
      while ([...this.sockets.values()].some(v => v.port === this.nextPort)) this.nextPort++;
      port = this.nextPort++;
    }
    if ([...this.sockets.values()].some(v => v !== s && v.type === s.type && v.port === port)) return -98;
    if (!this.emit({ op: "bind", fd, type: s.type, port })) return -100;
    s.port = port; return 0;
  }

  // Arguments are ARM guest words. All pointers are checked against the guest
  // allocation before forming WASM views, including fd_sets and sockaddr_in.
  call(op, a, base = 0) {
    const bytes = new Uint8Array(this.memory().buffer);
    const view = new DataView(bytes.buffer);
    const ptr = (p, n) => {
      if (!Number.isInteger(p) || p < 0 || n < 0 || p + n > 0x20000000 || base + p + n > bytes.length) throw Error("Bad socket pointer");
      return base + p;
    };
    const r32 = p => view.getUint32(ptr(p, 4), true);
    const w32 = (p, v) => view.setUint32(ptr(p, 4), v, true);
    const str = (p, limit = 256) => { let text=""; for(let i=0;i<limit;i++){const c=bytes[ptr(p+i,1)];if(!c) return text;text+=String.fromCharCode(c);}return text; };
    const cstr = (p, text, cap) => { if (cap <= text.length) return -36; bytes.set(new TextEncoder().encode(text+"\0"),ptr(p,text.length+1));return 0; };
    const addr = p => {
      const start=ptr(p,16);
      if(view.getUint16(start,true)!==2) throw Error("Invalid address family");
      return {port:view.getUint16(start+2,false),ip:[...bytes.subarray(start+4,start+8)].join(".")};
    };
    const address = (p, ip, port) => {const start=ptr(p,16);bytes.fill(0,start,start+16);view.setUint16(start,2,true);view.setUint16(start+2,port,false);bytes.set(ip.split(".").map(Number),start+4);};
    const outaddr = (p,len,ip,port) => {if(p){if(!len) throw Error("Missing address length");const n=Math.min(r32(len),16); const temp=new Uint8Array(16);new DataView(temp.buffer).setUint16(0,2,true);new DataView(temp.buffer).setUint16(2,port,false);temp.set(ip.split(".").map(Number),4);bytes.set(temp.subarray(0,n),ptr(p,n));w32(len,16);}};
    try {
      const [fd, b, c, flags, dest, destLen] = a;
      if (op === 20) {
        const name=fd?str(fd):null, service=b?str(b):"0", p=a[5];
        const family=c?r32(c+4):0, type=c?r32(c+8):2, protocol=c?r32(c+12):0;
        if(family!==0&&family!==2)return -6;
        if(!/^\d{1,5}$/.test(service)||Number(service)>65535)return -8;
        const ip=name===null?"0.0.0.0":name==="localhost"?"127.0.0.1":name===`mc2-${this.ip.split(".").pop()}`?this.ip:/^10\.77\.0\.(?:[2-9]|1[0-7])$/.test(name)?name:null;
        if(!ip)return -2;
        bytes.fill(0,ptr(p,64),ptr(p,64)+64);w32(p+4,2);w32(p+8,type||2);w32(p+12,protocol||((type||2)===2?17:6));w32(p+16,16);w32(p+24,p+32);address(p+32,ip,Number(service));w32(flags,p);return 0;
      }
      if (op === 19) return cstr(fd, this.online ? `mc2-${this.ip.split(".").pop()}` : "localhost", b);
      if (op === 18) {
        const name = str(fd), p=a[5];
        const ip = name === "localhost" ? "127.0.0.1" : name === `mc2-${this.ip.split(".").pop()}` ? this.ip : /^10\.77\.0\.(?:[2-9]|1[0-7])$/.test(name) ? name : null;
        if (!ip) return 0;
        bytes.fill(0,ptr(p,96),ptr(p,96)+96);
        w32(p,p+32);w32(p+4,p+20);view.setUint32(ptr(p+8,4),2,true);w32(p+12,4);w32(p+16,p+24);w32(p+24,p+80);
        cstr(p+32,name,48);bytes.set(ip.split(".").map(Number),ptr(p+80,4));return p;
      }
      if (!this.online) return -100;
      if (op === 0) { if(fd!==2) return -97; if(![1,2].includes(b & 15)) return -93; return this.socket(b&15); }
      if (op === 12) {
        if(fd>1024) return -22;
        let count=0;
        for(const [set,kind] of [[b,"read"],[c,"write"],[flags,"error"]]) {
          if(!set) continue;
          for(let i=0;i<fd;i++) {
            const word=set+4*(i>>>5), mask=1<<(i&31), bits=r32(word);
            if(!(bits&mask)) continue;
            const s=this.sockets.get(i);
            const ready=s && (kind==="read" ? s.queue.length || s.pending.length || s.eof : kind==="write" ? s.type===2 || s.connected || s.error : s.error);
            if(ready) count++; else w32(word,bits&~mask);
          }
        }
        return count;
      }
      const s = this.sockets.get(fd);
      if(!s) return -9;
      if(op===1) {if(c<16)return -22;const d=addr(b);if(d.ip!=="0.0.0.0"&&d.ip!==this.ip)return -99;return this.bind(fd,d.port);}
      if(op===2) {
        if(c<16)return -22;
        if(s.connected)return -106;if(s.connecting)return -114;
        s.remote=addr(b);
        if(!s.port){const r=this.bind(fd,0);if(r<0)return r;}
        if(s.type===2){s.connected=true;return 0;}
        s.connecting=true;this.emit({op:"connect",fd,...s.remote});return -115;
      }
      if(op===3) {if(s.type!==1)return -95;if(!s.port){const r=this.bind(fd,0);if(r<0)return r;}s.backlog=Math.max(1,Math.min(b,16));s.listening=true;this.emit({op:"listen",fd});return 0;}
      if(op===4) {
        if(!s.listening)return -22;
        const incoming=s.pending[0];if(!incoming)return -11;
        if(b) outaddr(b,c,incoming.ip,incoming.port);
        const accepted=this.socket(1);if(accepted<0)return accepted;
        s.pending.shift();const next=this.sockets.get(accepted);next.port=s.port;next.connected=true;next.remote={ip:incoming.ip,port:incoming.port};
        this.emit({op:"bind",fd:accepted,port:s.port,type:1});this.emit({op:"accept",fd:accepted,token:incoming.token});return accepted;
      }
      if([5,6,17].includes(op)) {
        if(s.eof)return -32;
        if(c>65507)return -90;
        if(s.type===1&&!s.connected)return -107;
        const d=op===6&&dest ? (destLen<16?null:addr(dest)) : s.remote;
        if(s.type===2&&!d)return -89;
        if(!s.port){const r=this.bind(fd,0);if(r<0)return r;}
        const payload=bytes.subarray(ptr(b,c),ptr(b,c)+c);
        const data=btoa(String.fromCharCode(...payload));
        if(!this.emit({op:"data",fd,...d,data}))return -11;
        return c;
      }
      if([7,8,16].includes(op)) {
        if(c===0)return 0;
        ptr(b,c);
        if(!s.queue.length)return s.error ? -s.error : s.eof ? 0 : -11;
        const packet=s.queue[0],n=Math.min(c,packet.data.length);
        if(op===8)outaddr(dest,destLen,packet.ip,packet.port);
        bytes.set(packet.data.subarray(0,n),ptr(b,n));
        if(!(flags&2) || op===16){
          s.bytes-=s.type===2?packet.data.length:n;
          if(s.type===2||n===packet.data.length)s.queue.shift();else packet.data=packet.data.subarray(n);
        }
        return n;
      }
      if(op===9)return 0;
      if(op===10) {if(r32(dest)<4)return -22;w32(flags,c===4?s.error||0:c===3?s.type:0);w32(dest,4);if(c===4)s.error=0;return 0;}
      if(op===11){outaddr(b,c,this.ip,s.port);return 0;}
      if(op===13) {if(b===3)return s.flags;if(b===4){s.flags=c;return 0;}if(b===1||b===2)return 0;return -22;}
      if(op===14) {
        if(b===0x5421)return 0; // FIONBIO: all guest sockets are polled
        if(b===0x541b){w32(c,s.queue[0]?.data.length||0);return 0;}
        if(b===0x8912) { // SIOCGIFCONF, ARM 32-bit ifreq is 32 bytes
          const size=r32(c), buffer=r32(c+4);if(!buffer){w32(c,32);return 0;}if(size<32){w32(c,0);return 0;}cstr(buffer,"wlan0",16);address(buffer+16,this.ip,0);w32(c,32);return 0;
        }
        if(b===0x8915){address(c+16,this.ip,0);return 0;}
        if(b===0x8919){address(c+16,"10.77.0.255",0);return 0;}
        if(b===0x891b){address(c+16,"255.255.255.0",0);return 0;}
        if(b===0x8913){view.setUint16(ptr(c+16,2),0x1043,true);return 0;}
        return -25;
      }
      if(op===15){this.emit({op:"close",fd});this.sockets.delete(fd);return 0;}
      return -38;
    } catch { return -14; }
  }
}
