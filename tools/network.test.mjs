import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { attachRelay } from "./relay.mjs";
import { Network } from "../web/network.js";

async function until(fn) {
  const end=Date.now()+3000;
  while(!fn()) { if(Date.now()>end)throw Error("Network condition timed out"); await new Promise(r=>setTimeout(r,10)); }
}
function client() {
  const memory={buffer:new ArrayBuffer(65536)};
  const network=new Network(()=>memory);
  const v=new DataView(memory.buffer),u=new Uint8Array(memory.buffer);
  const call=(op,...args)=>network.call(op,[...args,...Array(6).fill(0)].slice(0,6));
  const addr=(ip,port,p=64)=>{u.fill(0,p,p+16);v.setUint16(p,2,true);v.setUint16(p+2,port,false);u.set(ip.split(".").map(Number),p+4);return p;};
  return {network,v,u,call,addr};
}

test("virtual LAN: discovery, isolation, streams, EOF, errno, and guest ABI", async t=>{
  const server=http.createServer();attachRelay(server);server.listen(0,"127.0.0.1");await once(server,"listening");
  const url=`http://127.0.0.1:${server.address().port}/`;
  const a=client(),b=client(),other=client();
  t.after(()=>{a.network.ws?.close();b.network.ws?.close();other.network.ws?.close();server.close();});
  await a.network.open("family",url);await b.network.open("family",url);await other.network.open("other",url);
  assert.notEqual(a.network.ip,b.network.ip);
  const sa=a.call(0,2,2),sb=b.call(0,2,2),so=other.call(0,2,2);
  assert.equal(a.call(1,sa,a.addr("0.0.0.0",12345),16),0);
  assert.equal(b.call(1,sb,b.addr("0.0.0.0",12345),16),0);
  assert.equal(other.call(1,so,other.addr("0.0.0.0",12345),16),0);
  assert.equal(a.call(0,10,2),-97);
  assert.equal(a.call(7,sa,1000,10),-11);
  a.u.set([1,2,3,4],1000);
  // Wait for the other sockets to finish their ordered WebSocket binds.
  await new Promise(r=>setTimeout(r,30));
  assert.equal(a.call(6,sa,1000,4,0,a.addr("255.255.255.255",12345),16),4);
  await until(()=>b.network.sockets.get(sb).queue.length);
  b.v.setUint32(200,16,true);
  assert.equal(b.call(8,sb,1000,2,2,64,200),2); // MSG_PEEK preserves datagram
  assert.equal(b.call(8,sb,1000,4,0,64,200),4);
  assert.deepEqual([...b.u.subarray(1000,1004)],[1,2,3,4]);
  assert.equal(b.v.getUint16(66,false),12345);
  assert.equal(b.call(7,sb,1000,4),-11);
  assert.equal(other.network.sockets.get(so).queue.length,0);
  assert.equal(a.network.sockets.get(sa).queue.length,0);
  assert.equal(b.call(7,sb,65535,100),-14);
  // 32-bit ARM addrinfo: family/type/protocol/length/address/result pointer.
  b.u.set(new TextEncoder().encode("12345\0"),300);
  b.v.setUint32(404,2,true);b.v.setUint32(408,2,true);b.v.setUint32(412,17,true);
  assert.equal(b.network.call(20,[0,300,400,500,0,600]),0);
  assert.equal(b.v.getUint32(500,true),600);assert.equal(b.v.getUint32(624,true),632);
  assert.equal(b.v.getUint16(634,false),12345);
  // Hostent address-list terminator must not overlap the hostname.
  b.u.set(new TextEncoder().encode(`mc2-${b.network.ip.split(".").pop()}\0`),800);
  assert.equal(b.network.call(18,[800,0,0,0,0,900]),900);
  const list=b.v.getUint32(916,true);assert.equal(b.v.getUint32(list+4,true),0);
  // TCP handshake, readiness, partial read, and peer close.
  const listener=b.call(0,2,1);assert.equal(b.call(1,listener,b.addr("0.0.0.0",23456),16),0);assert.equal(b.call(3,listener,8),0);
  await new Promise(r=>setTimeout(r,30));
  const stream=a.call(0,2,1);assert.equal(a.call(2,stream,a.addr(b.network.ip,23456),16),-115);
  await until(()=>b.network.sockets.get(listener).pending.length);
  const accepted=b.call(4,listener,0,0);assert(accepted>=512);
  await until(()=>a.network.sockets.get(stream).connected);
  a.u.set([8,7,6,5],1000);assert.equal(a.call(5,stream,1000,4,0),4);
  await until(()=>b.network.sockets.get(accepted).queue.length);
  b.u.fill(0,2048,2176);b.v.setUint32(2048+4*(accepted>>>5),1<<(accepted&31),true);
  assert.equal(b.call(12,accepted+1,2048,0,0),1);
  assert.equal(b.call(7,accepted,1000,2,0),2);assert.equal(b.call(7,accepted,1002,2,0),2);
  assert.deepEqual([...b.u.subarray(1000,1004)],[8,7,6,5]);
  a.call(15,stream);await until(()=>b.network.sockets.get(accepted).eof);assert.equal(b.call(7,accepted,1000,2),0);
  assert.equal(b.call(5,accepted,1000,2,0),-32);
  const refused=a.call(0,2,1);assert.equal(a.call(2,refused,a.addr(b.network.ip,1),16),-115);
  await until(()=>a.network.sockets.get(refused).error===111);
});

test("network remains offline without a room and rejects invalid room names", async()=>{
  const c=client();
  assert.equal(c.call(0,2,2),-100);
  await assert.rejects(c.network.open("bad room", "http://localhost/"),/Room must/);
  assert.equal(c.network.online,false);
});
