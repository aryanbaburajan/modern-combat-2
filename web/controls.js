// Keyboard & mouse controls.
//
// The game only understands two touch points (ids 0 and 1), like two thumbs.
// We synthesize those touches on the game's own HUD controls, whose names and
// rectangles the engine reports whenever the HUD layout changes:
//   finger 0: movement stick (WASD / arrows)
//   finger 1: camera drag (mouse look), fire button, and button taps
// Outside gameplay (menus, cutscenes) the mouse simply acts as a finger.

const MOVE = 0;
const RIGHT = 1;

// Key -> list of candidate HUD control names (first one present wins).
// Key -> HUD buttons to press. `names` are tried in order (a trailing "*"
// matches any control with that prefix); `taps` = 2 for a double tap.
// The game names context buttons "Action<n>" and weapon pickups "Weapon<n>".
// Its weapon icon reloads on a tap and switches weapon on a double tap; the
// grenade icon throws on a tap and switches grenade type on a double tap.
const USE = ["Action*", "Weapon*", "ClickableObject", "ClickableQTE", "ExitTurret", "GrenadeThrowback", "Melee"];
export const DEFAULT_BINDINGS = {
  ControlLeft: { names: ["crouch"] },
  ControlRight: { names: ["crouch"] },
  KeyC: { names: ["crouch"] },
  KeyR: { names: ["weapon_selector"] },
  KeyQ: { names: ["weapon_selector"], taps: 2 },
  KeyG: { names: ["grenade_selector"] },
  KeyH: { names: ["grenade_selector"], taps: 2 },
  KeyV: { names: ["airstrike_selector"] },
  KeyE: { names: USE },
  KeyF: { names: ["Melee", "MeleeBack", "ClickableQTE"] },
  Space: { names: ["ClickableQTE", ...USE] },
  KeyX: { names: ["iron_sight"] },
  KeyP: { names: ["PauseButton"] },
};
const MOUSE_RIGHT = { names: ["iron_sight"] };
const WHEEL = { names: ["weapon_selector"], taps: 2 };

// How each action is named in the game's own text (see prompts.js).
export const KEY_LABELS = {
  move: "W A S D",
  walk: "Alt",
  look: "the mouse",
  fire: "left mouse button",
  aim: "right mouse button",
  crouch: "Ctrl",
  reload: "R",
  switchWeapon: "Q",
  grenade: "G",
  switchGrenade: "H",
  airstrike: "V",
  use: "E",
  melee: "F",
  pause: "Esc",
  skip: "Space",
};
// The cutscene "skip" arrow is drawn by the menu layer, not the HUD.
const SKIP_POINT = [740, 28];
const MOVE_KEYS = {
  KeyW: [0, -1], ArrowUp: [0, -1],
  KeyS: [0, 1], ArrowDown: [0, 1],
  KeyA: [-1, 0], ArrowLeft: [-1, 0],
  KeyD: [1, 0], ArrowRight: [1, 0],
};

// localStorage throws when the browser blocks site storage; settings are optional.
function setting(key) {
  try {
    return localStorage.getItem(key);
  } catch (e) {
    return null;
  }
}

export class Controls {
  constructor({ canvas, W, H, sendTouch, onHint, locate }) {
    this.locate = locate; // (patterns) => Promise<{name, x, y} | null>
    this.canvas = canvas;
    this.W = W;
    this.H = H;
    this.send = sendTouch; // (action 0=up 1=down 2=move, x, y, id)
    this.onHint = onHint || (() => {});
    this.layout = new Map();
    this.inCutscene = false;
    this.paused = false;
    this.loadout = false;
    this.cameraSeenAt = -1e9;
    this.ads = false; // game reports aiming down the sights
    this.adsHeld = false; // right mouse button held
    this.adsEnterAt = -1e9;
    this.adsExitPendingUntil = 0;
    this.wantLock = false; // the player has chosen mouse capture this session
    this.bindings = { ...DEFAULT_BINDINGS };
    this.sensitivity = parseFloat(setting("mc2.sens") || "1.0") || 1.0;
    this.invertY = setting("mc2.invertY") === "1";
    this.keys = new Set();
    this.moveAnchor = null;
    this.fingers = [
      { down: false, x: 0, y: 0, owner: null },
      { down: false, x: 0, y: 0, owner: null },
    ];
    this.lookDx = 0;
    this.lookDy = 0;
    this.lastLook = 0;
    this.fireHeld = false;
    this.taps = []; // pending button taps
    this.passthrough = new Map(); // real pointerId -> finger (outside pointer lock)
    this.menuHover = null;
    this.menuClickHeld = false;
    this.menuInactive = false;
    this.install();
    requestAnimationFrame(() => this.tick());
  }

