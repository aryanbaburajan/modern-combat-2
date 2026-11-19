//! Game-specific helpers that inspect the running game (used by the
//! keyboard/mouse control layer to find on-screen HUD buttons).

use crate::Emu;

const HUD_ENGINE_UPDATE: &str = "_ZN9HUDEngine6UpdateEi";
const COLLIDE_TOUCH: &str = "_ZN10HUDControl17CollideTouchPointEN6glitch4core10position2dIfEEb";

pub struct HudRect {
    pub name: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub cells: u32,
    /// A hit cell near the middle of the control (safe point to press).
    pub cx: f32,
    pub cy: f32,
    /// Effective opacity 0..255 (minimum along the parent chain).
    pub alpha: u32,
}

fn effective_alpha(emu: &Emu, mut n: u32) -> u32 {
    let mut a = 255u32;
    for _ in 0..32 {
        if n == 0 || !emu.hle.heap.contains(n) {
            break;
        }
        a = a.min(emu.mem.r32(n + 0x50).min(255));
        n = emu.mem.r32(n + 0x64);
    }
    a
}

const START_CUTSCENE: &str = "_ZN3Hud13StartCutsceneEbb";
const END_CUTSCENE: &str = "_ZN3Hud11EndCutsceneEv";
const HUD_PAUSE: &str = "_ZN3Hud5PauseEbb";
const HUD_UNPAUSE: &str = "_ZN3Hud7UnPauseEb";
const CONTROL_RENDER: &str = "_ZN10HUDControl6RenderEv";
const HUD_UPDATE_CONTROLS: &str = "_ZN3Hud14UpdateControlsEib";
const CHAR_CROUCH: &str = "_ZN9Character6CrouchEv";
const CHAR_STAND: &str = "_ZN9Character5StandEv";

const PROBE_HUD: usize = 0;
const PROBE_CUT_START: usize = 1;
const PROBE_CUT_END: usize = 2;
const PROBE_PAUSE: usize = 3;
const PROBE_UNPAUSE: usize = 4;
pub const PROBE_CROUCH: usize = 5;
pub const PROBE_STAND: usize = 6;
const PROBE_HUD_OBJ: usize = 7;

/// Watch the game functions we need to observe.
pub fn install_probes(emu: &mut Emu) {
    for name in [HUD_ENGINE_UPDATE, START_CUTSCENE, END_CUTSCENE, HUD_PAUSE, HUD_UNPAUSE, CHAR_CROUCH, CHAR_STAND, HUD_UPDATE_CONTROLS] {
        let pc = emu.lib.find_func(name).unwrap_or(1);
        emu.probes.push(crate::Probe { pc, ..Default::default() });
    }
    emu.render_pc = emu.lib.find_func(CONTROL_RENDER).unwrap_or(1);
    install_text_hooks(emu);
}

/// Was `a` called more recently than `b`?
fn called_after(emu: &Emu, a: usize, b: usize) -> bool {
    match (emu.probes.get(a), emu.probes.get(b)) {
        (Some(x), Some(y)) => x.hits > 0 && x.last > y.last,
        _ => false,
    }
}

/// True while the in-mission pause menu (which holds the settings) is open.
pub fn is_paused(emu: &Emu) -> bool {
    called_after(emu, PROBE_PAUSE, PROBE_UNPAUSE)
}

/// Mirrors Gameplay::IsInWeaponSelectorScreen for the supported game library.
/// The armory is a MenuFX state over a running multiplayer world, so the camera
/// HUD alone cannot tell the input layer whether mouse capture is appropriate.
pub fn in_loadout(emu: &Emu) -> bool {
    let instance = |name: &str| emu.lib.exports.get(name).map(|p| emu.mem.r32(*p)).unwrap_or(0);
    let gameplay = instance("_ZN8Gameplay10s_instanceE");
    let manager = instance("_ZN11MenuManager10s_instanceE");
    if !emu.hle.heap.contains(gameplay) || !emu.hle.heap.contains(manager) || emu.mem.r32(gameplay + 0x40) == 0 {
        return false;
    }
    let menus = emu.mem.r32(manager + 0x10);
    let states = emu.mem.r32(gameplay + 0x18);
    if !emu.hle.heap.contains(menus) || !emu.hle.heap.contains(states) { return false; }
    let count = emu.mem.r32(menus + 0x11c);
    let stack = emu.mem.r32(menus + 0x118);
    if count == 0 || count > 256 || !emu.hle.heap.contains(stack) { return false; }
    let selected = emu.mem.r32(states + 0x38);
    selected != 0 && emu.mem.r32(stack + 4 * (count - 1)) == selected
}

