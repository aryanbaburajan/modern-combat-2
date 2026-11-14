import { openSaveDatabase } from "./saves.js";
// Runs the emulator (wasm) and renders into an OffscreenCanvas.
import { GLES } from "./gl.js";
import { Network } from "./network.js";

let wasm, exp, mem, gles, manifest = {}, saves = new Map(), db;
let assetBase = new URL("game/", self.location.href);
let running = false, failed = false, gameW = 800, gameH = 480;
let pageVisible = true;
let visibleWaiters = [];
function waitUntilVisible() {
  return pageVisible ? Promise.resolve() : new Promise(resolve => visibleWaiters.push(resolve));
}
const residentArchives = new Map();
const queue = [];
const audio = { sounds: new Map(), music: new Map(), durations: new Map(), musicVol: 1, soundVol: 1 };
const dec = new TextDecoder();
const enc = new TextEncoder();
const network = new Network(() => exp.memory, (text) => {
  log("[network] " + text);
  postMessage({ type: "network", text });
});

function log(s) {
  postMessage({ type: "log", text: s });
}

function assetUrl(path) {
  return new URL(path, assetBase).href;
}

const memView = {
  base: 0,
  _buf: null,
  refresh() {
    if (this._buf !== exp.memory.buffer) {
      this._buf = exp.memory.buffer;
      this._u8 = new Uint8Array(this._buf);
      this._u32 = new Uint32Array(this._buf);
      this._i32 = new Int32Array(this._buf);
    }
  },
  u8() { this.refresh(); return this._u8; },
  u32() { this.refresh(); return this._u32; },
  i32() { this.refresh(); return this._i32; },
  str(p) {
    const u = this.u8();
    let a = this.base + p, e = a;
    while (u[e]) e++;
    return dec.decode(u.subarray(a, e));
  },
};
const hostStr = (p, n) => dec.decode(memView.u8().subarray(p, p + n));

// ---------- synchronous data access (sync XHR range requests) ----------
function fileSize(path) {
  const v = manifest[path];
  return v === undefined ? -1 : v;
}
function fileRead(path, off, buf, len) {
  const size = manifest[path];
  if (size === undefined || off >= size) return 0;
  const end = Math.min(size, off + len) - 1;
  const resident = residentArchives.get(path);
  if (resident) {
    const data = resident.subarray(off, end + 1);
    memView.u8().set(data, buf);
    return data.length;
  }
  const x = new XMLHttpRequest();
  x.open("GET", assetUrl(path), false);
  x.responseType = "arraybuffer";
  x.setRequestHeader("Range", `bytes=${off}-${end}`);
  x.send();
  if (x.status !== 206 && x.status !== 200) {
    return 0;
  }
  let data = new Uint8Array(x.response);
  if (x.status === 200) data = data.subarray(off, end + 1); // server ignored Range
  memView.u8().set(data, buf);
  return data.length;
}

// ---------- saves (IndexedDB) ----------
async function loadSaves() {
  try { db = await openSaveDatabase(); }
  catch (e) { postMessage({ type: "save-error", text: `Browser storage is unavailable: ${e.message}. Game progress will not be saved.` }); return; }
  await new Promise((res) => {
    let tx;
    try {
      tx = db.transaction("files", "readonly");
    } catch (e) {
      postMessage({ type: "save-error", text: "Could not read saved game progress from browser storage." });
      return res();
    }
    const st = tx.objectStore("files");
    const req = st.openCursor();
    req.onsuccess = () => {
      const c = req.result;
      if (!c) return res();
      saves.set(c.key, new Uint8Array(c.value));
      c.continue();
    };
    req.onerror = () => { postMessage({ type: "save-error", text: "Could not read saved game progress from browser storage." }); res(); };
  });
}
function persist(path, data) {
  if (!db) return;
  try {
    const tx = db.transaction("files", "readwrite");
    if (data) tx.objectStore("files").put(data, path);
    else tx.objectStore("files").delete(path);
    tx.onabort = tx.onerror = () => postMessage({ type: "save-error", text: "Could not save game progress in browser storage." });
  } catch (e) {
    postMessage({ type: "save-error", text: `Could not save game progress: ${e.message}` });
  }
}

