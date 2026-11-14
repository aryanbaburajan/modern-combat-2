import assert from 'node:assert/strict';
import WebSocket from 'ws';
const root=(process.env.MC2_TEST_RELAY || 'ws://127.0.0.1:8788/relay')+'?room=';
async function open(room, origin='https://mc2.aryanbaburajan.com') {
 const ws=new WebSocket(root+room,{origin}); const messages=[]; ws.on('message',d=>messages.push(JSON.parse(d))); await new Promise((resolve,reject)=>{ws.once('open',resolve);ws.once('error',reject)});return {ws,messages};
}
async function wait(fn){for(let i=0;i<200;i++){let v=fn();if(v)return v;await new Promise(r=>setTimeout(r,10));}throw Error('timed out')}
const a=await open('testroom'),b=await open('testroom'),c=await open('otherroom');
try {
 const ah=await wait(()=>a.messages.find(m=>m.op==='hello')); const bh=await wait(()=>b.messages.find(m=>m.op==='hello')); assert.notEqual(ah.ip,bh.ip);
 for(const x of [a,b,c]) x.ws.send(JSON.stringify({op:'bind',fd:512,type:2,port:12345}));
 await new Promise(r=>setTimeout(r,80));a.ws.send(JSON.stringify({op:'data',fd:512,ip:'255.255.255.255',port:12345,data:'AQID'}));
 assert.equal((await wait(()=>b.messages.find(m=>m.op==='data'))).data,'AQID');await new Promise(r=>setTimeout(r,80));assert.equal(c.messages.some(m=>m.op==='data'),false);
 b.ws.send(JSON.stringify({op:'bind',fd:513,type:1,port:23456}));b.ws.send(JSON.stringify({op:'listen',fd:513}));a.ws.send(JSON.stringify({op:'bind',fd:513,type:1,port:40000}));await new Promise(r=>setTimeout(r,80));a.ws.send(JSON.stringify({op:'connect',fd:513,ip:bh.ip,port:23456}));const incoming=await wait(()=>b.messages.find(m=>m.op==='incoming'));b.ws.send(JSON.stringify({op:'bind',fd:514,type:1,port:23456}));b.ws.send(JSON.stringify({op:'accept',fd:514,token:incoming.token}));await wait(()=>a.messages.find(m=>m.op==='connected'));a.ws.send(JSON.stringify({op:'data',fd:513,data:'BAUG'}));assert.equal((await wait(()=>b.messages.find(m=>m.op==='data'&&m.fd===514))).data,'BAUG');
 a.ws.close(); await wait(()=>b.messages.find(m=>m.op==='eof'&&m.fd===514));
 let rejected=false;try{await open('testroom','https://attacker.example')}catch{rejected=true}assert.ok(rejected);
 console.log('Cloudflare relay: rooms, UDP, TCP, disconnect and origin checks passed');
}finally{for(const x of [a,b,c]) x.ws.terminate()}