/// Should HUDControl::Render skip this control?
pub fn is_hidden_control(emu: &Emu, node: u32) -> bool {
    if !emu.hle.heap.contains(node) {
        return false;
    }
    let np = emu.mem.r32(node + 0xcc);
    if np == 0 {
        return false;
    }
    let name = emu.mem.cstr_bytes(np);
    emu.hidden_hud.iter().any(|h| h.as_bytes() == name)
}

/// Root controls that currently receive touches, most recently updated first.
/// Each HUD engine keeps a stack of pushed layouts, but HUDEngine::Update only
/// hit-tests the top one (CollideCursor -> GetRoot), so lower layers -- which
/// can hold stale duplicates of buttons -- are ignored. Several engines may be
/// updated each frame; all of them see every touch.
pub fn hud_roots(emu: &Emu) -> Vec<u32> {
    let mut roots = Vec::new();
    let p = match emu.probes.get(PROBE_HUD) {
        Some(p) => p,
        None => return roots,
    };
    // Engines updated within roughly the last frame.
    let newest = p.seen.iter().map(|e| e.1).max().unwrap_or(0);
    let mut engines: Vec<(u32, u64)> = p.seen.iter().copied().filter(|e| newest - e.1 < 20_000_000).collect();
    engines.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (eng, _) in engines {
        if !emu.hle.heap.contains(eng) {
            continue;
        }
        let count = emu.mem.r32(eng + 4);
        if count == 0 || count > 16 {
            continue;
        }
        let r = emu.mem.r32(eng + 4 * (count + 1));
        if emu.hle.heap.contains(r) && !roots.contains(&r) {
            roots.push(r);
        }
    }
    roots
}

/// Topmost control under (x, y) across all HUD layers, with the layer root.
fn collide_any(emu: &mut Emu, collide: u32, roots: &[u32], x: f32, y: f32) -> Option<(u32, u32)> {
    let pt = emu.hle.scratch + 3000;
    for &root in roots {
        emu.mem.w32(pt, x.to_bits());
        emu.mem.w32(pt + 4, y.to_bits());
        let hit = emu.call_nested(collide, &[root, pt, 0]).0;
        let faulted = emu.cpu.fault.take().is_some();
        if hit != 0 && !faulted {
            return Some((hit, root));
        }
    }
    None
}

/// True while an in-engine cutscene is playing.
pub fn in_cutscene(emu: &Emu) -> bool {
    // Two signals: the flag Hud::StartCutscene/EndCutscene keep at Hud+0x7d4,
    // and which of those two was called last. A cutscene needs both, so a
    // missed EndCutscene call or a stale Hud pointer alone can't leave us
    // stuck "in a cutscene" (which would block mouse capture).
    let by_calls = called_after(emu, PROBE_CUT_START, PROBE_CUT_END);
    match valid_hud(emu) {
        Some(hud) => by_calls && emu.mem.r8(hud + 0x7d4) != 0,
        None => by_calls,
    }
}

/// The Hud object, if the pointer we captured still looks like one: a live
/// Hud holds one of the HUD engines being updated at +4.
fn valid_hud(emu: &Emu) -> Option<u32> {
    let hud = emu.probes.get(PROBE_HUD_OBJ)?.r0;
    if !emu.hle.heap.contains(hud) {
        return None;
    }
    let eng = emu.mem.r32(hud + 4);
    let engines = &emu.probes.get(PROBE_HUD)?.seen;
    if engines.iter().any(|e| e.0 == eng) {
        Some(hud)
    } else {
        None
    }
}

/// The QTE target is activated by both Hud::StartQTE overloads and
/// deactivated by Hud::EndQTE. Read it directly since it can move each frame.
pub fn in_qte(emu: &Emu) -> bool {
    let Some(hud) = valid_hud(emu) else { return false; };
    let target = emu.mem.r32(hud + 0x624);
    emu.hle.heap.contains(target) && emu.mem.r8(target + 0x60) != 0
}