// ---------- Java static methods (audio etc.) ----------
function audioUrl(path) {
  const m = /GloftBPHP\/+(.*)$/.exec(path || "");
  return m ? assetUrl("GloftBPHP/" + m[1]) : null;
}
function javaCall(req) {
  const { m, a } = req;
  const now = performance.now();
  const post = (op, extra) => postMessage({ type: "audio", op, ...extra });
  switch (m) {
    case "GetPhoneLanguage": return "i:0";
    case "getManufacture": return "i:0";
    case "isWifiEnabled": return network.online ? "i:1" : "i:0";
    case "GetNumPlaylists": return "i:0";
    case "DisablePlaylist": return "i:0";
    case "GetCurrentTime": return "j:" + Date.now();
    case "loadSound": case "loadSoundPool": {
      const url = audioUrl(a[1]);
      audio.sounds.set(a[0], { url, end: 0, loop: false });
      if (url) post("load", { url });
      return "v";
    }
    case "playSound": {
      const url = audioUrl(a[2]);
      const dur = audio.durations.get(url) || 1000;
      audio.sounds.set(a[0], { url, end: a[1] ? Infinity : now + dur, loop: !!a[1] });
      if (url) post("play", { ch: "s" + a[0], url, loop: !!a[1], vol: a[3] * audio.soundVol });
      return "v";
    }
    case "stopSound": case "unloadSound": {
      const s = audio.sounds.get(a[0]);
      if (s) s.end = 0;
      post("stop", { ch: "s" + a[0] });
      return "v";
    }
    case "pauseSound": post("pause", { ch: "s" + a[0] }); return "v";
    case "resumeSound": post("resume", { ch: "s" + a[0] }); return "v";
    case "isSoundLoaded": return "i:" + (audio.sounds.has(a[0]) ? 1 : 0);
    case "isMediaPlaying": {
      const s = audio.sounds.get(a[0]);
      return "i:" + (s && s.end > now ? 1 : 0);
    }
    case "getSoundDuration": {
      const s = audio.sounds.get(a[0]);
      return "i:" + Math.round((s && audio.durations.get(s.url)) || 0);
    }
    case "loadMusic": {
      const url = audioUrl(a[1]);
      const cur = audio.music.get(a[0]);
      if (!cur || cur.url !== url) audio.music.set(a[0], { url, end: 0 });
      if (url) post("load", { url });
      return "v";
    }
    case "playMusic": {
      const mu = audio.music.get(a[0]);
      if (!mu) return "v";
      const dur = audio.durations.get(mu.url) || 60000;
      mu.end = a[1] ? Infinity : now + dur;
      post("play", { ch: "music", url: mu.url, loop: !!a[1], vol: audio.musicVol, music: true });
      return "v";
    }
    case "stopMusic": case "unloadMusic": {
      const mu = audio.music.get(a[0]);
      if (mu && mu.end) { mu.end = 0; post("stop", { ch: "music" }); }
      return "v";
    }
    case "pauseMusic": post("pause", { ch: "music" }); return "v";
    case "resumeMusic": post("resume", { ch: "music" }); return "v";
    case "isMusicPlaying": {
      const mu = audio.music.get(a[0]);
      return "i:" + (mu && mu.end > now ? 1 : 0);
    }
    case "getMusicDuration": {
      const mu = audio.music.get(a[0]);
      return "i:" + Math.round((mu && audio.durations.get(mu.url)) || 0);
    }
    case "setVolume":
      if (a[0] === -1) { audio.musicVol = a[1]; post("volume", { ch: "music", vol: a[1] }); }
      else if (a[0] === -2) audio.soundVol = a[1];
      else post("volume", { ch: "s" + a[0], vol: a[1] * audio.soundVol });
      return "v";
    case "stopAllSounds":
      for (const s of audio.sounds.values()) s.end = 0;
      if (a[0] !== 4) for (const mu of audio.music.values()) mu.end = 0;
      post("stopAll", { music: a[0] !== 4, sounds: a[0] !== 2 });
      return "v";
    case "pauseAllSound": case "pauseAllSounds": post("pauseAll", {}); return "v";
    case "resumeAllSound": post("resumeAll", {}); return "v";
    case "Exit": running = false; postMessage({ type: "exit" }); return "v";
    default:
      if (!javaCall.seen) javaCall.seen = new Set();
      if (!javaCall.seen.has(m)) { javaCall.seen.add(m); log(`[java] unhandled ${req.c}.${m}${req.s} ${JSON.stringify(a)}`); }
      return "v";
  }
}

