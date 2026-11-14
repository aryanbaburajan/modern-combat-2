//! Native headless harness: boots the game with a stub GL and real data files.
//!
//! usage: mc2cli <libsandstorm2.so> <data-root> [frames]
//! data-root is the directory containing `GloftBPHP/`.

use mc2engine::platform::{JValue, Platform};
use mc2engine::mem::Memory;
use mc2engine::{hle, Emu};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

struct Native {
    root: String,
    start: Instant,
    saves: HashMap<String, Vec<u8>>,
    files: HashMap<String, fs::File>,
    next_id: u32,
    gl_log: bool,
    gl_counts: HashMap<u32, u64>,
    shader_src: HashMap<u32, String>,
    prog_shaders: HashMap<u32, Vec<u32>>,
    prog_info: HashMap<u32, (Vec<(String, u32, u32)>, Vec<(String, u32, u32)>)>,
    log_java: bool,
    log_tex: bool,
    bound_buf: HashMap<u32, u32>,
    buf_size: HashMap<u32, u32>,
}

impl Platform for Native {
    fn log(&mut self, msg: &str) {
        println!("{:8.3} {}", self.start.elapsed().as_secs_f64(), msg);
    }
    fn now_ms(&mut self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
    fn wall_time(&mut self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
    }
    fn file_size(&mut self, path: &str) -> Option<u64> {
        fs::metadata(format!("{}/{}", self.root, path)).ok().filter(|m| m.is_file()).map(|m| m.len())
    }
    fn file_read(&mut self, path: &str, offset: u64, buf: &mut [u8]) -> usize {
        let full = format!("{}/{}", self.root, path);
        let f = match self.files.get_mut(&full) {
            Some(f) => f,
            None => match fs::File::open(&full) {
                Ok(f) => {
                    self.files.insert(full.clone(), f);
                    self.files.get_mut(&full).unwrap()
                }
                Err(_) => return 0,
            },
        };
        if f.seek(SeekFrom::Start(offset)).is_err() {
            return 0;
        }
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(_) => break,
            }
        }
        n
    }
    fn save_load(&mut self, path: &str) -> Option<Vec<u8>> {
        self.saves.get(path).cloned()
    }
    fn save_store(&mut self, path: &str, data: &[u8]) {
        println!("[save] store {} ({} bytes)", path, data.len());
        if let Ok(dir) = std::env::var("SAVE_DUMP") {
            let _ = fs::write(format!("{}/{}", dir, path.replace('/', "_")), data);
        }
        self.saves.insert(path.to_string(), data.to_vec());
    }
    fn save_delete(&mut self, path: &str) {
        self.saves.remove(path);
    }
    fn gl(&mut self, mem: &mut Memory, func: u32, a: &[u32]) -> u32 {
        if func == 0xffff_0001 {
            // size of the buffer bound to target a[0] (for glMapBufferOES)
            let b = self.bound_buf.get(&a[0]).copied().unwrap_or(0);
            return self.buf_size.get(&b).copied().unwrap_or(0);
        }
        let name = hle::gl::GL_FUNCS.get(func as usize).copied().unwrap_or("?");
        *self.gl_counts.entry(func).or_insert(0) += 1;
        if self.gl_log {
            println!("[gl] {}({:x?})", name, &a[..4]);
        }
        let mut gen = |n: u32, p: u32, s: &mut Self| {
            for i in 0..n {
                mem.w32(p + 4 * i, s.next_id);
                s.next_id += 1;
            }
        };
        if name.starts_with("glCompressedTex") || name == "glTexImage2D" || name == "glTexSubImage2D" {
            if self.log_tex {
                println!("[tex] {} target={:x} level={} fmt={:x} {}x{} a4={:x} a5={:x} a6={:x} a7={:x}", name, a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8]);
            }
        }
        match name {
            "glGenTextures" | "glGenBuffers" | "glGenFramebuffers" | "glGenRenderbuffers" | "glGenFramebuffersOES" | "glGenRenderbuffersOES" => {
                gen(a[0], a[1], self);
                0
            }
            "glCreateShader" | "glCreateProgram" => {
                self.next_id += 1;
                self.next_id
            }
            "glShaderSource" => {
                let (sh, count, strs, lens) = (a[0], a[1], a[2], a[3]);
                let mut src = String::new();
                for i in 0..count {
                    let p = mem.r32(strs + 4 * i);
                    if lens != 0 && mem.r32(lens + 4 * i) as i32 >= 0 {
                        let l = mem.r32(lens + 4 * i) as usize;
                        src += &String::from_utf8_lossy(mem.slice(p, l));
                    } else {
                        src += &mem.cstr(p);
                    }
                }
                self.shader_src.insert(sh, src);
                0
            }
            "glGetShaderiv" => {
                // COMPILE_STATUS = 1, INFO_LOG_LENGTH = 0
                mem.w32(a[2], if a[1] == 0x8B81 { 1 } else { 0 });
                0
            }
            "glAttachShader" => {
                self.prog_shaders.entry(a[0]).or_default().push(a[1]);
                0
            }
            "glLinkProgram" => {
                // Like a real driver: report the attributes/uniforms the shaders declare.
                let srcs: Vec<String> = self.prog_shaders.get(&a[0]).map(|v| v.iter().filter_map(|sh| self.shader_src.get(sh).cloned()).collect()).unwrap_or_default();
                self.prog_info.insert(a[0], glsl_interface(&srcs));
                0
            }
            "glGetProgramiv" => {
                let (attrs, unis) = self.prog_info.get(&a[0]).cloned().unwrap_or_default();
                let maxlen = |v: &Vec<(String, u32, u32)>| v.iter().map(|x| x.0.len() as u32 + 1).max().unwrap_or(0);
                let v = match a[1] {
                    0x8B82 => 1,                    // LINK_STATUS
                    0x8B89 => attrs.len() as u32,   // ACTIVE_ATTRIBUTES
                    0x8B8A => maxlen(&attrs),       // ACTIVE_ATTRIBUTE_MAX_LENGTH
                    0x8B86 => unis.len() as u32,    // ACTIVE_UNIFORMS
                    0x8B87 => maxlen(&unis),        // ACTIVE_UNIFORM_MAX_LENGTH
                    _ => 0,
                };
                mem.w32(a[2], v);
                0
            }
            "glGetActiveAttrib" | "glGetActiveUniform" => {
                // (program, index, bufsize, length*, size*, type*, name*)
                let (attrs, unis) = self.prog_info.get(&a[0]).cloned().unwrap_or_default();
                let list = if name == "glGetActiveAttrib" { attrs } else { unis };
                if let Some((n, size, ty)) = list.get(a[1] as usize) {
                    let mut nm = n.clone();
                    if *size > 1 {
                        nm += "[0]";
                    }
                    let cap = a[2] as usize;
                    let bytes = &nm.as_bytes()[..nm.len().min(cap.saturating_sub(1))];
                    mem.write_cstr(a[6], bytes);
                    if a[3] != 0 {
                        mem.w32(a[3], bytes.len() as u32);
                    }
                    mem.w32(a[4], *size);
                    mem.w32(a[5], *ty);
                }
                0
            }
            "glGetAttribLocation" | "glGetUniformLocation" => {
                let (attrs, unis) = self.prog_info.get(&a[0]).cloned().unwrap_or_default();
                let want = mem.cstr(a[1]);
                let want = want.trim_end_matches("[0]");
                if name == "glGetAttribLocation" {
                    attrs.iter().position(|x| x.0 == want).map(|i| i as u32).unwrap_or(u32::MAX)
                } else {
                    unis.iter().position(|x| x.0 == want).map(|i| i as u32 + 1).unwrap_or(u32::MAX)
                }
            }
            "glGetIntegerv" => {
                let v = match a[0] {
                    // typical desktop WebGL2 limits
                    0x0D33 => 8192,  // MAX_TEXTURE_SIZE
                    0x8869 => 16,    // MAX_VERTEX_ATTRIBS
                    0x8872 => 16,    // MAX_TEXTURE_IMAGE_UNITS
                    0x8B4C => 16,    // MAX_VERTEX_TEXTURE_IMAGE_UNITS
                    0x8B4D => 32,    // MAX_COMBINED_TEXTURE_IMAGE_UNITS
                    0x8DFB => 4096,  // MAX_VERTEX_UNIFORM_VECTORS
                    0x8DFC => 30,    // MAX_VARYING_VECTORS
                    0x8DFD => 1024,  // MAX_FRAGMENT_UNIFORM_VECTORS
                    0x84E8 => 8192,  // MAX_RENDERBUFFER_SIZE
                    0x86A2 => 0,     // NUM_COMPRESSED_TEXTURE_FORMATS
                    _ => 0,
                };
                mem.w32(a[1], v);
                0
            }
            "glBindBuffer" => {
                self.bound_buf.insert(a[0], a[1]);
                0
            }
            "glBufferData" => {
                let b = self.bound_buf.get(&a[0]).copied().unwrap_or(0);
                self.buf_size.insert(b, a[1]);
                0
            }
            "glGetError" => 0,
            "glCheckFramebufferStatus" | "glCheckFramebufferStatusOES" => 0x8CD5,
            _ => 0,
        }
    }
    fn java_call(&mut self, class: &str, name: &str, sig: &str, args: &[JValue]) -> JValue {
        if self.log_java {
            println!("[java] {}.{}{} {:?}", class, name, sig, args);
        }
        match name {
            "GetPhoneLanguage" => JValue::Int(0),
            "getManufacture" => JValue::Int(0),
            "isWifiEnabled" => JValue::Int(0),
            "GetNumPlaylists" => JValue::Int(0),
            "isSoundLoaded" | "isMediaPlaying" | "isMusicPlaying" => JValue::Int(0),
            "GetPlayListName" => JValue::Bytes(Vec::new()),
            _ => JValue::Void,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: mc2cli <libsandstorm2.so> <data-root> [frames]");
        std::process::exit(2);
    }
    let frames: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let lib = fs::read(&args[1]).expect("read lib");
    let plat = Native {
        root: args[2].clone(),
        start: Instant::now(),
        saves: HashMap::new(),
        files: HashMap::new(),
        next_id: 1,
        gl_log: std::env::var("GL_LOG").is_ok(),
        gl_counts: HashMap::new(),
        shader_src: HashMap::new(),
        prog_shaders: HashMap::new(),
        prog_info: HashMap::new(),
        log_java: std::env::var("JAVA_LOG").is_ok(),
        log_tex: std::env::var("TEX_LOG").is_ok(),
        bound_buf: HashMap::new(),
        buf_size: HashMap::new(),
    };
    let mut emu = Emu::new(&lib, Box::new(plat)).expect("load");
    emu.hle.vfs.log_opens = std::env::var("VFS_LOG").is_ok();
    emu.hle.jni.trace = std::env::var("JNI_LOG").is_ok();
    emu.trace = std::env::var("TRACE").is_ok();
    emu.early.enabled = std::env::var("EARLY_MOVE").is_ok();
    if let Ok(list) = std::env::var("TRACE_FUNCS") {
        for name in list.split(',') {
            match emu.lib.find_func(name) {
                Some(a) => emu.traced.push((a, name.to_string())),
                None => eprintln!("TRACE_FUNCS: unknown symbol {}", name),
            }
        }
    }
    if let Ok(e) = std::env::var("GL_EXTS") {
        emu.hle.gl.extensions = e;
    }
    if let Ok(r) = std::env::var("TEXT_RULE") {
        if let Some((a, b)) = r.split_once('|') {
            mc2engine::game::set_text_rules(&mut emu, vec![(a.to_string(), b.to_string())]);
        }
    }
    let t0 = Instant::now();
    let check = |r: Result<u32, String>, what: &str| -> u32 {
        match r {
            Ok(v) => v,
            Err(e) => {
                eprintln!("FAULT during {}: {}", what, e);
                std::process::exit(1);
            }
        }
    };
    check(emu.run_init_array().map(|_| 0), "init_array");
    println!("init_array done: {} instructions", emu.icount);

    let env = emu.hle.jni.env;
    let cls = emu.hle.jni.new_obj(hle::jni::JObj::Class("Game".into()));
    let s = |emu: &mut Emu, v: &str| emu.hle.jni.new_string(v);
    let (w, h) = (800u32, 480u32);
    let pfx = "Java_com_gameloft_android_GAND_GloftBPHP_ML_";
    let a = [s(&mut emu, "US"), s(&mut emu, "GL_00"), s(&mut emu, "samsung_GT-I9000"), s(&mut emu, "2.3.3")];
    check(emu.call_export(&format!("{}Game_nativeGetInfo", pfx), &[env, cls, a[0], a[1], a[2], a[3]]), "nativeGetInfo");
    let ver = s(&mut emu, "1.0.5");
    check(emu.call_export(&format!("{}GameRenderer_nativeInit", pfx), &[env, cls, 2, w, h, ver]), "GameRenderer.nativeInit");
    check(emu.call_export(&format!("{}GLMediaPlayer_nativeInit", pfx), &[env, cls, 0]), "GLMediaPlayer.nativeInit");
    let ns = check(emu.call_export(&format!("{}GLMediaPlayer_nativeGetTotalSounds", pfx), &[env, cls]), "sounds");
    let nm = check(emu.call_export(&format!("{}GLMediaPlayer_nativeGetTotalMusics", pfx), &[env, cls]), "musics");
    println!("sounds={} musics={}", ns, nm);
    check(emu.call_export(&format!("{}Game_nativeInit", pfx), &[env, cls]), "Game.nativeInit");
    check(emu.call_export(&format!("{}GameRenderer_nativeResize", pfx), &[env, cls, w, h]), "nativeResize");
    println!("init done: {} instructions in {:.2}s", emu.icount, t0.elapsed().as_secs_f64());
    if std::env::var("AIM_OFF").is_ok() {
        println!("aim assist off: {}", mc2engine::game::set_aim_assist(&mut emu, false));
    }
    if std::env::var("UNLOCK_ALL").is_ok() {
        mc2engine::game::unlock_all_missions(&mut emu);
    }
    let render = *emu.lib.exports.get(&format!("{}GameRenderer_nativeRender", pfx)).unwrap();
    if let Ok(path) = std::env::var("SCRIPT") {
        run_script(&mut emu, &fs::read_to_string(&path).expect("read script"), render, env, cls);
        if let Ok(dir) = std::env::var("SERVE") {
            serve(&mut emu, &dir, render, env, cls);
        }
        return;
    }
    for f in 0..frames {
        let i0 = emu.icount;
        let ft = Instant::now();
        check(emu.call(render, &[env, cls]), "nativeRender");
        if let Err(e) = hle::thread::run_background(&mut emu, 2_000_000) {
            eprintln!("FAULT in background: {}", e);
            std::process::exit(1);
        }
        if f % 10 == 0 || std::env::var("FRAME_LOG").is_ok() {
            println!(
                "frame {} : {} insns, {:.1} ms, gl calls {}, heap {} MB, read {} MB",
                f,
                emu.icount - i0,
                ft.elapsed().as_secs_f64() * 1000.0,
                emu.hle.gl.calls,
                emu.hle.heap.live_bytes >> 20,
                emu.hle.vfs.bytes_read >> 20
            );
        }
    }
    if let Ok(path) = std::env::var("DUMP_STRINGS") {
        let mut out = String::new();
        for (t, base, offs, count) in mc2engine::game::string_tables(&mut emu) {
            for i in 0..count {
                let a = base.wrapping_add(emu.mem.r32(offs + 4 * i));
                let s = String::from_utf8_lossy(emu.mem.cstr_bytes(a)).replace('\n', "\\n");
                out += &format!("{}:{}\t{}\n", t, i, s);
            }
        }
        fs::write(&path, out).expect("write strings");
        println!("dumped strings to {} (text hook hits {}, hook pcs {:x?}, saved {})", path, emu.text.hook_hits, emu.text.hook_pcs, 0);
    }
    let secs = t0.elapsed().as_secs_f64();
    println!("total {} instructions in {:.2}s ({:.1} MIPS)", emu.icount, secs, emu.icount as f64 / secs / 1e6);
    let mut un: Vec<_> = emu.hle.unimpl_seen.iter().collect();
    println!("GL usage:");
    if let Some(p) = std::env::var_os("GL_STATS") {
        let _ = p;
    }
    un.sort();
    println!("unimplemented imports called: {:?}", un);
    drop(emu);
}