/// Hit-test a grid over the screen with the game's own touch collision and
/// return the bounding box of every control that answered.
pub fn hud_scan(emu: &mut Emu, w: u32, h: u32, step: u32) -> Vec<HudRect> {
    let mut out: Vec<HudRect> = Vec::new();
    let collide = match emu.lib.find_func(COLLIDE_TOUCH) {
        Some(a) => a | 1,
        None => return out,
    };
    let roots = hud_roots(emu);
    if roots.is_empty() {
        return out;
    }
    let mut cells: Vec<(usize, f32, f32)> = Vec::new();
    let step = step.max(2);
    let mut y = step / 2;
    while y < h {
        let mut x = step / 2;
        while x < w {
            if let Some((hit, root)) = collide_any(emu, collide, &roots, x as f32, y as f32) {
                let (fx, fy) = (x as f32, y as f32);
                let alpha = effective_alpha(emu, hit);
                // Credit the point to the control that was hit and to each of
                // its named ancestors (buttons often contain unnamed icons).
                let mut node = hit;
                let mut depth = 0;
                while node != 0 && node != root && depth < 8 && emu.hle.heap.contains(node) {
                    let np = emu.mem.r32(node + 0xcc);
                    if np != 0 || depth == 0 {
                        let name = if np != 0 { emu.mem.cstr(np) } else { format!("@{:x}", node) };
                        let idx = match out.iter().position(|r| r.name == name) {
                            Some(i) => i,
                            None => out.len(),
                        };
                        cells.push((idx, fx, fy));
                        match out.get_mut(idx) {
                            Some(r) => {
                                r.x0 = r.x0.min(fx);
                                r.y0 = r.y0.min(fy);
                                r.x1 = r.x1.max(fx);
                                r.y1 = r.y1.max(fy);
                                r.cells += 1;
                            }
                            None => out.push(HudRect { name, x0: fx, y0: fy, x1: fx, y1: fy, cells: 1, cx: fx, cy: fy, alpha }),
                        }
                    }
                    node = emu.mem.r32(node + 0x64);
                    depth += 1;
                }
            }
            x += step;
        }
        y += step;
    }
    for (i, r) in out.iter_mut().enumerate() {
        let (mx, my) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
        let mut best = f32::MAX;
        for &(_, x, y) in cells.iter().filter(|c| c.0 == i) {
            let d = (x - mx) * (x - mx) + (y - my) * (y - my);
            if d < best {
                best = d;
                r.cx = x;
                r.cy = y;
            }
        }
    }
    out
}

/// Name of the HUD control under (x, y), if any.
pub fn hud_hit(emu: &mut Emu, x: f32, y: f32) -> Option<String> {
    let collide = emu.lib.find_func(COLLIDE_TOUCH)? | 1;
    let roots = hud_roots(emu);
    let (hit, _) = collide_any(emu, collide, &roots, x, y)?;
    let np = emu.mem.r32(hit + 0xcc);
    Some(if np != 0 { emu.mem.cstr(np) } else { format!("@{:x}", hit) })
}

/// Cheap fingerprint of the HUD layout (which controls are active and where),
/// computed by walking the control tree in guest memory without running guest
/// code. Changes whenever buttons appear, disappear or move.
pub fn hud_signature(emu: &Emu) -> u64 {
    let roots = hud_roots(emu);
    if roots.is_empty() {
        return 0;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u32| {
        h ^= v as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    };
    let mut stack = roots.clone();
    let mut visited = 0;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 8192 {
            break;
        }
        let m = &emu.mem;
        mix(n);
        mix(m.r8(n + 0x60) as u32 | (m.r8(n + 0x62) as u32) << 8 | ((m.r32(n + 0x50) > 16) as u32) << 16);
        // Positions are deliberately left out: some controls (e.g. clickable
        // world objects) follow 3D objects and move every frame.
        if m.r8(n + 0x60) == 0 {
            continue;
        }
        let cnt = m.r32(n + 0x68);
        let arr = m.r32(n + 0x6c);
        if cnt > 256 || !emu.hle.heap.contains(arr) {
            continue;
        }
        for i in 0..cnt {
            let c = m.r32(arr + 4 * i);
            if emu.hle.heap.contains(c) {
                stack.push(c);
            }
        }
    }
    h
}

fn name_matches(name: &str, pat: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pat,
    }
}

