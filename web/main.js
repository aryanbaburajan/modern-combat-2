import { importGame } from "./local-game.js";
import { Controls, KEY_LABELS } from "./controls.js";
import { promptRules } from "./prompts.js";

// Main thread: canvas, input and audio. The game itself runs in worker.js.

const params = new URLSearchParams(location.search);
const assetBase = new URL("game/", location.href);
const assetUrl = (path) => new URL(path, assetBase).href;
const W = parseInt(params.get("w") || "800", 10);
const H = parseInt(params.get("h") || "480", 10);
const canvas = document.getElementById("screen");
const statusEl = document.getElementById("status");
const statsEl = document.getElementById("stats");
const logEl = document.getElementById("log");
canvas.width = W;
canvas.height = H;

let preparationStages = [];
let preparationProgress = 0;
let displayedProgress = 0;
let progressFrame = 0;
let bootPending = false;
let progressWaiters = [];
function renderPreparation() {
  progressFrame = 0;
  const start = document.getElementById("start");
  if (!start) return;
  if (displayedProgress < preparationProgress) {
    displayedProgress++;
    start.textContent = `Preparing game files… ${displayedProgress}%`;
    progressFrame = requestAnimationFrame(renderPreparation);
    return;
  }
  if (bootPending) start.textContent = "Booting engine…";
  for (const resolve of progressWaiters.splice(0)) resolve();
}
function updatePreparation(stage, fraction = 0) {
  const index = preparationStages.findIndex(item => item.name === stage);
  if (index < 0 || !Number.isFinite(fraction)) return;
  const total = preparationStages.reduce((sum, item) => sum + item.weight, 0);
  const completed = preparationStages.slice(0, index).reduce((sum, item) => sum + item.weight, 0);
  const progress = (completed + preparationStages[index].weight * Math.max(0, Math.min(1, fraction))) / total;
  preparationProgress = Math.max(preparationProgress, Math.min(99, Math.floor(progress * 100)));
  if (!progressFrame) progressFrame = requestAnimationFrame(renderPreparation);
}
function showBootWhenCaughtUp() {
  bootPending = true;
  if (!progressFrame) progressFrame = requestAnimationFrame(renderPreparation);
}
function waitForProgressAnimation() {
  if (!progressFrame) return Promise.resolve();
  return new Promise(resolve => progressWaiters.push(resolve));
}
function cancelProgressAnimation() {
  if (progressFrame) cancelAnimationFrame(progressFrame);
  progressFrame = 0;
  for (const resolve of progressWaiters.splice(0)) resolve();
}
let assetCacheReady = Promise.resolve(false);
if ("serviceWorker" in navigator && "caches" in window) {
  assetCacheReady = navigator.serviceWorker.register("sw.js", { updateViaCache: "none" })
    .then(() => navigator.serviceWorker.ready)
    .then(() => new Promise(resolve => {
      if (navigator.serviceWorker.controller) return resolve(true);
      const timer = setTimeout(() => resolve(false), 5000);
      navigator.serviceWorker.addEventListener("controllerchange", () => { clearTimeout(timer); resolve(true); }, { once: true });
    })).catch(() => false);
}

let logPending = [];
function addLog(s) {
  console.log(s);
  if (!logEl || !document.body.classList.contains("showlog")) return;
  if (logPending.length === 0) requestAnimationFrame(flushLog);
  logPending.push(s);
}
function flushLog() {
  const text = logPending.join("\n") + "\n";
  logPending = [];
  logEl.append(text);
  if (logEl.childNodes.length > 400) {
    while (logEl.childNodes.length > 200) logEl.firstChild.remove();
  }
  logEl.scrollTop = logEl.scrollHeight;
}

// ---------------- audio ----------------
let actx = null;
const buffers = new Map(); // url -> Promise<AudioBuffer>
const channels = new Map(); // channel -> {src, gain, url, loop, offset, started, paused}

