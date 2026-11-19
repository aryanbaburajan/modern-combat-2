//! WebAssembly entry points and the browser-backed Platform.

use crate::hle;
use crate::mem::Memory;
use crate::platform::{JValue, Platform};
use crate::Emu;

#[link(wasm_import_module = "env")]
extern "C" {
    fn js_net(op: u32, args: *const u32, base: *const u8) -> i32;
    fn js_log(ptr: *const u8, len: usize);
    fn js_now() -> f64;
    fn js_wall() -> f64;
    fn js_file_size(ptr: *const u8, len: usize) -> f64;
    fn js_file_read(ptr: *const u8, len: usize, off: f64, buf: *mut u8, buf_len: usize) -> usize;
    fn js_save_size(ptr: *const u8, len: usize) -> i32;
    fn js_save_read(ptr: *const u8, len: usize, dst: *mut u8);
    fn js_save_store(ptr: *const u8, len: usize, data: *const u8, data_len: usize);
    fn js_save_delete(ptr: *const u8, len: usize);
    fn js_gl(func: u32, args: *const u32) -> u32;
    fn js_java(req: *const u8, req_len: usize, out: *mut u8, out_cap: usize) -> i32;
}

struct Web;

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Platform for Web {
    fn net(&mut self, op: u32, args: &[u32; 6], memory: &mut Memory) -> i32 {
        unsafe { js_net(op, args.as_ptr(), memory.host_ptr()) }
    }
    fn log(&mut self, msg: &str) {
        unsafe { js_log(msg.as_ptr(), msg.len()) }
    }
    fn now_ms(&mut self) -> f64 {
        unsafe { js_now() }
    }
    fn wall_time(&mut self) -> f64 {
        unsafe { js_wall() }
    }
    fn file_size(&mut self, path: &str) -> Option<u64> {
        let r = unsafe { js_file_size(path.as_ptr(), path.len()) };
        if r < 0.0 {
            None
        } else {
            Some(r as u64)
        }
    }
    fn file_read(&mut self, path: &str, offset: u64, buf: &mut [u8]) -> usize {
        unsafe { js_file_read(path.as_ptr(), path.len(), offset as f64, buf.as_mut_ptr(), buf.len()) }
    }
    fn save_load(&mut self, path: &str) -> Option<Vec<u8>> {
        let n = unsafe { js_save_size(path.as_ptr(), path.len()) };
        if n < 0 {
            return None;
        }
        let mut v = vec![0u8; n as usize];
        unsafe { js_save_read(path.as_ptr(), path.len(), v.as_mut_ptr()) };
        Some(v)
    }
    fn save_store(&mut self, path: &str, data: &[u8]) {
        unsafe { js_save_store(path.as_ptr(), path.len(), data.as_ptr(), data.len()) }
    }
    fn save_delete(&mut self, path: &str) {
        unsafe { js_save_delete(path.as_ptr(), path.len()) }
    }
    fn gl(&mut self, _mem: &mut Memory, func: u32, args: &[u32]) -> u32 {
        let mut a = [0u32; 12];
        let n = args.len().min(12);
        a[..n].copy_from_slice(&args[..n]);
        unsafe { js_gl(func, a.as_ptr()) }
    }
    fn java_call(&mut self, class: &str, name: &str, sig: &str, args: &[JValue]) -> JValue {
        let mut req = format!("{{\"c\":{},\"m\":{},\"s\":{},\"a\":[", json_str(class), json_str(name), json_str(sig));
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                req.push(',');
            }
            match a {
                JValue::Int(v) => req += &v.to_string(),
                JValue::Long(v) => req += &v.to_string(),
                JValue::Float(v) => req += &format!("{}", if v.is_finite() { *v } else { 0.0 }),
                JValue::Double(v) => req += &format!("{}", if v.is_finite() { *v } else { 0.0 }),
                JValue::Str(Some(s)) => req += &json_str(s),
                _ => req += "null",
            }
        }
        req += "]}";
        let mut out = vec![0u8; 4096];
        let n = unsafe { js_java(req.as_ptr(), req.len(), out.as_mut_ptr(), out.len()) };
        if n <= 0 {
            return JValue::Void;
        }
        let resp = String::from_utf8_lossy(&out[..n as usize]).into_owned();
        let (tag, val) = resp.split_at(1);
        let val = val.strip_prefix(':').unwrap_or(val);
        match tag {
            "i" => JValue::Int(val.parse().unwrap_or(0)),
            "j" => JValue::Long(val.parse().unwrap_or(0)),
            "f" => JValue::Float(val.parse().unwrap_or(0.0)),
            "d" => JValue::Double(val.parse().unwrap_or(0.0)),
            "s" => JValue::Str(Some(val.to_string())),
            _ => JValue::Void,
        }
    }
}