/// Find the first active, visible control matching one of `patterns` (in
/// priority order; a trailing `*` matches a prefix) and return its name and a
/// point on it that the game's touch collision accepts. Works for controls
/// that move every frame because it is evaluated at call time.
pub fn hud_locate(emu: &mut Emu, patterns: &[&str], w: u32, h: u32) -> Option<(String, f32, f32)> {
    let roots = hud_roots(emu);
    let collide = emu.lib.find_func(COLLIDE_TOUCH)? | 1;
    // Collect candidate nodes from all HUD layers.
    let mut found: Vec<(usize, String, u32)> = Vec::new();
    let mut stack = roots.clone();
    let mut visited = 0;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 8192 {
            break;
        }
        let active = emu.mem.r8(n + 0x60) != 0;
        let drawn = emu.mem.r8(n + 0x38) != 0;
        if !active && !drawn {
            continue;
        }
        let np = emu.mem.r32(n + 0xcc);
        if np != 0 {
            let name = emu.mem.cstr(np);
            if let Some(pi) = patterns.iter().position(|p| name_matches(&name, p)) {
                if effective_alpha(emu, n) > 8 {
                    found.push((pi, name, n));
                }
            }
        }
        let cnt = emu.mem.r32(n + 0x68);
        let arr = emu.mem.r32(n + 0x6c);
        if cnt > 256 || !emu.hle.heap.contains(arr) {
            continue;
        }
        for i in 0..cnt {
            let c = emu.mem.r32(arr + 4 * i);
            if emu.hle.heap.contains(c) {
                stack.push(c);
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut fallback: Option<(usize, String, f32, f32)> = None;
    // Lazily computed full-screen hit map: (x, y, hit chain) per grid cell.
    let mut screen: Option<Vec<(f32, f32, Vec<u32>)>> = None;
    let chain = |emu: &Emu, mut hit: u32| -> Vec<u32> {
        let mut v = Vec::new();
        while hit != 0 && v.len() < 8 && emu.hle.heap.contains(hit) {
            v.push(hit);
            hit = emu.mem.r32(hit + 0x64);
        }
        v
    };
    for (prio, name, node) in found {
        // First search around the control's stored position (cheap), then
        // the whole screen: the stored position is often not where the
        // control is drawn (it can be relative to an anchor).
        let ax = f32::from_bits(emu.mem.r32(node + 0xfc)) + f32::from_bits(emu.mem.r32(node + 0xdc));
        let ay = f32::from_bits(emu.mem.r32(node + 0x100)) + f32::from_bits(emu.mem.r32(node + 0xe0));
        let mut hits: Vec<(f32, f32)> = Vec::new();
        if ax.is_finite() && ay.is_finite() {
            let mut dy = -96.0;
            while dy <= 96.0 {
                let mut dx = -96.0;
                while dx <= 96.0 {
                    let (x, y) = (ax + dx, ay + dy);
                    if x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32 {
                        let hit = collide_any(emu, collide, &roots, x, y).map(|h| h.0).unwrap_or(0);
                        if chain(emu, hit).contains(&node) {
                            hits.push((x, y));
                        }
                    }
                    dx += 8.0;
                }
                dy += 8.0;
            }
        }
        if hits.is_empty() {
            if screen.is_none() {
                let mut cells = Vec::new();
                let mut y = 6.0;
                while y < h as f32 {
                    let mut x = 6.0;
                    while x < w as f32 {
                        let hit = collide_any(emu, collide, &roots, x, y).map(|h| h.0).unwrap_or(0);
                        cells.push((x, y, chain(emu, hit)));
                        x += 12.0;
                    }
                    y += 12.0;
                }
                screen = Some(cells);
            }
            hits = screen.as_ref().unwrap().iter().filter(|c| c.2.contains(&node)).map(|c| (c.0, c.1)).collect();
        }
        if hits.is_empty() {
            // Ask the control itself: its touch-zone test (vtable +0x60, as
            // used by HUDControl::IsTouchPointInZone) works even when the
            // generic hit test can't reach it.
            let vt = emu.mem.r32(node);
            let zone_fn = emu.mem.r32(vt.wrapping_add(0x60));
            if vt != 0 && zone_fn != 0 && (zone_fn & !1) >= emu.lib.text_start && (zone_fn & !1) < emu.lib.text_end {
                let pt = emu.hle.scratch + 3000;
                let mut y = 6.0;
                while y < h as f32 {
                    let mut x = 6.0;
                    while x < w as f32 {
                        emu.mem.w32(pt, (x as f32).to_bits());
                        emu.mem.w32(pt + 4, (y as f32).to_bits());
                        let inside = emu.call_nested(zone_fn, &[node, pt]).0 & 0xff;
                        emu.cpu.fault = None;
                        if inside != 0 {
                            hits.push((x, y));
                        }
                        x += 12.0;
                    }
                    y += 12.0;
                }
            }
        }
        if hits.is_empty() {
            // ClickableObject (an item in the world) sits under the
            // full-screen camera area, so the hit test never reaches it; the
            // game checks its own touch zone, centred on its position.
            if name == "ClickableObject" && fallback.is_none() && ax.is_finite() && ay.is_finite() && ax >= 0.0 && ay >= 0.0 && ax < w as f32 && ay < h as f32 {
                fallback = Some((prio, name, ax, ay));
            }
            continue;
        }
        let (sx, sy) = hits.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mx, my) = (sx / hits.len() as f32, sy / hits.len() as f32);
        let best = hits.iter().min_by(|a, b| ((a.0 - mx).powi(2) + (a.1 - my).powi(2)).partial_cmp(&((b.0 - mx).powi(2) + (b.1 - my).powi(2))).unwrap()).copied().unwrap();
        // A higher-priority match that could only be located by position wins.
        if let Some((fp, fname, fx, fy)) = fallback {
            if fp < prio {
                return Some((fname, fx, fy));
            }
        }
        return Some((name, best.0, best.1));
    }
    fallback.map(|(_, n, x, y)| (n, x, y))
}

/// Names of all active, visible controls across HUD layers (diagnostics).
pub fn hud_visible_names(emu: &Emu) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = hud_roots(emu);
    let mut visited = 0;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 8192 {
            break;
        }
        if emu.mem.r8(n + 0x60) == 0 {
            continue;
        }
        let np = emu.mem.r32(n + 0xcc);
        if np != 0 && effective_alpha(emu, n) > 8 {
            let name = emu.mem.cstr(np);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let cnt = emu.mem.r32(n + 0x68);
        let arr = emu.mem.r32(n + 0x6c);
        if cnt > 256 || !emu.hle.heap.contains(arr) {
            continue;
        }
        for i in 0..cnt {
            let c = emu.mem.r32(arr + 4 * i);
            if emu.hle.heap.contains(c) {
                stack.push(c);
            }
        }
    }
    names
}