impl Drop for Native {
    fn drop(&mut self) {
        let mut v: Vec<_> = self.gl_counts.iter().map(|(k, c)| (hle::gl::GL_FUNCS.get(*k as usize).copied().unwrap_or("?"), *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        println!("GL call counts: {:?}", v);
    }
}

/// Scripted run for testing input natively. Each line: `<seconds> <command> [args]`
/// (time since boot finished). Commands:
///   tap X Y [ID] | down X Y ID | move X Y ID | up X Y ID
///   press PAT,PAT...   locate a HUD control (like the browser key bindings) and tap it
///   locate PAT,PAT...  print where a control would be tapped
///   hud                print visible HUD controls, roots and game state
///   quit
fn run_script(emu: &mut Emu, script: &str, render: u32, env: u32, cls: u32) {
    let touch_fn = *emu.lib.exports.get("Java_com_gameloft_android_GAND_GloftBPHP_ML_GameGLSurfaceView_nativeOnTouch").unwrap();
    let mut cmds: Vec<(f64, Vec<String>)> = script
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| {
            let mut it = l.split_whitespace();
            let t: f64 = it.next().unwrap().parse().unwrap();
            (t, it.map(|s| s.to_string()).collect())
        })
        .collect();
    cmds.reverse();
    let t0 = Instant::now();
    let mut pending_up: Vec<(f64, u32, u32, u32)> = Vec::new();
    let mut frames = 0u64;
    let touch = |emu: &mut Emu, a: u32, x: u32, y: u32, id: u32| {
        if let Err(e) = emu.call(touch_fn, &[env, cls, a, x, y, id]) {
            eprintln!("FAULT in touch: {}", e);
            std::process::exit(1);
        }
    };
    loop {
        let now = t0.elapsed().as_secs_f64();
        let due: Vec<(f64, u32, u32, u32)> = pending_up.iter().copied().filter(|p| now >= p.0).collect();
        pending_up.retain(|p| now < p.0);
        for (_, x, y, id) in due {
            touch(emu, 0, x, y, id);
        }
        while let Some((t, c)) = cmds.last() {
            if now < *t {
                break;
            }
            let c = c.clone();
            cmds.pop();
            let arg = |i: usize| c.get(i).map(|s| s.parse::<f32>().unwrap_or(0.0)).unwrap_or(0.0);
            println!("[script {:6.2}s f{}] {}", now, frames, c.join(" "));
            match c[0].as_str() {
                "tap" => {
                    let id = c.get(3).map(|s| s.parse().unwrap()).unwrap_or(1);
                    touch(emu, 1, arg(1) as u32, arg(2) as u32, id);
                    pending_up.push((now + 0.1, arg(1) as u32, arg(2) as u32, id));
                }
                "down" => touch(emu, 1, arg(1) as u32, arg(2) as u32, arg(3) as u32),
                "move" => touch(emu, 2, arg(1) as u32, arg(2) as u32, arg(3) as u32),
                "up" => touch(emu, 0, arg(1) as u32, arg(2) as u32, arg(3) as u32),
                "locate" | "press" => {
                    let pats: Vec<&str> = c[1].split(',').collect();
                    match mc2engine::game::hud_locate(emu, &pats, 800, 480) {
                        Some((n, x, y)) => {
                            println!("  -> {} at {},{}", n, x, y);
                            if c[0] == "press" {
                                touch(emu, 1, x as u32, y as u32, 1);
                                pending_up.push((now + 0.1, x as u32, y as u32, 1));
                            }
                        }
                        None => println!("  -> not found; visible: {}", mc2engine::game::hud_visible_names(emu).join(", ")),
                    }
                }
                "hud" => {
                    let roots = mc2engine::game::hud_roots(emu);
                    println!("  roots {:x?} cutscene={} paused={}", roots, mc2engine::game::in_cutscene(emu), mc2engine::game::is_paused(emu));
                    println!("  visible: {}", mc2engine::game::hud_visible_names(emu).join(", "));
                    println!("  Character::Crouch hits={} Stand hits={} early-move level={:?} overrides={}", emu.probes[mc2engine::game::PROBE_CROUCH].hits, emu.probes[mc2engine::game::PROBE_STAND].hits, emu.early.level, emu.early.overrides);
                    for r in mc2engine::game::hud_scan(emu, 800, 480, 16) {
                        println!("    {:<24} ({:.0},{:.0})-({:.0},{:.0}) c=({:.0},{:.0}) a={}", r.name, r.x0, r.y0, r.x1, r.y1, r.cx, r.cy, r.alpha);
                    }
                }
                "where" => {
                    let p = mc2engine::game::player_pos(emu);
                    let o = mc2engine::game::objective_pos(emu);
                    match (p, o) {
                        (Some(p), Some(o)) => {
                            let d = ((p[0] - o[0]).powi(2) + (p[1] - o[1]).powi(2) + (p[2] - o[2]).powi(2)).sqrt();
                            println!("  player ({:.1},{:.1},{:.1}) objective ({:.1},{:.1},{:.1}) dist {:.1}", p[0], p[1], p[2], o[0], o[1], o[2], d);
                        }
                        _ => println!("  no player"),
                    }
                }
                "teleport" => {
                    let ok = mc2engine::game::set_player_pos(emu, [arg(1), arg(2), arg(3)]);
                    println!("  teleport {}", ok);
                }
                "goto_objective" => {
                    if let Some(o) = mc2engine::game::objective_pos(emu) {
                        let ok = mc2engine::game::set_player_pos(emu, o);
                        println!("  teleport to objective ({:.1},{:.1},{:.1}) {}", o[0], o[1], o[2], ok);
                    }
                }
                "dumphud" => {
                    for (d, n, act, vis, tch, a, x, y) in mc2engine::game::hud_dump(emu) {
                        println!("    {}{} act={} vis={} touch={} a={} pos=({:.0},{:.0})", "  ".repeat(d as usize), n, act, vis, tch, a, x, y);
                    }
                }
                "elements" => {
                    for (i, n, st, act, vis) in mc2engine::game::hud_elements(emu) {
                        println!("    element {:2} {:<28} state={} act={} vis={}", i, n, st, act, vis);
                    }
                }
                "zone" => println!("  {:?}", mc2engine::game::hud_zone(emu, &c[1], 800, 480)),
                "quit" => return,
                other => println!("  unknown command {}", other),
            }
        }
        if let Err(e) = emu.call(render, &[env, cls]) {
            eprintln!("FAULT in render: {}", e);
            std::process::exit(1);
        }
        if let Err(e) = hle::thread::run_background(emu, 2_000_000) {
            eprintln!("FAULT in background: {}", e);
            std::process::exit(1);
        }
        mc2engine::game::tick_early_move(emu);
        frames += 1;
        // Pace like a 60 Hz display; the game's timing code expects real frame times.
        let target = t0 + std::time::Duration::from_secs_f64(frames as f64 / 60.0);
        let now_i = Instant::now();
        if target > now_i {
            std::thread::sleep(target - now_i);
        }
        if cmds.is_empty() && pending_up.is_empty() {
            return;
        }
    }
}

/// Attributes and uniforms declared by GLSL sources: (name, array size, GL type).
fn glsl_interface(srcs: &[String]) -> (Vec<(String, u32, u32)>, Vec<(String, u32, u32)>) {
    let ty = |t: &str| -> u32 {
        match t {
            "float" => 0x1406,
            "vec2" => 0x8B50,
            "vec3" => 0x8B51,
            "vec4" => 0x8B52,
            "int" => 0x1404,
            "ivec2" => 0x8B53,
            "ivec3" => 0x8B54,
            "ivec4" => 0x8B55,
            "bool" => 0x8B56,
            "mat2" => 0x8B5A,
            "mat3" => 0x8B5B,
            "mat4" => 0x8B5C,
            "sampler2D" => 0x8B5E,
            "samplerCube" => 0x8B60,
            _ => 0x8B52,
        }
    };
    let mut attrs = Vec::new();
    let mut unis: Vec<(String, u32, u32)> = Vec::new();
    for src in srcs {
        for stmt in src.split(';') {
            let words: Vec<&str> = stmt.split_whitespace().filter(|w| !matches!(*w, "lowp" | "mediump" | "highp" | "const")).collect();
            if words.len() < 3 || (words[0] != "attribute" && words[0] != "uniform") {
                continue;
            }
            let t = ty(words[1]);
            for decl in words[2..].join(" ").split(',') {
                let decl = decl.trim();
                let (name, size) = match decl.split_once('[') {
                    Some((n, rest)) => (n.trim().to_string(), rest.trim_end_matches(']').trim().parse().unwrap_or(1)),
                    None => (decl.to_string(), 1),
                };
                if name.is_empty() {
                    continue;
                }
                if words[0] == "attribute" {
                    if !attrs.iter().any(|a: &(String, u32, u32)| a.0 == name) {
                        attrs.push((name, size, t));
                    }
                } else if !unis.iter().any(|u| u.0 == name) {
                    unis.push((name, size, t));
                }
            }
        }
    }
    (attrs, unis)
}

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

/// Fork server: the game state reached by the setup script is kept in this
/// process; every `*.txt` script dropped into `dir` runs in a forked copy of
/// it (output in `<script>.out`), so each test starts from the same point.
fn serve(emu: &mut Emu, dir: &str, render: u32, env: u32, cls: u32) {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    fs::create_dir_all(dir).ok();
    println!("[serve] ready; drop scripts into {}", dir);
    std::io::stdout().flush().ok();
    loop {
        let mut reqs: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "txt")).collect();
        reqs.sort();
        let req = match reqs.into_iter().next() {
            Some(r) => r,
            None => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };
        let script = fs::read_to_string(&req).unwrap_or_default();
        let running = req.with_extension("running");
        fs::rename(&req, &running).ok();
        if script.trim() == "exit" {
            return;
        }
        std::io::stdout().flush().ok();
        let pid = unsafe { fork() };
        if pid == 0 {
            let out = fs::File::create(req.with_extension("out")).unwrap();
            unsafe {
                dup2(out.as_raw_fd(), 1);
                dup2(out.as_raw_fd(), 2);
            }
            run_script(emu, &script, render, env, cls);
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        let mut status = 0;
        unsafe { waitpid(pid, &mut status, 0) };
        fs::rename(&running, req.with_extension("done")).ok();
    }
}