const imports = {
  env: {
    js_net: (op, args, base) => network.call(op, [...new Uint32Array(exp.memory.buffer, args, 6)], base),
    js_log: (p, n) => log(hostStr(p, n)),
    js_now: () => performance.now(),
    js_wall: () => Date.now() / 1000,
    js_file_size: (p, n) => fileSize(hostStr(p, n)),
    js_file_read: (p, n, off, buf, len) => fileRead(hostStr(p, n), off, buf, len),
    js_save_size: (p, n) => { const d = saves.get(hostStr(p, n)); return d ? d.length : -1; },
    js_save_read: (p, n, dst) => { const d = saves.get(hostStr(p, n)); if (d) memView.u8().set(d, dst); },
    js_save_store: (p, n, d, len) => {
      const k = hostStr(p, n);
      const v = memView.u8().slice(d, d + len);
      saves.set(k, v);
      persist(k, v);
    },
    js_save_delete: (p, n) => { const k = hostStr(p, n); saves.delete(k); persist(k, null); },
    js_gl: (id, args) => gles.call(id, args),
    js_java: (p, n, out, cap) => {
      const r = enc.encode(javaCall(JSON.parse(hostStr(p, n))));
      const len = Math.min(r.length, cap);
      memView.u8().set(r.subarray(0, len), out);
      return len;
    },
  },
};

function lastError() {
  const p = exp.mc2_alloc(8192);
  const n = exp.mc2_last_error(p, 8192);
  const s = hostStr(p, n);
  exp.mc2_free(p, 8192);
  return s;
}
function check(r, what) {
  if (r !== 0) {
    failed = true;
    running = false;
    const e = lastError();
    postMessage({ type: "fatal", text: `${what} failed: ${e}` });
    throw new Error(e);
  }
}