const STRING_MGR_GET: &str = "_ZN9StringMgr3GetEv";

/// The game's StringMgr singleton (via StringMgr::Get()).
pub fn string_mgr(emu: &mut Emu) -> Option<u32> {
    let f = emu.lib.find_func(STRING_MGR_GET)? | 1;
    let m = emu.call_nested(f, &[]).0;
    if emu.hle.heap.contains(m) || (m >= crate::mem::LIB_BASE && m < crate::mem::LIB_BASE + emu.lib.size) {
        Some(m)
    } else {
        None
    }
}

/// All loaded string tables: (table index, buffer base, offsets array, count).
pub fn string_tables(emu: &mut Emu) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    let mgr = match string_mgr(emu) {
        Some(m) => m,
        None => return out,
    };
    let n = emu.mem.r32(mgr + 4);
    let arr = emu.mem.r32(mgr + 8);
    if n > 256 || !emu.hle.heap.contains(arr) {
        return out;
    }
    for t in 0..n {
        let tab = emu.mem.r32(arr + 4 * t);
        if !emu.hle.heap.contains(tab) {
            continue;
        }
        let count = emu.mem.r32(tab + 0x24);
        let base = emu.mem.r32(tab + 0x28);
        let offs = emu.mem.r32(tab + 0x2c);
        if count > 100_000 || !emu.hle.heap.contains(offs) {
            continue;
        }
        out.push((t, base, offs, count));
    }
    out
}

// ---------------------------------------------------------------------------
// Text substitution
//
// The game's localized text lives in StringTables: a UTF-8 buffer (+0x28), an
// array of offsets into it (+0x2c, count at +0x24) and a name->string map
// (std::map header at +0x38; nodes: left +8, right +0xc, key +0x10, value
// +0x14). We rewrite selected strings (e.g. "tap the crouch icon" -> "press
// Ctrl") by writing new text into guest memory and repointing the offset/map
// entries. Tables are patched lazily the first time the game reads from them
// (StringTable::GetString entry), so even text read once at load time is
// covered. Original pointers are kept so rules can be changed or removed.

const STRING_TABLE_GET_BY_ID: &str = "_ZNK11StringTable9GetStringEi";
const STRING_TABLE_GET_BY_NAME: &str = "_ZN11StringTable9GetStringEPKc";

#[derive(Default)]
pub struct TextPatcher {
    pub rules: Vec<(String, String)>,
    pub version: u32,
    /// table -> (base, offs, count, rules version) it was patched with
    patched: std::collections::HashMap<u32, (u32, u32, u32, u32)>,
    /// (address, original value) of every word we overwrote
    saved: Vec<(u32, u32)>,
    pub hook_pcs: [u32; 2],
    pub hook_hits: u64,
}