function ensureAudio() {
  if (!actx) actx = new AudioContext();
  if (actx.state === "suspended" && !introPlaying) actx.resume();
}
function loadBuffer(url) {
  if (!buffers.has(url)) {
    buffers.set(
      url,
      fetch(url)
        .then((r) => r.arrayBuffer())
        .then((b) => actx.decodeAudioData(b))
        .then((buf) => {
          worker.postMessage({ type: "duration", url, ms: buf.duration * 1000 });
          return buf;
        })
        .catch((e) => {
          addLog(`[audio] failed to load ${url}: ${e}`);
          return null;
        })
    );
  }
  return buffers.get(url);
}
function stopChannel(ch) {
  const c = channels.get(ch);
  if (!c) return;
  c.token = {};
  try { c.src && c.src.stop(); } catch (e) {}
  channels.delete(ch);
}
function startChannel(ch, url, loop, vol, offset = 0) {
  stopChannel(ch);
  const token = {};
  const c = { url, loop, vol, token, src: null, gain: null, started: 0, offset, paused: false };
  channels.set(ch, c);
  loadBuffer(url).then((buf) => {
    if (!buf || c.token !== token || c.paused) return;
    const src = actx.createBufferSource();
    src.buffer = buf;
    src.loop = loop;
    const gain = actx.createGain();
    gain.gain.value = vol;
    src.connect(gain).connect(actx.destination);
    src.onended = () => { if (channels.get(ch) === c && !loop) channels.delete(ch); };
    src.start(0, offset % buf.duration);
    c.src = src;
    c.gain = gain;
    c.started = actx.currentTime - offset;
  });
}
function handleAudio(m) {
  if (!actx) return;
  switch (m.op) {
    case "load": loadBuffer(m.url); break;
    case "play": startChannel(m.ch, m.url, m.loop, Math.max(0, Math.min(1, m.vol))); break;
    case "stop": stopChannel(m.ch); break;
    case "volume": { const c = channels.get(m.ch); if (c) { c.vol = m.vol; if (c.gain) c.gain.gain.value = m.vol; } break; }
    case "pause": { const c = channels.get(m.ch); if (c && !c.paused) { c.paused = true; c.offset = actx.currentTime - c.started; try { c.src && c.src.stop(); } catch (e) {} c.src = null; } break; }
    case "resume": { const c = channels.get(m.ch); if (c && c.paused) startChannel(m.ch, c.url, c.loop, c.vol, c.offset); break; }
    case "stopAll":
      for (const ch of [...channels.keys()]) if ((ch === "music" && m.music) || (ch !== "music" && m.sounds)) stopChannel(ch);
      break;
    case "pauseAll": actx.suspend(); break;
    case "resumeAll": if (!introPlaying) actx.resume(); break;
  }
}

// ---------------- worker ----------------
const worker = new Worker("worker.js", { type: "module" });
let resolveGameReady, rejectGameReady;
const gameReady = new Promise((resolve, reject) => { resolveGameReady = resolve; rejectGameReady = reject; });
const networkEl = document.createElement("div");
networkEl.style.cssText = "position:absolute;top:8px;right:12px;color:#9f9;font:12px 'Geist Mono',ui-monospace,monospace;pointer-events:none";
document.body.append(networkEl);
worker.onmessage = (e) => {
  const m = e.data;
  switch (m.type) {
    case "network": networkEl.textContent = m.text; break;
    case "save-error": { const warning = document.getElementById("save-warning"); warning.textContent = m.text; warning.style.display = "block"; addLog(m.text); break; }
    case "log": addLog(m.text); break;
    case "preparation": updatePreparation("archives", m.progress); break;
    case "status": if (/Loading engine|Booting game/.test(m.text)) showBootWhenCaughtUp(); break;
    case "ready": resolveGameReady(); break;
    case "fatal": rejectGameReady(new Error(m.text.split("\n")[0])); statusEl.style.display = "block"; statusEl.textContent = "Error: " + m.text.split("\n")[0]; addLog(m.text); break;
    case "audio": handleAudio(m); break;
    case "gamestate":
      controls.setGameState(m);
      if (params.has("hudlog")) addLog(`[hud] cutscene=${m.cutscene}`);
      break;
    case "hudlayout":
      controls.setLayout(m.rects);
      if (params.has("hudlog")) addLog(`[hud] ${m.ms.toFixed(1)} ms: ${m.rects.map((r) => r.name + ":" + r.alpha).join(", ")}`);
      break;
    case "located": {
      const cb = locateWaiters.get(m.id);
      locateWaiters.delete(m.id);
      if (cb) cb(m);
      break;
    }
    case "hud": { const cb = hudWaiters.get(m.id); hudWaiters.delete(m.id); if (cb) cb(m.rects); break; }
    case "stats":
      if (!params.has("stats")) break;
      statsEl.textContent = `${m.fps.toFixed(0)} fps · ${m.busy.toFixed(1)} ms/frame · heap ${(m.heap / 1048576).toFixed(0)} MB · ${m.draws} draws`;
      break;
    case "exit": statusEl.style.display = "block"; statusEl.textContent = "Game exited."; break;
  }
};