async function start(msg) {
  const { canvas, width, height } = msg;
  pageVisible = msg.visible !== false;
  await waitUntilVisible();
  if (msg.room) {
    postMessage({ type: "status", text: "Connecting to room…" });
    await network.open(msg.room, msg.pageURL, msg.relayURL);
  }
  if (msg.assetBase) assetBase = new URL(msg.assetBase, self.location.href);
  gameW = width;
  gameH = height;
  postMessage({ type: "status", text: "Loading manifest…" });
  manifest = await (await fetch(assetUrl("manifest.json"))).json();
  const archives = Object.entries(manifest).filter(([path]) => path.toLowerCase().endsWith(".pak"));
  const totalArchiveBytes = archives.reduce((sum, [, size]) => sum + size, 0);
  let loadedArchiveBytes = 0;
  for (const [path, size] of archives) {
    await waitUntilVisible();
    const response = await fetch(assetUrl(path));
    if (!response.ok) throw new Error(`Cannot load game archive ${path} (${response.status}). Reimport your game files.`);
    const bytes = new Uint8Array(size);
    const reader = response.body?.getReader();
    if (reader) {
      let offset = 0;
      for (;;) {
        await waitUntilVisible();
        const { value, done } = await reader.read();
        if (done) break;
        if (offset + value.length > size) throw new Error(`Game archive ${path} has an unexpected size. Reimport your game files.`);
        bytes.set(value, offset);
        offset += value.length;
        postMessage({ type: "preparation", progress: (loadedArchiveBytes + offset) / totalArchiveBytes });
      }
      if (offset !== size) throw new Error(`Game archive ${path} is incomplete. Reimport your game files.`);
    } else {
      const buffer = new Uint8Array(await response.arrayBuffer());
      if (buffer.length !== size) throw new Error(`Game archive ${path} is incomplete. Reimport your game files.`);
      bytes.set(buffer);
    }
    residentArchives.set(path, bytes);
    loadedArchiveBytes += bytes.length;
    postMessage({ type: "preparation", progress: loadedArchiveBytes / totalArchiveBytes });
  }
  await waitUntilVisible();
  await loadSaves();
  postMessage({ type: "status", text: "Loading engine…" });
  const [wbytes, lib] = await Promise.all([
    fetch("mc2engine.wasm").then((response) => {
      if (!response.ok) throw new Error(`Cannot load engine (${response.status})`);
      return response.arrayBuffer();
    }),
    fetch(assetUrl("libsandstorm2.so")).then((response) => {
      if (!response.ok) throw new Error(`Cannot load game library (${response.status})`);
      return response.arrayBuffer();
    }),
  ]);
  await waitUntilVisible();
  ({ instance: wasm } = await WebAssembly.instantiate(wbytes, imports));
  exp = wasm.exports;
  const p = exp.mc2_alloc(lib.byteLength);
  new Uint8Array(exp.memory.buffer).set(new Uint8Array(lib), p);
  check(exp.mc2_init(p, lib.byteLength), "init");
  memView.base = exp.mc2_guest_base();
  // Apply page settings now so the game's text is patched before it loads.
  for (let i = queue.length - 1; i >= 0; i--) {
    if (["hidehud", "textrules", "earlymove"].includes(queue[i].kind)) applyConfig(queue.splice(i, 1)[0]);
  }

  const gl = canvas.getContext("webgl2", { antialias: false, depth: true, stencil: true, alpha: false, preserveDrawingBuffer: false });
  if (!gl) { postMessage({ type: "fatal", text: "WebGL2 is not available" }); return; }
  gles = new GLES(gl, memView, log);
  const np = exp.mc2_alloc(16384);
  const nn = exp.mc2_gl_names(np, 16384);
  gles.setNames(hostStr(np, nn).split("\n"));
  if (msg.flags) exp.mc2_set_flags(msg.flags);

  postMessage({ type: "status", text: "Booting game (first start takes a few seconds)…" });
  await new Promise((r) => setTimeout(r, 0));
  const t0 = performance.now();
  await waitUntilVisible();
  check(exp.mc2_boot(width, height, 0), "boot");
  // All missions selectable from the start (the save is loaded by now).
  if (msg.unlockAll) exp.mc2_unlock_all_missions();
  // Upgrade each browser's local save once, retaining the pre-upgrade save.
  // Use the game's setter and serializer rather than patching file offsets.
  if (!saves.has("web/max-wifi-rank-v1")) {
    for (const [path, data] of [...saves]) {
      if (!path.endsWith("SaveGame.bin")) continue;
      const backup = "web/before-max-wifi-rank/" + path;
      if (!saves.has(backup)) {
        const copy = data.slice();
        saves.set(backup, copy);
        persist(backup, copy);
      }
    }
    if (exp.mc2_unlock_multiplayer_weapons()) {
      const marker = new Uint8Array([1]);
      saves.set("web/max-wifi-rank-v1", marker);
      persist("web/max-wifi-rank-v1", marker);
    } else log("[unlock] could not save maximum Wi-Fi rank; will retry next start");
  }
  // First start only: aim assist off by default (the player can turn it back
  // on in Options -> Advanced; the game saves that choice).
  if (!saves.has("web/defaults-v1")) {
    exp.mc2_set_aim_assist(0);
    const marker = new Uint8Array([1]);
    saves.set("web/defaults-v1", marker);
    persist("web/defaults-v1", marker);
  }
  log(`boot took ${(performance.now() - t0).toFixed(0)} ms`);
  running = true;
  requestAnimationFrame(frame);
}

// Settings from the page that must reach the engine (also before boot).
function applyConfig(ev) {
  const send = (text, fn) => {
    const b = enc.encode(text);
    const p = exp.mc2_alloc(b.length + 1);
    memView.u8().set(b, p);
    fn(p, b.length);
    exp.mc2_free(p, b.length + 1);
  };
  if (ev.kind === "earlymove") exp.mc2_set_early_move(ev.on ? 1 : 0);
  else if (ev.kind === "hidehud") send(ev.names.join("\n"), exp.mc2_set_hidden_hud);
  else if (ev.kind === "textrules") send(ev.rules.map(([a, c]) => a.replace(/\n/g, "\\n") + "\t" + c.replace(/\n/g, "\\n")).join("\n"), exp.mc2_set_text_rules);
}

let locBuf = 0;
function hudLocate(patterns) {
  if (!locBuf) locBuf = exp.mc2_alloc(4096 + 65536);
  const req = enc.encode(patterns.join("\n"));
  memView.u8().set(req, locBuf);
  const n = exp.mc2_hud_locate(locBuf, req.length, gameW, gameH, locBuf + 4096, 65536);
  const [name, x, y] = hostStr(locBuf + 4096, n).split("\t");
  if (name === "!") return { found: false, visible: x };
  return { found: true, name, x: +x, y: +y };
}