pub fn install_text_hooks(emu: &mut Emu) {
    emu.text.hook_pcs = [
        emu.lib.find_func(STRING_TABLE_GET_BY_ID).unwrap_or(1),
        emu.lib.find_func(STRING_TABLE_GET_BY_NAME).unwrap_or(1),
    ];
}

/// Replace the substitution rules; tables are re-patched on next access.
pub fn set_text_rules(emu: &mut Emu, rules: Vec<(String, String)>) {
    // Restore every original pointer first.
    for &(addr, orig) in emu.text.saved.iter().rev() {
        emu.mem.w32(addr, orig);
    }
    emu.text.saved.clear();
    emu.text.patched.clear();
    emu.text.rules = rules;
    emu.text.version += 1;
}

/// Called on entry to StringTable::GetString with the table in r0.
pub fn on_string_table_access(emu: &mut Emu, table: u32) {
    emu.text.hook_hits += 1;
    if emu.text.rules.is_empty() || !emu.hle.heap.contains(table) {
        return;
    }
    let count = emu.mem.r32(table + 0x24);
    let base = emu.mem.r32(table + 0x28);
    let offs = emu.mem.r32(table + 0x2c);
    let key = (base, offs, count, emu.text.version);
    if emu.text.patched.get(&table) == Some(&key) {
        return;
    }
    emu.text.patched.insert(table, key);
    if count > 100_000 || !emu.hle.heap.contains(offs) {
        return;
    }
    let mut moved: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for i in 0..count {
        let slot = offs + 4 * i;
        let orig_off = emu.mem.r32(slot);
        let addr = base.wrapping_add(orig_off);
        let text = emu.mem.cstr_bytes(addr);
        if text.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(text).into_owned();
        let mut out = s.clone();
        for (from, to) in emu.text.rules.iter() {
            match from.strip_prefix('=') {
                // whole-string rule
                Some(exact) => {
                    if out == exact {
                        out = to.clone();
                    }
                }
                None => {
                    if out.contains(from.as_str()) {
                        out = out.replace(from.as_str(), to);
                    }
                }
            }
        }
        if out == s {
            continue;
        }
        let p = emu.hle.heap.malloc(&mut emu.mem, out.len() as u32 + 1);
        if p == 0 {
            continue;
        }
        emu.mem.write_cstr(p, out.as_bytes());
        emu.text.saved.push((slot, orig_off));
        emu.mem.w32(slot, p.wrapping_sub(base));
        moved.insert(addr, p);
    }
    if moved.is_empty() {
        return;
    }
    // Name-keyed lookups return pointers stored in the map nodes.
    let header = table + 0x38;
    let mut stack = vec![emu.mem.r32(header + 4)];
    let mut visited = 0;
    while let Some(n) = stack.pop() {
        if n == 0 || n == header || !emu.hle.heap.contains(n) {
            continue;
        }
        visited += 1;
        if visited > 200_000 {
            break;
        }
        let v = emu.mem.r32(n + 0x14);
        if let Some(&p) = moved.get(&v) {
            emu.text.saved.push((n + 0x14, v));
            emu.mem.w32(n + 0x14, p);
        }
        stack.push(emu.mem.r32(n + 8));
        stack.push(emu.mem.r32(n + 0xc));
    }
}

// ---------------------------------------------------------------------------
// Player / objective access (testing tools)

/// The player character (Hud::GetAttachedCharacter: Hud+0x4c).
pub fn player(emu: &Emu) -> Option<u32> {
    let hud = emu.probes.get(PROBE_HUD_OBJ)?.r0;
    if !emu.hle.heap.contains(hud) {
        return None;
    }
    let c = emu.mem.r32(hud + 0x4c);
    if emu.hle.heap.contains(c) {
        Some(c)
    } else {
        None
    }
}

fn vec3(emu: &Emu, a: u32) -> [f32; 3] {
    [f32::from_bits(emu.mem.r32(a)), f32::from_bits(emu.mem.r32(a + 4)), f32::from_bits(emu.mem.r32(a + 8))]
}

/// Player position (GameObject::GetPosition: this+0xa0).
pub fn player_pos(emu: &Emu) -> Option<[f32; 3]> {
    player(emu).map(|c| vec3(emu, c + 0xa0))
}

/// Current objective point (set by Character::RefreshObjectivePosition).
pub fn objective_pos(emu: &Emu) -> Option<[f32; 3]> {
    player(emu).map(|c| vec3(emu, c + 0x228))
}