// HUD layout query (names + rectangles of the game's on-screen controls).
const hudWaiters = new Map();
let hudReq = 0;
function hudScan(step = 8) {
  return new Promise((res) => {
    const id = ++hudReq;
    hudWaiters.set(id, res);
    worker.postMessage({ type: "hudscan", id, step });
  });
}
window.mc2HudScan = hudScan;

// Find a HUD button by name right now (handles buttons that move every frame).
const locateWaiters = new Map();
function hudLocate(patterns) {
  return new Promise((res) => {
    const id = ++hudReq;
    locateWaiters.set(id, res);
    worker.postMessage({ type: "locate", id, patterns });
  });
}

// ---------------- input ----------------
const controls = new Controls({
  canvas,
  W,
  H,
  locate: hudLocate,
  sendTouch: (action, x, y, id) => {
    ensureAudio();
    worker.postMessage({ type: "touch", action, x, y, id });
  },
  // No on-page hints: controls are explained in the game's own text.
  onHint: () => {},
});
window.mc2Controls = controls;

// On-screen touch controls the game draws (stick, fire, crouch, aim, pause…)
// are hidden for keyboard & mouse play; they still work as touch targets.
// They come back automatically when a touch screen is used, or with ?touchhud.
const TOUCH_ONLY_HUD = ["PauseButton", "iPodButton", "Debug", "crouch", "iron_sight", "Fire", "move_dead_zone", "Thumbstick_move", "MoveThumbstickContainer"];
let touchHud = params.has("touchhud");
function sendHiddenHud() {
  worker.postMessage({ type: "hidehud", names: touchHud ? [] : TOUCH_ONLY_HUD });
  // The game's text names keys instead of touch icons (unless playing by touch).
  worker.postMessage({ type: "textrules", rules: touchHud ? [] : promptRules(KEY_LABELS) });
}
sendHiddenHud();
// Let the player move and crouch from the start of the first mission.
worker.postMessage({ type: "earlymove", on: !params.has("noearlymove") });
canvas.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "touch" && !touchHud) {
    touchHud = true;
    sendHiddenHud();
  }
}, { capture: true });

let gameStarted = false;
document.addEventListener("visibilitychange", () => {
  worker.postMessage({ type: "visibility", visible: !document.hidden });
  if (gameStarted) worker.postMessage({ type: "pause", paused: document.hidden ? 1 : 0 });
  if (actx) document.hidden || introPlaying ? actx.suspend() : actx.resume();
});

function waitUntilVisible() {
  if (!document.hidden) return Promise.resolve();
  return new Promise(resolve => {
    const onVisibility = () => {
      if (document.hidden) return;
      document.removeEventListener("visibilitychange", onVisibility);
      resolve();
    };
    document.addEventListener("visibilitychange", onVisibility);
    onVisibility();
  });
}