  // In a mission and controllable. The game's cutscene/pause flags are
  // authoritative; the HUD layout only needs to have shown the camera drag
  // area recently (it briefly disappears while the HUD is rebuilt, e.g.
  // right after a cutscene, which must not drop or block mouse capture).
  get gameplay() {
    return !this.inCutscene && !this.paused && !this.loadout && (this.layout.has("Thumbstick_camera") || performance.now() - this.cameraSeenAt < 1500);
  }
  get cutscene() {
    return this.inCutscene;
  }

  setGameState(st) {
    const wasPlaying = this.gameplay;
    this.ads = !!st.ads;
    // Right button released while the sights were still coming up: leave
    // as soon as the game reports we're in.
    if (this.ads && !this.adsHeld && performance.now() < this.adsExitPendingUntil) {
      this.adsExitPendingUntil = 0;
      this.tap(MOUSE_RIGHT);
    }
    if (!wasPlaying && !st.cutscene && !st.paused) this.releasePassthrough();
    this.inCutscene = !!st.cutscene;
    this.paused = !!st.paused;
    this.loadout = !!st.loadout;
    // A held menu touch must not turn into an armory click on transition.
    if (this.menuHover && (this.loadout || this.gameplay || this.inCutscene)) this.releaseMenuHover(true);
    this.leaveIfNotPlaying();
    // Back from the pause menu: re-capture the mouse if the browser still
    // considers the click on "resume" a fresh user gesture.
    if (!wasPlaying && this.gameplay && this.wantLock && !this.locked && navigator.userActivation && navigator.userActivation.isActive) {
      this.requestLock("resume");
    }
    this.updateHint();
  }

  leaveIfNotPlaying() {
    if (!this.gameplay) {
      if (this.locked && !this.forceLock) document.exitPointerLock();
      this.releaseAll();
    }
  }

  requestLock(why) {
    if (!this.gameplay) return;
    this.wantLock = true;
    this.lockRequestedAt = performance.now();
    try {
      const p = this.canvas.requestPointerLock();
      if (p && p.catch) p.catch((e) => console.log(`[controls] mouse capture refused (${why}): ${e.message}`));
    } catch (e) {
      console.log(`[controls] mouse capture failed (${why}): ${e.message}`);
    }
  }
  get locked() {
    return this.forceLock || document.pointerLockElement === this.canvas;
  }

  setLayout(rects) {
    // Hidden (fully faded) controls stay in the HUD tree; ignore them.
    this.layout = new Map(rects.filter((r) => r.alpha === undefined || r.alpha > 8).map((r) => [r.name, r]));
    this.anchor = null;
    if (this.layout.has("Thumbstick_camera")) this.cameraSeenAt = performance.now();
    else if (this.locked) {
      // Only leave mouse capture if the camera area stays gone.
      clearTimeout(this.leaveTimer);
      this.leaveTimer = setTimeout(() => this.leaveIfNotPlaying(), 1600);
    }
    this.updateHint();
    if (this.gameplay) this.releaseMenuHover(true);
  }

  updateHint() {
    if (this.gameplay && !this.locked) this.onHint("Click to capture the mouse — WASD move · Alt walk · mouse look · L-click fire · hold R-click aim · Ctrl crouch · R reload · Q/wheel weapon · G grenade · E use · F melee · Esc pause");
    else if (this.cutscene) this.onHint("Space / Enter: skip");
    else this.onHint("");
  }