/// Move the player (GameObject::SetPosition).
pub fn set_player_pos(emu: &mut Emu, p: [f32; 3]) -> bool {
    let (c, f) = match (player(emu), emu.lib.find_func("_ZN10GameObject11SetPositionERKN6glitch4core8vector3dIfEE")) {
        (Some(c), Some(f)) => (c, f),
        _ => return false,
    };
    let v = emu.hle.scratch + 3100;
    for i in 0..3 {
        emu.mem.w32(v + 4 * i as u32, p[i].to_bits());
    }
    emu.call_nested(f | 1, &[c, v]);
    emu.cpu.fault = None;
    true
}

/// Every control in the touch-receiving HUD trees, including inactive and
/// hidden ones: (depth, name, active, visible, touchable, alpha, abs x, abs y).
pub fn hud_dump(emu: &Emu) -> Vec<(u32, String, u8, u8, u8, u32, f32, f32)> {
    let mut out = Vec::new();
    let mut stack: Vec<(u32, u32)> = hud_roots(emu).into_iter().map(|r| (r, 0)).collect();
    while let Some((n, d)) = stack.pop() {
        if out.len() > 4096 {
            break;
        }
        let m = &emu.mem;
        let np = m.r32(n + 0xcc);
        let name = if np != 0 { m.cstr(np) } else { format!("@{:x}", n) };
        let ax = f32::from_bits(m.r32(n + 0xfc)) + f32::from_bits(m.r32(n + 0xdc));
        let ay = f32::from_bits(m.r32(n + 0x100)) + f32::from_bits(m.r32(n + 0xe0));
        out.push((d, name, m.r8(n + 0x60), m.r8(n + 0x38), m.r8(n + 0x62), m.r32(n + 0x50), ax, ay));
        let cnt = m.r32(n + 0x68);
        let arr = m.r32(n + 0x6c);
        if cnt > 256 || !emu.hle.heap.contains(arr) {
            continue;
        }
        for i in (0..cnt).rev() {
            let c = m.r32(arr + 4 * i);
            if emu.hle.heap.contains(c) {
                stack.push((c, d + 1));
            }
        }
    }
    out
}

/// Touch zone of a control found by name in any touch-receiving HUD tree,
/// using the control's own zone test regardless of its flags (diagnostics).
pub fn hud_zone(emu: &mut Emu, name: &str, w: u32, h: u32) -> Option<(f32, f32, f32, f32, u32)> {
    let mut stack = hud_roots(emu);
    let mut node = 0;
    while let Some(n) = stack.pop() {
        let np = emu.mem.r32(n + 0xcc);
        if np != 0 && emu.mem.cstr(np) == name {
            node = n;
            break;
        }
        let cnt = emu.mem.r32(n + 0x68);
        let arr = emu.mem.r32(n + 0x6c);
        if cnt <= 256 && emu.hle.heap.contains(arr) {
            for i in 0..cnt {
                let c = emu.mem.r32(arr + 4 * i);
                if emu.hle.heap.contains(c) {
                    stack.push(c);
                }
            }
        }
    }
    if node == 0 {
        return None;
    }
    let zone_fn = emu.mem.r32(emu.mem.r32(node).wrapping_add(0x60));
    let pt = emu.hle.scratch + 3000;
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN, 0);
    let mut y = 4.0;
    while y < h as f32 {
        let mut x = 4.0;
        while x < w as f32 {
            emu.mem.w32(pt, (x as f32).to_bits());
            emu.mem.w32(pt + 4, (y as f32).to_bits());
            if emu.call_nested(zone_fn, &[node, pt]).0 & 0xff != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                n += 1;
            }
            emu.cpu.fault = None;
            x += 8.0;
        }
        y += 8.0;
    }
    Some((x0, y0, x1, y1, n))
}

/// HUD elements managed by Hud::ActivateElement/DeactivateElement:
/// (index, control name, state, active, visible). Table at Hud+(i+0xd7)*8.
pub fn hud_elements(emu: &Emu) -> Vec<(u32, String, u32, u8, u8)> {
    let mut out = Vec::new();
    let hud = match emu.probes.get(PROBE_HUD_OBJ) {
        Some(p) if emu.hle.heap.contains(p.r0) => p.r0,
        _ => return out,
    };
    for i in 0..48u32 {
        let e = hud + (i + 0xd7) * 8;
        let c = emu.mem.r32(e + 4);
        let state = emu.mem.r32(e + 8);
        if !emu.hle.heap.contains(c) {
            continue;
        }
        let np = emu.mem.r32(c + 0xcc);
        let name = if emu.hle.heap.contains(np) || np >= crate::mem::LIB_BASE { emu.mem.cstr(np) } else { String::new() };
        out.push((i, name, state, emu.mem.r8(c + 0x60), emu.mem.r8(c + 0x38)));
    }
    out
}

