//! Modern Combat 2 runtime: runs the original Android ARM game library on top
//! of an ARMv7 interpreter with high-level emulation of its system imports.

pub mod cpu;
pub mod game;
pub mod hle;
pub mod loader;
pub mod mem;
pub mod platform;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

use cpu::{BlockCache, Cpu};
use loader::LoadedLib;
use mem::*;
use platform::Platform;

pub enum Stop {
    /// The outermost guest call returned to the host.
    Return,
    /// Instruction budget exhausted.
    Budget,
    /// Current thread blocked (mutex, join, sleep) or exited.
    Blocked,
    Fault(String),
}

pub struct Emu {
    pub cpu: Cpu,
    pub mem: Memory,
    pub cache: BlockCache,
    pub lib: LoadedLib,
    pub hle: hle::State,
    pub plat: Box<dyn Platform>,
    pub icount: u64,
    pub trace: bool,
    /// Guest function entry points being watched: on entry we record r0 and
    /// count the call (used to observe game state, see game.rs).
    pub probes: Vec<Probe>,
    /// Entry of HUDControl::Render; calls on controls named in `hidden_hud`
    /// return immediately (the control is not drawn but still accepts touches).
    pub render_pc: u32,
    pub hidden_hud: Vec<String>,
    pub text: game::TextPatcher,
    /// Debug: function entries to log (pc, symbol).
    pub traced: Vec<(u32, String)>,
}

#[derive(Clone, Default)]
pub struct Probe {
    pub pc: u32,
    pub r0: u32,
    pub r1: u32,
    pub hits: u64,
    /// Instruction count at the most recent hit (orders calls between probes).
    pub last: u64,
    /// Distinct r0 values seen recently, with the instruction count of their
    /// last hit (e.g. every HUD engine instance that is being updated).
    pub seen: Vec<(u32, u64)>,
}

impl Emu {
    pub fn new(lib_bytes: &[u8], plat: Box<dyn Platform>) -> Result<Emu, String> {
        let mut mem = Memory::new();
        let mut hle = hle::State::new(&mut mem);
        let lib = loader::load_elf(&mut mem, lib_bytes, LIB_BASE, &mut |name, is_func| hle.resolve_import(name, is_func))?;
        let cache = BlockCache::new(lib.text_start, lib.text_end);
        let mut emu = Emu { cpu: Cpu::new(), mem, cache, lib, hle, plat, icount: 0, trace: false, probes: Vec::new(), render_pc: 1, hidden_hud: Vec::new(), text: Default::default(), traced: Vec::new() };
        emu.hle.init_threads(&mut emu.mem);
        game::install_probes(&mut emu);
        Ok(emu)
    }

    pub fn log(&mut self, msg: &str) {
        self.plat.log(msg);
    }

    pub fn symbolize(&self, addr: u32) -> String {
        if addr >= THUNK_BASE && addr < THUNK_END {
            return format!("<hle:{}>", self.hle.thunk_name((addr - THUNK_BASE) / 4));
        }
        match self.lib.symbolize(addr) {
            Some((n, off)) => format!("{}+0x{:x}", n, off),
            None => format!("0x{:08x}", addr),
        }
    }

    // ---- argument helpers for HLE functions ----
    #[inline]
    pub fn arg(&self, i: usize) -> u32 {
        if i < 4 {
            self.cpu.r[i]
        } else {
            self.mem.r32(self.cpu.r[13].wrapping_add(4 * (i as u32 - 4)))
        }
    }
    pub fn arg_f32(&self, i: usize) -> f32 {
        f32::from_bits(self.arg(i))
    }
    /// 64-bit argument starting at word `i` (must be even-aligned by caller).
    pub fn arg64(&self, i: usize) -> u64 {
        self.arg(i) as u64 | (self.arg(i + 1) as u64) << 32
    }
    pub fn arg_f64(&self, i: usize) -> f64 {
        f64::from_bits(self.arg64(i))
    }
    pub fn ret(&mut self, v: u32) {
        self.cpu.r[0] = v;
    }
    pub fn ret64(&mut self, v: u64) {
        self.cpu.r[0] = v as u32;
        self.cpu.r[1] = (v >> 32) as u32;
    }
    pub fn ret_f32(&mut self, v: f32) {
        self.cpu.r[0] = v.to_bits();
    }
    pub fn ret_f64(&mut self, v: f64) {
        self.ret64(v.to_bits());
    }