  find(names) {
    for (const n of names) {
      if (n.endsWith("*")) {
        const prefix = n.slice(0, -1);
        const hits = [...this.layout.values()].filter((r) => r.name.startsWith(prefix)).sort((a, b) => a.name.localeCompare(b.name));
        if (hits.length) return hits[0];
      } else {
        const r = this.layout.get(n);
        if (r) return r;
      }
    }
    return null;
  }

  // ---- finger primitives ----
  press(f, x, y, owner) {
    const F = this.fingers[f];
    if (F.down) this.release(f);
    F.down = true;
    F.owner = owner;
    F.x = x;
    F.y = y;
    this.send(1, Math.round(x), Math.round(y), f);
  }
  moveTo(f, x, y) {
    const F = this.fingers[f];
    if (!F.down) return;
    F.x = Math.max(0, Math.min(this.W - 1, x));
    F.y = Math.max(0, Math.min(this.H - 1, y));
    this.send(2, Math.round(F.x), Math.round(F.y), f);
  }
  release(f) {
    const F = this.fingers[f];
    if (!F.down) return;
    F.down = false;
    F.owner = null;
    if (f === MOVE) this.moveAnchor = null;
    this.send(0, Math.round(F.x), Math.round(F.y), f);
  }
  // Lift touches that came straight from the mouse/touchscreen (e.g. a QTE
  // circle still held when a cutscene ends), so no finger is left stuck down.
  releasePassthrough() {
    for (const [, f] of this.passthrough) this.send(0, 0, 0, f);
    this.passthrough.clear();
  }

  releaseAll() {
    this.adsHeld = false;
    this.release(MOVE);
    this.release(RIGHT);
    this.fireHeld = false;
    this.keys.clear();
    this.taps = [];
  }

  // A point on the camera drag area that no other control covers.
  cameraAnchor() {
    if (this.anchor) return this.anchor;
    const cam = this.layout.get("Thumbstick_camera");
    if (!cam) return null;
    const others = [...this.layout.values()].filter((r) => r.name !== "Thumbstick_camera");
    const free = (x, y) => others.every((r) => x < r.x0 - 12 || x > r.x1 + 12 || y < r.y0 - 12 || y > r.y1 + 12);
    let best = null;
    for (let y = cam.y0 + 40; y <= cam.y1 - 40; y += 20) {
      for (let x = Math.max(cam.x0, this.W * 0.45); x <= cam.x1 - 40; x += 20) {
        if (!free(x, y)) continue;
        const d = Math.hypot(x - this.W * 0.62, y - this.H * 0.45);
        if (!best || d < best.d) best = { x, y, d };
      }
    }
    this.anchor = best || { x: (cam.x0 + cam.x1) / 2, y: (cam.y0 + cam.y1) / 2 };
    return this.anchor;
  }

  // Keep the touch origin fixed until release, even when HUD bounds shift.
  updateMovement() {
    let dx = 0, dy = 0;
    for (const k of this.keys) {
      const v = MOVE_KEYS[k];
      if (v) { dx += v[0]; dy += v[1]; }
    }
    const stick = this.find(["move_dead_zone", "Thumbstick_move", "MoveThumbstickContainer"]);
    if (this.fingers[MOVE].owner === "tap") {
      // finger borrowed for a button tap; movement resumes next frame
    } else if ((dx || dy) && stick) {
      if (this.fingers[MOVE].owner !== "move") {
        const cx = Math.round((stick.x0 + stick.x1) / 2);
        const cy = Math.round((stick.y0 + stick.y1) / 2);
        const radius = 0.3 * Math.min(stick.x1 - stick.x0, stick.y1 - stick.y0);
        this.press(MOVE, cx, cy, "move");
        this.moveAnchor = { cx, cy, radius };
      }
      const { cx, cy, radius } = this.moveAnchor;
      const len = Math.hypot(dx, dy);
      // Hold Alt to walk slowly (half stick deflection), as the game's
      // stealth hints suggest.
      const scale = (this.keys.has("AltLeft") || this.keys.has("AltRight") ? 0.45 : 1) * radius / len;
      this.moveTo(MOVE, cx + dx * scale, cy + dy * scale);
    } else if (this.fingers[MOVE].owner === "move") {
      this.release(MOVE);
    }

  }