/// Unlock every mission on every difficulty with the game's own
/// GameSettings::UnlockAll(). Call after boot (the save is loaded by then);
/// the result is saved with the player's progress. Extreme difficulty's own
/// lock (GameSettings+0x11) is left as it was.
pub fn unlock_all_missions(emu: &mut Emu) -> bool {
    let (get, unlock) = match (emu.lib.find_func("_ZN12GameSettings11GetInstanceEv"), emu.lib.find_func("_ZN12GameSettings9UnlockAllEv")) {
        (Some(g), Some(u)) => (g, u),
        _ => return false,
    };
    let gs = emu.call_nested(get | 1, &[]).0;
    if !emu.hle.heap.contains(gs) {
        return false;
    }
    let extreme = emu.mem.r8(gs + 0x11);
    emu.call_nested(unlock | 1, &[gs]);
    emu.mem.w8(gs + 0x11, extreme);
    let ok = emu.cpu.fault.take().is_none();
    let msg = format!("[unlock] all missions unlocked: {}", ok);
    emu.log(&msg);
    ok
}

/// Grant the highest rank defined by v1.0.5's Local Wi-Fi XP table.
/// Rank is derived from XP, not stored separately. The Wi-Fi XP member is
/// GameSettings+0x280, serialized by SaveSettings into SaveGame.bin.
pub fn unlock_multiplayer_weapons(emu: &mut Emu) -> bool {
    let (get, set, rank, save) = match (
        emu.lib.find_func("_ZN12GameSettings11GetInstanceEv"),
        emu.lib.find_func("_ZN12GameSettings9SetXPWifiEi"),
        emu.lib.find_func("_ZNK12GameSettings11GetRankWifiEv"),
        emu.lib.find_func("_ZN12GameSettings12SaveSettingsEv"),
    ) {
        (Some(g), Some(s), Some(r), Some(w)) => (g, s, r, w),
        _ => return false,
    };
    let gs = emu.call_nested(get | 1, &[]).0;
    if !emu.hle.heap.contains(gs) { return false; }
    // xpForRankWifi has 72 entries: index 71 is the rank-72 threshold.
    let table = match emu.lib.exports.get("_ZN7RanksMP13xpForRankWifiE") {
        Some(p) => *p,
        None => return false,
    };
    let xp = emu.mem.r32(table + 71 * 4);
    if xp != 600_000 { return false; }
    emu.call_nested(set | 1, &[gs, xp]);
    if emu.cpu.fault.take().is_some() { return false; }
    let actual_rank = emu.call_nested(rank | 1, &[gs]).0;
    if emu.cpu.fault.take().is_some() || actual_rank != 72 { return false; }
    let saved = emu.call_nested(save | 1, &[gs]).0 != 0;
    let ok = emu.cpu.fault.take().is_none() && saved;
    emu.log(&format!("[unlock] Local Wi-Fi rank {} ({} XP), saved: {}", actual_rank, xp, ok));
    ok
}

/// Set the game's "AIM ASSIST" option (GameSettings+0xc, the flag
/// GameSettings::ToggleAutoAim flips and Hud::UpdateAutoAim checks).
pub fn set_aim_assist(emu: &mut Emu, on: bool) -> bool {
    let get = match emu.lib.find_func("_ZN12GameSettings11GetInstanceEv") {
        Some(g) => g,
        None => return false,
    };
    let gs = emu.call_nested(get | 1, &[]).0;
    if !emu.hle.heap.contains(gs) {
        return false;
    }
    emu.mem.w8(gs + 0xc, on as u8);
    true
}

/// Is the player aiming down the sights (Character::IsInIronSight)?
pub fn in_iron_sight(emu: &mut Emu) -> bool {
    let (c, f) = match (player(emu), emu.lib.find_func("_ZNK9Character13IsInIronSightEv")) {
        (Some(c), Some(f)) => (c, f),
        _ => return false,
    };
    let r = emu.call_nested(f | 1, &[c]).0 & 0xff != 0;
    emu.cpu.fault = None;
    r
}