// ---------------- intro video ----------------
// The player's original video is transcoded in a worker and cached locally.
let introPlaying = false;
async function prepareIntro() {
  const cache = await caches.open("mc2-local-game-v1");
  const convertedURL = assetUrl("converted-intro-v1.mp4");
  const saved = await cache.match(convertedURL);
  if (saved) return saved.blob();
  const source = await cache.match(assetUrl("GloftBPHP/data/intro/logo.mp4"));
  if (!source) throw new Error("The game data ZIP does not contain the intro video.");
  await waitUntilVisible();
  const { FFmpeg } = await import("./vendor/ffmpeg/index.js");
  await waitUntilVisible();
  const ffmpeg = new FFmpeg();
  let interrupted = false;
  const onVisibility = () => {
    if (document.hidden) { interrupted = true; ffmpeg.terminate(); }
  };
  document.addEventListener("visibilitychange", onVisibility);
  try {
    await ffmpeg.load({ coreURL: new URL("vendor/ffmpeg-core/ffmpeg-core.js", location.href).href, wasmURL: new URL("vendor/ffmpeg-core/ffmpeg-core.wasm", location.href).href });
    ffmpeg.on("progress", ({ progress }) => updatePreparation("conversion", progress));
    await ffmpeg.writeFile("intro.mp4", new Uint8Array(await source.arrayBuffer()));
    const code = await ffmpeg.exec(["-i", "intro.mp4", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "25", "-pix_fmt", "yuv420p", "-c:a", "copy", "-movflags", "+faststart", "converted.mp4"]);
    if (code !== 0) throw new Error(`Video conversion failed (code ${code}).`);
    const bytes = await ffmpeg.readFile("converted.mp4");
    const blob = new Blob([bytes], { type: "video/mp4" });
    await cache.put(convertedURL, new Response(blob, { headers: { "Content-Type": "video/mp4" } }));
    return blob;
  } catch (error) {
    if (!interrupted) throw error;
  } finally {
    document.removeEventListener("visibilitychange", onVisibility);
    ffmpeg.terminate();
  }
  await waitUntilVisible();
  return prepareIntro();
}
async function playIntro(blob) {
  const video = document.getElementById("intro");
  const skip = document.getElementById("skip");
  const objectURL = URL.createObjectURL(blob);
  introPlaying = true;
  if (actx) await actx.suspend();
  video.src = objectURL;
  video.style.display = "block";
  try {
    await new Promise((resolve, reject) => {
      let finished = false;
      const cleanup = () => {
        video.pause(); video.style.display = "none"; skip.style.display = "none";
        video.removeEventListener("ended", done); video.removeEventListener("error", fail);
        video.removeEventListener("pointerdown", reveal); skip.removeEventListener("click", done);
        removeEventListener("keydown", onKey, true);
      };
      const done = () => { if (!finished) { finished = true; cleanup(); resolve(); } };
      const fail = () => { if (!finished) { finished = true; cleanup(); reject(new Error("The converted intro could not be played.")); } };
      const reveal = () => { if (video.currentTime >= 8) skip.style.display = "block"; };
      const onKey = (e) => {
        if (video.currentTime < 8) return;
        if (skip.style.display === "block" && ["Space", "Enter", "Escape"].includes(e.code)) done();
        else reveal();
        e.preventDefault(); e.stopPropagation();
      };
      video.addEventListener("ended", done); video.addEventListener("error", fail);
      video.addEventListener("pointerdown", reveal); skip.addEventListener("click", done);
      addEventListener("keydown", onKey, true);
      video.play().catch(() => {
        statusEl.style.display = "block";
        statusEl.style.zIndex = "12";
        statusEl.textContent = "Click the video to play the intro.";
        video.addEventListener("click", () => {
          video.play().then(() => { statusEl.style.display = "none"; statusEl.style.zIndex = ""; }).catch(fail);
        }, { once: true });
      });
    });
  } finally {
    URL.revokeObjectURL(objectURL);
    introPlaying = false;
    statusEl.style.zIndex = "";
    if (actx) actx.resume();
  }
}