  // ---- per-frame update ----
  tick() {
    requestAnimationFrame(() => this.tick());
    if (!this.gameplay || !this.locked) return;
    const now = performance.now();

    this.updateMovement();

    // Hold-to-aim: if the game left iron sights on its own (reload, weapon
    // switch) while the right button is still held, go back in as soon as it
    // lets us -- retrying, since it ignores the button mid-reload.
    if (this.adsHeld && !this.ads && now - this.adsEnterAt > 700 && !this.taps.length) {
      this.adsEnterAt = now;
      this.tap(MOUSE_RIGHT);
    }

    // Pending button taps (press for a few frames, then release).
    for (const t of this.taps) {
      if (t.state === 0) {
        if (now < t.notBefore || this.taps.some((o) => o !== t && o.state === 1 && o.x === t.x && o.y === t.y)) continue;
        const f = this.pickFinger();
        if (f < 0) continue;
        t.finger = f;
        this.press(f, t.x, t.y, "tap");
        t.state = 1;
        t.until = now + 50;
      } else if (t.state === 1 && now >= t.until) {
        this.release(t.finger);
        t.state = 2;
      }
    }
    this.taps = this.taps.filter((t) => t.state !== 2);

    // Mouse look / fire-drag on the right finger.
    const R = this.fingers[RIGHT];
    if (this.lookDx || this.lookDy) {
      const k = this.sensitivity * 0.6;
      const mx = this.lookDx * k, my = this.lookDy * k * (this.invertY ? -1 : 1);
      this.lookDx = this.lookDy = 0;
      if (R.owner === "fire") {
        this.moveTo(RIGHT, R.x + mx, R.y + my);
      } else if (R.owner === null || R.owner === "look") {
        const a = this.cameraAnchor();
        if (a) {
          if (R.owner !== "look") this.press(RIGHT, a.x, a.y, "look");
          let nx = R.x + mx, ny = R.y + my;
          // Re-centre the virtual finger before it reaches the screen edge.
          if (Math.abs(nx - a.x) > 140 || Math.abs(ny - a.y) > 110 || nx < 4 || ny < 4 || nx > this.W - 4 || ny > this.H - 4) {
            this.release(RIGHT);
            this.press(RIGHT, a.x, a.y, "look");
            nx = a.x + mx;
            ny = a.y + my;
          }
          this.moveTo(RIGHT, nx, ny);
          this.lastLook = now;
        }
      }
    } else if (R.owner === "look" && now - this.lastLook > 120) {
      this.release(RIGHT);
    }
  }

  pickFinger() {
    if (!this.fingers[RIGHT].down) return RIGHT;
    if (this.fingers[RIGHT].owner === "look") { this.release(RIGHT); return RIGHT; }
    if (!this.fingers[MOVE].down) return MOVE;
    // Both busy (moving + firing): borrow the movement finger for a moment;
    // movement resumes next frame because the keys are still held.
    if (this.fingers[MOVE].owner === "move") { this.release(MOVE); return MOVE; }
    return -1;
  }

  // Tap the first matching HUD button. The engine is asked for its position
  // at the moment of the key press, so buttons that track world objects work.
  tap(binding) {
    const names = binding.names;
    const count = binding.taps || 1;
    const fallback = this.find(names);
    const push = (x, y) => {
      const now = performance.now();
      for (let i = 0; i < count; i++) this.taps.push({ x, y, state: 0, notBefore: now + i * 120 });
    };
    // Buttons from the last HUD scan are tapped immediately; the engine is
    // only asked (one frame round-trip) for controls the scan didn't see,
    // such as prompts that follow objects in the world.
    if (fallback || !this.locate) {
      if (fallback) push(fallback.cx, fallback.cy);
      return;
    }
    this.locate(names).then((r) => {
      if (r.found) {
        console.log(`[controls] tapping ${r.name} at ${Math.round(r.x)},${Math.round(r.y)}`);
        push(r.x, r.y);
      }
      else if (fallback) push(fallback.cx, fallback.cy);
      else console.log(`[controls] no HUD button for ${names.join("/")}; visible HUD controls: ${r.visible || "(none)"}`);
    });
  }