let scanBuf = 0;
function hudScan(step) {
  if (!scanBuf) scanBuf = exp.mc2_alloc(65536);
  const n = exp.mc2_hud_scan(gameW, gameH, step, scanBuf, 65536);
  try {
    return JSON.parse(hostStr(scanBuf, n)).map(([name, x0, y0, x1, y1, cells, cx, cy, alpha]) => ({ name, x0, y0, x1, y1, cells, cx, cy, alpha }));
  } catch (e) {
    return [];
  }
}

// Push the HUD layout to the page whenever it changes (after it has been
// stable for a few frames).
let hudSig = -1, hudSigFrames = 0, hudSentSig = -1;
let gameState = -1;
function trackHud() {
  const st = exp.mc2_game_state();
  if (st !== gameState) {
    gameState = st;
    postMessage({ type: "gamestate", cutscene: !!(st & 1), paused: !!(st & 2), ads: !!(st & 4), loadout: !!(st & 8) });
  }
  const sig = exp.mc2_hud_signature();
  if (sig !== hudSig) {
    hudSig = sig;
    hudSigFrames = 0;
    return;
  }
  if (++hudSigFrames < 3 || sig === hudSentSig) return;
  hudSentSig = sig;
  const t = performance.now();
  const rects = sig ? hudScan(12) : [];
  postMessage({ type: "hudlayout", rects, ms: performance.now() - t });
}

const stats = new Uint32Array(8);
let lastStat = performance.now(), frames = 0, busy = 0;
let firstDrawReported = false;
function frame() {
  if (!running) return;
  try {
    for (const ev of queue.splice(0)) {
      if (ev.kind === "touch") check(exp.mc2_touch(ev.action, ev.x, ev.y, ev.id), "touch");
      else if (ev.kind === "key") check(exp.mc2_key(ev.down, ev.code), "key");
      else if (ev.kind === "pause") check(exp.mc2_pause(ev.paused), "pause");
      else if (ev.kind === "hidehud" || ev.kind === "textrules" || ev.kind === "earlymove") applyConfig(ev);
      else if (ev.kind === "locate") postMessage({ type: "located", id: ev.id, ...hudLocate(ev.patterns) });
      else if (ev.kind === "hudscan") postMessage({ type: "hud", id: ev.id, rects: hudScan(ev.step || 8) });
    }
    const t = performance.now();
    gles.drawCalls = 0;
    check(exp.mc2_frame(3000000), "frame");
    if (!firstDrawReported && gles.drawCalls > 0) {
      firstDrawReported = true;
      // Freeze on the first drawn frame until the intro has finished.
      running = false;
      postMessage({ type: "ready" });
    }
    busy += performance.now() - t;
    frames++;
    trackHud();
    if (t - lastStat > 1000) {
      const sp = exp.mc2_alloc(32);
      exp.mc2_stats(sp);
      const v = new Uint32Array(exp.memory.buffer, sp, 6);
      postMessage({ type: "stats", fps: (frames * 1000) / (t - lastStat), busy: busy / frames, icount: v[0] + v[1] * 4294967296, heap: v[2], draws: gles.drawCalls });
      exp.mc2_free(sp, 32);
      frames = 0; busy = 0; lastStat = t;
    }
  } catch (e) {
    log("frame error: " + e.stack);
    if (!failed) postMessage({ type: "fatal", text: `Game frame failed: ${e.message || e}` });
    return;
  }
  if (running) requestAnimationFrame(frame);
}

onmessage = (e) => {
  const m = e.data;
  switch (m.type) {
    case "visibility": pageVisible = m.visible; if (pageVisible) { for (const resolve of visibleWaiters.splice(0)) resolve(); } break;
    case "start": start(m).catch((err) => postMessage({ type: "fatal", text: String(err.stack || err) })); break;
    case "resume-game": if (firstDrawReported && !running && !failed) { running = true; requestAnimationFrame(frame); } break;
    case "touch": case "key": case "pause": case "hudscan": case "locate": case "hidehud": case "textrules": case "earlymove": queue.push({ kind: m.type, ...m }); break;
    case "duration": audio.durations.set(m.url, m.ms); break;
  }
};
