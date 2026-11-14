//! High-level emulation of the libraries the game imports.

pub mod fmt;
pub mod gl;
pub mod heap;
pub mod jni;
pub mod libc;
pub mod math;
pub mod net;
pub mod stdio;
pub mod texdecode;
pub mod thread;

use crate::mem::*;
use crate::Emu;
use std::collections::HashMap;

pub enum Flow {
    /// Return to LR.
    Ret,
    /// Handler already set the PC.
    Jump,
    /// Current thread must block; the call will be retried when it resumes.
    Block,
    Fault(String),
}

pub type HleFn = fn(&mut Emu) -> Flow;

pub struct Thunk {
    pub name: String,
    pub f: Option<HleFn>,
}

pub struct State {
    pub thunks: Vec<Thunk>,
    by_name: HashMap<String, u32>,
    registry: HashMap<&'static str, HleFn>,
    pub data_syms: HashMap<&'static str, u32>,
    static_next: u32,
    pub heap: heap::Heap,
    pub vfs: stdio::Vfs,
    pub threads: thread::Threads,
    pub jni: jni::Jni,
    pub gl: gl::GlState,
    pub rand48: u64,
    pub strtok_save: u32,
    pub unimpl_seen: HashMap<String, u32>,
    pub scratch: u32,
}

impl State {
    pub fn new(mem: &mut Memory) -> State {
        let mut registry: HashMap<&'static str, HleFn> = HashMap::new();
        libc::register(&mut registry);
        net::register(&mut registry);
        math::register(&mut registry);
        stdio::register(&mut registry);
        thread::register(&mut registry);
        gl::register(&mut registry);
        let mut st = State {
            thunks: Vec::new(),
            by_name: HashMap::new(),
            registry,
            data_syms: HashMap::new(),
            static_next: STATIC_BASE,
            heap: heap::Heap::new(HEAP_BASE, HEAP_END),
            vfs: stdio::Vfs::new(),
            threads: thread::Threads::new(),
            jni: jni::Jni::new(),
            gl: gl::GlState::new(),
            rand48: 0x1234ABCD330E,
            strtok_save: 0,
            unimpl_seen: HashMap::new(),
            scratch: 0,
        };
        // Reserve special thunks so their addresses are stable.
        libc::init_data(&mut st, mem);
        stdio::init_data(&mut st, mem);
        st.scratch = st.static_alloc(4096, 16);
        jni::init(&mut st, mem);
        st
    }

    pub fn static_alloc(&mut self, size: u32, align: u32) -> u32 {
        let a = (self.static_next + align - 1) & !(align - 1);
        self.static_next = a + size;
        assert!(self.static_next <= STATIC_END, "static area exhausted");
        a
    }

    /// Get (or create) the thunk address for an HLE function name.
    pub fn thunk_for(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.by_name.get(name) {
            return THUNK_BASE + i * 4;
        }
        let f = self.registry.get(name).copied();
        let i = self.thunks.len() as u32;
        assert!(THUNK_BASE + i * 4 < THUNK_END, "too many thunks");
        self.thunks.push(Thunk { name: name.to_string(), f });
        self.by_name.insert(name.to_string(), i);
        THUNK_BASE + i * 4
    }

    /// Register an HLE handler under a private name and return its thunk.
    pub fn thunk_fn(&mut self, name: &str, f: HleFn) -> u32 {
        let a = self.thunk_for(name);
        let i = ((a - THUNK_BASE) / 4) as usize;
        self.thunks[i].f = Some(f);
        a
    }

    pub fn thunk_name(&self, idx: u32) -> &str {
        self.thunks.get(idx as usize).map(|t| t.name.as_str()).unwrap_or("?")
    }

    pub fn resolve_import(&mut self, name: &str, is_func: bool) -> u32 {
        if let Some(&a) = self.data_syms.get(name) {
            return a;
        }
        if !is_func && !self.registry.contains_key(name) {
            // Unknown data import: give it some zeroed storage.
            let a = self.static_alloc(256, 16);
            let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
            self.data_syms.insert(leaked, a);
            return a;
        }
        self.thunk_for(name)
    }
}

pub fn dispatch(emu: &mut Emu, idx: u32) -> Flow {
    let f = match emu.hle.thunks.get(idx as usize) {
        Some(t) => t.f,
        None => return Flow::Fault(format!("jump to bad thunk index {}", idx)),
    };
    match f {
        Some(f) => f(emu),
        None => {
            let name = emu.hle.thunks[idx as usize].name.clone();
            let n = emu.hle.unimpl_seen.entry(name.clone()).or_insert(0);
            *n += 1;
            if *n <= 3 {
                let lr = emu.cpu.r[14];
                let msg = format!("[hle] unimplemented {} called from {}", name, emu.symbolize(lr));
                emu.log(&msg);
            }
            emu.ret(0);
            Flow::Ret
        }
    }
}

/// Reads variadic arguments following the AAPCS (softfp) rules, either from
/// the current call's registers/stack or from a `va_list` pointer.
pub struct VaArgs {
    regs: [u32; 4],
    next_reg: usize,
    stack: u32,
}

impl VaArgs {
    /// Variadic args starting at argument word `first`.
    pub fn from_call(emu: &Emu, first: usize) -> VaArgs {
        let mut regs = [0u32; 4];
        regs.copy_from_slice(&emu.cpu.r[0..4]);
        if first >= 4 {
            VaArgs { regs, next_reg: 4, stack: emu.cpu.r[13] + 4 * (first as u32 - 4) }
        } else {
            VaArgs { regs, next_reg: first, stack: emu.cpu.r[13] }
        }
    }
    pub fn from_va_list(ptr: u32) -> VaArgs {
        VaArgs { regs: [0; 4], next_reg: 4, stack: ptr }
    }
    pub fn u32(&mut self, mem: &Memory) -> u32 {
        if self.next_reg < 4 {
            self.next_reg += 1;
            self.regs[self.next_reg - 1]
        } else {
            let v = mem.r32(self.stack);
            self.stack += 4;
            v
        }
    }
    pub fn u64(&mut self, mem: &Memory) -> u64 {
        if self.next_reg < 4 {
            if self.next_reg & 1 != 0 {
                self.next_reg += 1;
            }
            if self.next_reg < 4 {
                let v = self.regs[self.next_reg] as u64 | (self.regs[self.next_reg + 1] as u64) << 32;
                self.next_reg += 2;
                return v;
            }
        }
        self.stack = (self.stack + 7) & !7;
        let v = mem.r64(self.stack);
        self.stack += 8;
        v
    }
    pub fn f64(&mut self, mem: &Memory) -> f64 {
        f64::from_bits(self.u64(mem))
    }
}