    /// Run the current thread for up to `budget` instructions.
    pub fn run_slice(&mut self, budget: u64) -> Stop {
        let start = self.icount;
        loop {
            let pc = self.cpu.pc;
            if pc >= THUNK_BASE && pc < 0x0002_0000 {
                if pc == RETURN_MAGIC {
                    return Stop::Return;
                }
                if pc == THREAD_EXIT_MAGIC {
                    let rv = self.cpu.r[0];
                    hle::thread::exit_current(self, rv);
                    return Stop::Blocked;
                }
                let idx = (pc - THUNK_BASE) / 4;
                match hle::dispatch(self, idx) {
                    hle::Flow::Ret => {
                        let lr = self.cpu.r[14];
                        self.cpu.thumb = lr & 1 != 0;
                        self.cpu.pc = lr & !1;
                    }
                    hle::Flow::Jump => {}
                    hle::Flow::Block => return Stop::Blocked,
                    hle::Flow::Fault(f) => return Stop::Fault(f),
                }
                if self.icount - start >= budget {
                    return Stop::Budget;
                }
                continue;
            }
            for p in self.probes.iter_mut() {
                if p.pc == pc {
                    let r0 = self.cpu.r[0];
                    p.r0 = r0;
                    p.r1 = self.cpu.r[1];
                    p.hits += 1;
                    p.last = self.icount;
                    match p.seen.iter_mut().find(|e| e.0 == r0) {
                        Some(e) => e.1 = self.icount,
                        None => {
                            if p.seen.len() >= 8 {
                                p.seen.sort_by_key(|e| std::cmp::Reverse(e.1));
                                p.seen.truncate(7);
                            }
                            p.seen.push((r0, self.icount));
                        }
                    }
                }
            }
            if (pc == self.text.hook_pcs[0] || pc == self.text.hook_pcs[1]) && !self.text.rules.is_empty() {
                let t = self.cpu.r[0];
                game::on_string_table_access(self, t);
            }
            if !self.traced.is_empty() {
                if let Some(t) = self.traced.iter().find(|t| t.0 == pc).map(|t| t.1.clone()) {
                    let desc = |emu: &Emu, v: u32| -> String {
                        if emu.hle.heap.contains(v) && emu.hle.heap.contains(emu.mem.r32(v + 0xcc)) {
                            let n = emu.mem.cstr(emu.mem.r32(v + 0xcc));
                            if !n.is_empty() && n.len() < 40 && n.bytes().all(|b| b.is_ascii_graphic()) {
                                return format!("{:x}<{}>", v, n);
                            }
                        }
                        let sb = emu.mem.cstr_bytes(v);
                        if sb.len() >= 3 && sb.len() < 40 && sb.iter().all(|b| b.is_ascii_graphic()) {
                            return format!("{:x}\"{}\"", v, String::from_utf8_lossy(sb));
                        }
                        format!("{:x}", v)
                    };
                    let msg = format!("[trace] {}({}, {}, {}, {}) from {}", t, desc(self, self.cpu.r[0]), desc(self, self.cpu.r[1]), desc(self, self.cpu.r[2]), desc(self, self.cpu.r[3]), self.symbolize(self.cpu.r[14]));
                    self.plat.log(&msg);
                }
            }
            if pc == self.render_pc && !self.hidden_hud.is_empty() && game::is_hidden_control(self, self.cpu.r[0]) {
                let lr = self.cpu.r[14];
                self.cpu.thumb = lr & 1 != 0;
                self.cpu.pc = lr & !1;
                continue;
            }
            if self.trace {
                let s = self.symbolize(pc);
                self.plat.log(&format!("[trace] {:08x} {} r0={:08x} r1={:08x} r2={:08x} r3={:08x} sp={:08x} lr={:08x}", pc, s, self.cpu.r[0], self.cpu.r[1], self.cpu.r[2], self.cpu.r[3], self.cpu.r[13], self.cpu.r[14]));
            }
            let blk = self.cache.get(&self.mem, pc, self.cpu.thumb);
            let next = self.cpu.exec_block(&mut self.mem, blk);
            self.icount += blk.insts.len() as u64;
            self.cpu.pc = next;
            if let Some(f) = self.cpu.fault.take() {
                let s = self.symbolize(next);
                return Stop::Fault(format!("{} at {:08x} ({})", f, next, s));
            }
            if self.icount - start >= budget {
                return Stop::Budget;
            }
        }
    }