  // Hold-to-aim: the game's iron-sight button toggles, so tap it on press
  // (if not already aiming) and again on release (if aiming).
  aim(down) {
    this.adsHeld = down;
    if (down) {
      if (!this.ads) {
        this.adsEnterAt = performance.now();
        this.tap(MOUSE_RIGHT);
      }
    } else if (this.ads) {
      this.tap(MOUSE_RIGHT);
    } else if (performance.now() - this.adsEnterAt < 800) {
      this.adsExitPendingUntil = performance.now() + 1500;
    }
  }

  fire(down) {
    this.fireHeld = down;
    const R = this.fingers[RIGHT];
    if (down) {
      const r = this.find(["Fire", "fire"]);
      if (!r) return;
      if (R.down) this.release(RIGHT);
      this.press(RIGHT, r.cx, r.cy, "fire");
    } else if (R.owner === "fire") {
      this.release(RIGHT);
    }
  }

  // ---- DOM events ----
  releaseMenuHover(cancel = false) {
    if (!this.menuHover || (this.menuInactive && !cancel)) return;
    const { x, y, finger } = this.menuHover;
    this.menuHover = null;
    // Move outside all controls before lifting during a screen transition.
    if (cancel) this.send(2, -1, -1, finger);
    this.send(0, cancel ? -1 : x, cancel ? -1 : y, finger);
  }

  hoverMenu(e) {
    if (e.pointerType !== "mouse" || this.menuInactive || this.loadout || this.locked || this.gameplay || this.inCutscene || this.menuClickHeld) return;
    const p = this.toGame(e);
    if (!this.menuHover) {
      const used = new Set(this.passthrough.values());
      const finger = used.has(0) ? (used.has(1) ? -1 : 1) : 0;
      if (finger < 0) return;
      this.menuHover = { ...p, finger };
      this.send(1, p.x, p.y, finger);
    } else {
      Object.assign(this.menuHover, p);
      this.send(2, p.x, p.y, this.menuHover.finger);
    }
  }

  toGame(e) {
    const r = this.canvas.getBoundingClientRect();
    return { x: Math.round(((e.clientX - r.left) / r.width) * this.W), y: Math.round(((e.clientY - r.top) / r.height) * this.H) };
  }