// Cache Storage is scoped to the browser profile and origin (including port).
const gameFilesEl = document.getElementById("game-files");
const gameFileStatusEl = document.getElementById("game-file-status");
const replaceFilesEl = document.getElementById("replace-files");
async function hasSavedGame() {
  const cache = await caches.open("mc2-local-game-v1");
  return !!(await cache.match(assetUrl("manifest.json")) && await cache.match(assetUrl("libsandstorm2.so")));
}
async function showSavedGame() {
  try {
    const saved = "caches" in window && await hasSavedGame();
    gameFilesEl.hidden = !!saved;
    replaceFilesEl.hidden = !saved;
    gameFileStatusEl.textContent = saved
      ? "Your game files are saved in this browser. Press Start to play."
      : "Select your APK and game data ZIP. Files stay in this browser and can be reused on this address.";
  } catch {
    gameFileStatusEl.textContent = "Select your APK and game data ZIP. Your browser must allow local storage.";
  }
}
replaceFilesEl.addEventListener("click", () => {
  gameFilesEl.hidden = false;
  replaceFilesEl.hidden = true;
  gameFileStatusEl.textContent = "Select both files to replace the saved game, or leave both empty to keep it.";
});
showSavedGame();
// ---------------- start ----------------
document.getElementById("start").addEventListener("click", async () => {
  const start = document.getElementById("start");
  if (typeof canvas.transferControlToOffscreen !== "function" || typeof Worker !== "function") {
    statusEl.style.display = "block";
    statusEl.textContent = "This browser does not support the game's canvas workers. Try a current version of Chrome, Edge, or Firefox.";
    return;
  }
  const apk = document.getElementById("apk").files[0];
  const data = document.getElementById("data-zip").files[0];
  start.disabled = true;
  document.body.classList.add("preparing");
  cancelProgressAnimation();
  preparationProgress = 0;
  displayedProgress = 0;
  bootPending = false;
  start.textContent = "Preparing game files… 0%";
  statusEl.style.display = "none";
  let intro;
  try {
    if (!await assetCacheReady) throw new Error("Local storage requires a secure connection and service worker support");
    const importing = !!(apk || data);
    if (importing && (!apk || !data)) throw new Error("Select both the APK and game data ZIP");
    const cache = await caches.open("mc2-local-game-v1");
    const needsConversion = !params.has("nointro") && (importing || !await cache.match(assetUrl("converted-intro-v1.mp4")));
    preparationStages = [
      ...(importing ? [{ name: "import", weight: 35 }] : []),
      ...(needsConversion ? [{ name: "conversion", weight: 35 }] : []),
      { name: "archives", weight: 30 },
    ];
    updatePreparation(preparationStages[0].name);
    if (importing) {
      if (navigator.storage?.persist) await navigator.storage.persist().catch(() => false);
      await importGame(apk, data, assetBase, m => { if (m.active && m.total) updatePreparation("import", m.loaded / m.total); }, waitUntilVisible);
    } else if (!await hasSavedGame()) {
      throw new Error("Select your APK and game data ZIP to start");
    }
    if (!params.has("nointro")) {
      intro = await prepareIntro();
    }
  } catch (e) {
    cancelProgressAnimation();
    document.body.classList.remove("preparing");
    start.disabled = false;
    start.textContent = "Start Modern Combat 2";
    statusEl.style.display = "block";
    statusEl.textContent = e?.message || String(e);
    return;
  }
  const menuFont = new FontFace("MC2", `url(${assetUrl("GloftBPHP/data/2d/fonts/ATROX.ttf")})`);
  menuFont.load().then(font => document.fonts.add(font)).catch(() => {});
  updatePreparation("archives");
  // Initialise the game first, then keep its first drawn frame paused under
  // the intro so no game time or input is lost while the video plays.
  introPlaying = !!intro;
  await waitUntilVisible();
  const off = canvas.transferControlToOffscreen();
  const flags = parseInt(params.get("debug") || "0", 10);
  worker.postMessage({ type: "start", visible: !document.hidden, canvas: off, width: W, height: H, flags, unlockAll: !params.has("locked"), assetBase: assetBase.href, room: params.get("room"), relayURL: globalThis.MC2_RELAY_URL || null, pageURL: location.href }, [off]);
  try {
    await gameReady;
    await waitUntilVisible();
    await waitForProgressAnimation();
    document.getElementById("setup").remove();
    statusEl.style.display = "none";
    if (intro) await playIntro(intro);
    document.body.classList.remove("preparing");
    await waitUntilVisible();
    ensureAudio();
    gameStarted = true;
    worker.postMessage({ type: "resume-game" });
  } catch (e) {
    cancelProgressAnimation();
    document.body.classList.remove("preparing");
    introPlaying = false;
    start.disabled = true;
    start.textContent = "Reload to retry";
    statusEl.style.display = "block";
    statusEl.textContent = `${e?.message || String(e)} Reload the page to retry.`;
  }
});