    /// Set up the current CPU context to call `func` with `args`, returning to `ret`.
    pub fn setup_call(&mut self, func: u32, args: &[u32], ret: u32) {
        let nstack = args.len().saturating_sub(4) as u32;
        let mut sp = self.cpu.r[13];
        sp = (sp - nstack * 4) & !7;
        for (i, &a) in args.iter().enumerate() {
            if i < 4 {
                self.cpu.r[i] = a;
            } else {
                self.mem.w32(sp + 4 * (i as u32 - 4), a);
            }
        }
        self.cpu.r[13] = sp;
        self.cpu.r[14] = ret;
        self.cpu.thumb = func & 1 != 0;
        self.cpu.pc = func & !1;
    }

    /// Call a guest function from inside an HLE handler, on the current thread,
    /// without scheduling other threads. Returns r0 (and r1 in .1).
    pub fn call_nested(&mut self, func: u32, args: &[u32]) -> (u32, u32) {
        let saved = self.cpu.clone();
        self.setup_call(func, args, RETURN_MAGIC);
        loop {
            match self.run_slice(u64::MAX) {
                Stop::Return => break,
                Stop::Budget => {}
                Stop::Blocked => {
                    // Cannot switch threads inside a nested call; retry.
                    let _ = 0;
                }
                Stop::Fault(f) => {
                    let msg = format!("fault in nested call to {}: {}", self.symbolize(func), f);
                    self.log(&msg);
                    break;
                }
            }
        }
        let r = (self.cpu.r[0], self.cpu.r[1]);
        let fault = self.cpu.fault.take();
        self.cpu = saved;
        self.cpu.fault = fault;
        r
    }

    /// Call a guest function from the host on the main thread, letting other
    /// guest threads run while it executes.
    pub fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, String> {
        hle::thread::host_call(self, func, args)
    }

    pub fn call_export(&mut self, name: &str, args: &[u32]) -> Result<u32, String> {
        let f = *self.lib.exports.get(name).ok_or_else(|| format!("missing export {}", name))?;
        self.call(f, args)
    }

    pub fn run_init_array(&mut self) -> Result<(), String> {
        let inits = self.lib.init_array.clone();
        for f in inits {
            self.call(f, &[])?;
        }
        Ok(())
    }

    pub fn backtrace(&self) -> String {
        let mut s = format!("pc={:08x} {} lr={:08x} {}\n", self.cpu.pc, self.symbolize(self.cpu.pc), self.cpu.r[14], self.symbolize(self.cpu.r[14]));
        for i in 0..16 {
            s += &format!("r{}={:08x} ", i, self.cpu.r[i]);
        }
        s += "\nstack scan:";
        let sp = self.cpu.r[13];
        let mut n = 0;
        for i in 0..512 {
            let v = self.mem.r32(sp + i * 4);
            if v & 1 == 1 && v >= self.lib.text_start && v < self.lib.text_end {
                s += &format!("\n  [sp+{:x}] {:08x} {}", i * 4, v, self.symbolize(v));
                n += 1;
                if n > 24 {
                    break;
                }
            }
        }
        s
    }
}