static mut EMU: Option<Emu> = None;
static mut LAST_ERROR: String = String::new();

#[allow(static_mut_refs)]
fn emu() -> &'static mut Emu {
    unsafe { EMU.as_mut().expect("emulator not initialised") }
}

#[allow(static_mut_refs)]
fn fail(e: String) -> i32 {
    unsafe {
        js_log(e.as_ptr(), e.len());
        LAST_ERROR = e;
    }
    -1
}

#[no_mangle]
pub extern "C" fn mc2_alloc(size: usize) -> *mut u8 {
    let mut v = vec![0u8; size];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

#[no_mangle]
pub unsafe extern "C" fn mc2_free(p: *mut u8, size: usize) {
    drop(Vec::from_raw_parts(p, size, size));
}

#[no_mangle]
#[allow(static_mut_refs)]
pub extern "C" fn mc2_last_error(out: *mut u8, cap: usize) -> usize {
    unsafe {
        let n = LAST_ERROR.len().min(cap);
        std::ptr::copy_nonoverlapping(LAST_ERROR.as_ptr(), out, n);
        n
    }
}

/// Load the game library. Returns 0 on success.
#[no_mangle]
pub unsafe extern "C" fn mc2_init(lib: *const u8, len: usize) -> i32 {
    std::panic::set_hook(Box::new(|info| {
        let s = format!("panic: {}", info);
        js_log(s.as_ptr(), s.len());
    }));
    let bytes = std::slice::from_raw_parts(lib, len);
    match Emu::new(bytes, Box::new(Web)) {
        Ok(e) => {
            EMU = Some(e);
            0
        }
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_guest_base() -> *const u8 {
    emu().mem.host_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn mc2_set_extensions(p: *const u8, len: usize) {
    let s = String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned();
    emu().hle.gl.extensions = s;
}

#[no_mangle]
pub extern "C" fn mc2_set_flags(flags: u32) {
    let e = emu();
    e.hle.vfs.log_opens = flags & 1 != 0;
    e.hle.jni.trace = flags & 2 != 0;
    e.trace = flags & 4 != 0;
}

const PFX: &str = "Java_com_gameloft_android_GAND_GloftBPHP_ML_";

static mut CLASSES: Vec<(String, u32)> = Vec::new();

#[allow(static_mut_refs)]
fn class_obj(e: &mut Emu, name: &str) -> u32 {
    unsafe {
        if let Some((_, h)) = CLASSES.iter().find(|(n, _)| n == name) {
            return *h;
        }
        let h = e.hle.jni.new_obj(hle::jni::JObj::Class(format!("com/gameloft/android/GAND/GloftBPHP/ML/{}", name)));
        CLASSES.push((name.to_string(), h));
        h
    }
}

fn call_native(class: &str, method: &str, extra: &[u32]) -> Result<u32, String> {
    let e = emu();
    let env = e.hle.jni.env;
    let cls = class_obj(e, class);
    let mut args = vec![env, cls];
    args.extend_from_slice(extra);
    e.call_export(&format!("{}{}_{}", PFX, class, method), &args)
}

/// Run constructors and the Java-side startup sequence.
#[no_mangle]
pub extern "C" fn mc2_boot(w: u32, h: u32, lang: u32) -> i32 {
    let _ = lang;
    let r = (|| -> Result<(), String> {
        emu().run_init_array()?;
        let e = emu();
        let s: Vec<u32> = ["US", "GL_00", "samsung_GT-I9000", "2.3.3"].iter().map(|v| e.hle.jni.new_string(v)).collect();
        call_native("Game", "nativeGetInfo", &s)?;
        let ver = emu().hle.jni.new_string("1.0.5");
        call_native("GameRenderer", "nativeInit", &[2, w, h, ver])?;
        call_native("GLMediaPlayer", "nativeInit", &[0])?;
        call_native("GLMediaPlayer", "nativeGetTotalSounds", &[])?;
        call_native("GLMediaPlayer", "nativeGetTotalMusics", &[])?;
        call_native("Game", "nativeInit", &[])?;
        call_native("GameRenderer", "nativeResize", &[w, h])?;
        Ok(())
    })();
    match r {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// Render one frame, then give background threads some time.
#[no_mangle]
pub extern "C" fn mc2_frame(bg_budget: u32) -> i32 {
    let r = call_native("GameRenderer", "nativeRender", &[]).and_then(|_| hle::thread::run_background(emu(), bg_budget as u64));
    match r {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_keyboard_straight(enabled: u32) {
    emu().keyboard_straight = enabled != 0;
}

#[no_mangle]
pub extern "C" fn mc2_touch(action: u32, x: i32, y: i32, id: u32) -> i32 {
    match call_native("GameGLSurfaceView", "nativeOnTouch", &[action, x as u32, y as u32, id]) {
        Ok(_) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_key(down: u32, code: u32) -> i32 {
    let m = if down != 0 { "nativeOnKeyDown" } else { "nativeOnKeyUp" };
    match call_native("Game", m, &[code]) {
        Ok(_) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_resize(w: u32, h: u32) -> i32 {
    match call_native("GameRenderer", "nativeResize", &[w, h]) {
        Ok(_) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_pause(paused: u32) -> i32 {
    let m = if paused != 0 { "nativePause" } else { "nativeResume" };
    match call_native("GameGLSurfaceView", m, &[]) {
        Ok(_) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
pub extern "C" fn mc2_accel(x: f32, y: f32, z: f32) -> i32 {
    match call_native("Game", "nativeAccelerator", &[x.to_bits(), y.to_bits(), z.to_bits()]) {
        Ok(_) => 0,
        Err(e) => fail(e),
    }
}

/// Fill `out` with stats: [icount_lo, icount_hi, heap_live, gl_calls_lo, bytes_read_lo, threads].
#[no_mangle]
pub extern "C" fn mc2_stats(out: *mut u32) {
    let e = emu();
    let v = [e.icount as u32, (e.icount >> 32) as u32, e.hle.heap.live_bytes as u32, e.hle.gl.calls as u32, e.hle.vfs.bytes_read as u32, e.hle.threads.list.len() as u32];
    unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), out, v.len()) }
}

/// Names of the GL functions, newline separated, for the JS dispatcher.
#[no_mangle]
pub extern "C" fn mc2_gl_names(out: *mut u8, cap: usize) -> usize {
    let s = hle::gl::GL_FUNCS.join("\n");
    let n = s.len().min(cap);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), out, n) }
    s.len()
}

/// Scan the in-game HUD; writes JSON `[[name,x0,y0,x1,y1,cells],...]` to `out`.
#[no_mangle]
pub extern "C" fn mc2_hud_scan(w: u32, h: u32, step: u32, out: *mut u8, cap: usize) -> usize {
    let rects = crate::game::hud_scan(emu(), w, h, step);
    let mut s = String::from("[");
    for (i, r) in rects.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s += &format!("[{},{},{},{},{},{},{},{},{}]", json_str(&r.name), r.x0, r.y0, r.x1, r.y1, r.cells, r.cx, r.cy, r.alpha);
    }
    s.push(']');
    let n = s.len().min(cap);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), out, n) }
    n
}

/// Layout fingerprint of the in-game HUD (0 when no HUD is active).
#[no_mangle]
pub extern "C" fn mc2_hud_signature() -> f64 {
    (crate::game::hud_signature(emu()) >> 12) as f64
}

/// Game state bits for the input layer: bit 0 = cutscene playing,
/// bit 1 = paused, bit 2 = aiming down the sights, bit 3 = loadout selection, bit 4 = quick-time event.
#[no_mangle]
pub extern "C" fn mc2_game_state() -> u32 {
    let e = emu();
    crate::game::in_cutscene(e) as u32 | (crate::game::is_paused(e) as u32) << 1 | (crate::game::in_iron_sight(e) as u32) << 2 | (crate::game::in_loadout(e) as u32) << 3 | (crate::game::in_qte(e) as u32) << 4
}

/// Set the HUD controls that should not be drawn (newline-separated names).
#[no_mangle]
pub unsafe extern "C" fn mc2_set_hidden_hud(names: *const u8, len: usize) {
    let text = String::from_utf8_lossy(std::slice::from_raw_parts(names, len)).into_owned();
    emu().hidden_hud = text.split('\n').filter(|n| !n.is_empty()).map(|n| n.to_string()).collect();
}

/// Locate a HUD control right now. `patterns` is newline-separated (trailing
/// `*` = prefix). Writes "name\tx\ty" to `out` (empty if not found).
#[no_mangle]
pub unsafe extern "C" fn mc2_hud_locate(patterns: *const u8, len: usize, w: u32, h: u32, out: *mut u8, cap: usize) -> usize {
    let text = String::from_utf8_lossy(std::slice::from_raw_parts(patterns, len)).into_owned();
    let pats: Vec<&str> = text.split('\n').filter(|p| !p.is_empty()).collect();
    let s = match crate::game::hud_locate(emu(), &pats, w, h) {
        Some((name, x, y)) => format!("{}\t{}\t{}", name, x, y),
        None => {
            // Diagnostics: every drawn control (and everything under
            // contextual_buttons) with its flags, whether active or not.
            let mut lines = Vec::new();
            let mut ctx_depth: Option<u32> = None;
            for (d, n, act, vis, tch, a, x, y) in crate::game::hud_dump(emu()) {
                if let Some(cd) = ctx_depth {
                    if d <= cd {
                        ctx_depth = None;
                    }
                }
                if n == "contextual_buttons" {
                    ctx_depth = Some(d);
                }
                if vis != 0 || ctx_depth.is_some() {
                    lines.push(format!("{}{}[act{} vis{} touch{} a{} @{:.0},{:.0}]", "  ".repeat(d as usize), n, act, vis, tch, a, x, y));
                }
            }
            format!("!\t{}\n{}", crate::game::hud_visible_names(emu()).join(", "), lines.join("\n"))
        }
    };
    let n = s.len().min(cap);
    std::ptr::copy_nonoverlapping(s.as_ptr(), out, n);
    n
}

/// Set text substitution rules: "from\tto" pairs separated by newlines
/// (literal "\n" inside a rule stands for a line break).
#[no_mangle]
pub unsafe extern "C" fn mc2_set_text_rules(p: *const u8, len: usize) {
    let text = String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned();
    let rules = text
        .split('\n')
        .filter_map(|l| l.split_once('\t'))
        .map(|(a, b)| (a.replace("\\n", "\n"), b.replace("\\n", "\n")))
        .collect();
    crate::game::set_text_rules(emu(), rules);
}

/// Unlock all missions (call after boot).
#[no_mangle]
pub extern "C" fn mc2_unlock_all_missions() -> u32 {
    crate::game::unlock_all_missions(emu()) as u32
}

/// Grant and save the highest valid Local Wi-Fi rank.
#[no_mangle]
pub extern "C" fn mc2_unlock_multiplayer_weapons() -> u32 {
    crate::game::unlock_multiplayer_weapons(emu()) as u32
}

/// Set the game's aim-assist option.
#[no_mangle]
pub extern "C" fn mc2_set_aim_assist(on: u32) -> u32 {
    crate::game::set_aim_assist(emu(), on != 0) as u32
}