  install() {
    const c = this.canvas;
    c.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      if (this.locked) return; // buttons handled by mousedown/mouseup below
      if (this.gameplay && e.pointerType === "mouse") {
        this.releaseMenuHover(true);
        this.requestLock("click");
        return;
      }
      if (e.pointerType === "mouse" && e.button !== 0) return;
      if (e.pointerType === "mouse" && !this.loadout && !this.inCutscene) {
        this.hoverMenu(e);
        this.menuClickHeld = true;
        this.releaseMenuHover();
        c.setPointerCapture(e.pointerId);
        return;
      }
      if (e.pointerType !== "mouse") this.releaseMenuHover(true);
      if (e.pointerType === "mouse" && (this.layout.size || this.inCutscene || this.paused)) {
        console.log(`[controls] click passed to the game, not capturing the mouse: cutscene=${this.inCutscene} paused=${this.paused} cameraArea=${this.layout.has("Thumbstick_camera")}`);
      }
      // Plain touch/mouse passthrough (menus, touch screens).
      const used = new Set(this.passthrough.values());
      const f = used.has(0) ? (used.has(1) ? -1 : 1) : 0;
      if (f < 0) return;
      c.setPointerCapture(e.pointerId);
      this.passthrough.set(e.pointerId, f);
      const p = this.toGame(e);
      this.send(1, p.x, p.y, f);
    });
    c.addEventListener("pointermove", (e) => {
      if (this.locked) {
        this.lookDx += e.movementX;
        this.lookDy += e.movementY;
        return;
      }
      this.hoverMenu(e);
      const f = this.passthrough.get(e.pointerId);
      if (f === undefined) return;
      const p = this.toGame(e);
      this.send(2, p.x, p.y, f);
    });
    const up = (e) => {
      if (e.pointerType === "mouse" && this.menuClickHeld) {
        this.menuClickHeld = false;
        if (e.type !== "pointercancel") this.hoverMenu(e);
        return;
      }
      if (this.locked) return;
      const f = this.passthrough.get(e.pointerId);
      if (f === undefined) return;
      this.passthrough.delete(e.pointerId);
      const p = this.toGame(e);
      this.send(0, p.x, p.y, f);
    };
    c.addEventListener("pointerup", up);
    c.addEventListener("pointerenter", (e) => this.hoverMenu(e));
    // Pointer leave and blur keep synthetic menu touches held, preventing
    // Alt-Tab from activating the highlighted button.
    // While the mouse is captured, use mouse events for the buttons: pointer
    // events don't report a second button pressed while another is held
    // (e.g. aiming while firing).
    c.addEventListener("mousedown", (e) => {
      if (!this.locked) return;
      if (e.button === 0) this.fire(true);
      else if (e.button === 2) this.aim(true);
      e.preventDefault();
    });
    addEventListener("mouseup", (e) => {
      if (!this.locked) return;
      if (e.button === 0) this.fire(false);
      else if (e.button === 2) this.aim(false);
    });
    c.addEventListener("pointercancel", up);
    c.addEventListener("contextmenu", (e) => e.preventDefault());
    c.addEventListener("wheel", (e) => {
      if (this.locked) {
        if (!this.wheelUntil || performance.now() > this.wheelUntil) {
          this.wheelUntil = performance.now() + 350;
          this.tap(WHEEL);
        }
        e.preventDefault();
      }
    }, { passive: false });

    document.addEventListener("pointerlockerror", () => {
      console.log("[controls] the browser refused mouse capture; click the game again to retry");
      this.updateHint();
    });
    document.addEventListener("pointerlockchange", () => {
      this.releaseMenuHover(true);
      this.menuClickHeld = false;
      if (this.locked) this.releasePassthrough();
      if (!this.locked) {
        // Leaving mouse capture (Esc) pauses the game, like the pause button.
        const wasPlaying = this.fingers.some((f) => f.down) || this.gameplay;
        this.releaseAll();
        if (wasPlaying && this.gameplay) {
          const p = this.layout.get("PauseButton");
          if (p) { this.send(1, p.cx, p.cy, RIGHT); setTimeout(() => this.send(0, p.cx, p.cy, RIGHT), 80); }
        }
      }
      this.updateHint();
    });

    addEventListener("keydown", (e) => {
      if (this.cutscene && (e.code === "Space" || e.code === "Enter") && !e.repeat) {
        const [x, y] = [SKIP_POINT[0] * this.W / 800, SKIP_POINT[1] * this.H / 480];
        this.send(1, x, y, RIGHT);
        setTimeout(() => this.send(0, x, y, RIGHT), 150);
        e.preventDefault();
        return;
      }
      if (!this.gameplay || !this.locked) return;
      if (e.repeat) { e.preventDefault(); return; }
      this.keys.add(e.code);
      const b = this.bindings[e.code];
      if (b) this.tap(b);
      if (MOVE_KEYS[e.code] || b || e.code.startsWith("Alt")) e.preventDefault();
    });
    addEventListener("keyup", (e) => {
      this.keys.delete(e.code);
    });
    addEventListener("blur", () => {
      this.menuInactive = true;
      this.menuClickHeld = false;
      this.releasePassthrough();
      this.releaseAll();
    });
    addEventListener("focus", () => { this.menuInactive = false; });
    document.addEventListener("visibilitychange", () => {
      this.menuInactive = document.hidden;
      if (document.hidden) this.menuClickHeld = false;
    });
  }
}
