//! Guest threads, scheduled cooperatively (with instruction-count time slicing)
//! on the single host thread, plus pthread mutexes and TLS keys.

use super::{Flow, HleFn, State};
use crate::cpu::Cpu;
use crate::mem::*;
use crate::{Emu, Stop};
use std::collections::HashMap;

const MAIN_STACK: u32 = 8 << 20;
const DEFAULT_STACK: u32 = 1 << 20;
const MAIN_SLICE: u64 = 4_000_000;
const BG_SLICE: u64 = 400_000;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TState {
    Ready,
    Sleeping(f64),
    WaitMutex(u32),
    WaitJoin(u32),
    Done,
}

pub struct Thread {
    pub tid: u32,
    pub ctx: Option<Cpu>,
    pub state: TState,
    pub retval: u32,
    pub errno: u32,
    pub tls: HashMap<u32, u32>,
    pub start: u32,
}

pub struct Threads {
    pub list: Vec<Thread>,
    pub cur: usize,
    mutexes: HashMap<u32, (u32, u32)>, // addr -> (owner tid, recursion count)
    next_key: u32,
    stack_next: u32,
    next_tid: u32,
    pub switches: u64,
}

impl Threads {
    pub fn new() -> Threads {
        Threads { list: Vec::new(), cur: 0, mutexes: HashMap::new(), next_key: 1, stack_next: STACK_BASE, next_tid: 1, switches: 0 }
    }
    pub fn cur_tid(&self) -> u32 {
        self.list.get(self.cur).map(|t| t.tid).unwrap_or(1)
    }
}

impl State {
    pub fn init_threads(&mut self, mem: &mut Memory) {
        let errno = self.static_alloc(4, 4);
        mem.w32(errno, 0);
        let tid = self.threads.next_tid;
        self.threads.next_tid += 1;
        self.threads.list.push(Thread { tid, ctx: None, state: TState::Ready, retval: 0, errno, tls: HashMap::new(), start: 0 });
    }
}

pub fn init_main_stack(emu: &mut Emu) {
    emu.cpu.r[13] = STACK_END - 64;
    let _ = MAIN_STACK;
}

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    macro_rules! reg {
        ($($name:literal => $f:expr),* $(,)?) => { $( r.insert($name, $f); )* };
    }
    reg! {
        "pthread_create" => pthread_create, "pthread_join" => pthread_join, "pthread_exit" => pthread_exit,
        "pthread_self" => pthread_self, "pthread_kill" => ret0, "pthread_detach" => ret0,
        "pthread_mutex_init" => mutex_init, "pthread_mutex_destroy" => mutex_destroy,
        "pthread_mutex_lock" => mutex_lock, "pthread_mutex_unlock" => mutex_unlock,
        "pthread_mutex_trylock" => mutex_trylock,
        "pthread_key_create" => key_create, "pthread_key_delete" => ret0,
        "pthread_getspecific" => getspecific, "pthread_setspecific" => setspecific,
        "pthread_attr_init" => ret0, "pthread_attr_destroy" => ret0,
        "pthread_attr_setstacksize" => ret0, "pthread_attr_setdetachstate" => ret0,
        "sched_yield" => sched_yield,
    }
}

fn ret0(emu: &mut Emu) -> Flow {
    emu.ret(0);
    Flow::Ret
}

pub fn errno_addr(emu: &Emu) -> u32 {
    emu.hle.threads.list[emu.hle.threads.cur].errno
}

fn runnable(emu: &Emu, i: usize, now: f64) -> bool {
    let t = &emu.hle.threads.list[i];
    match t.state {
        TState::Ready => true,
        TState::Sleeping(until) => now >= until,
        TState::WaitMutex(a) => match emu.hle.threads.mutexes.get(&a) {
            None => true,
            Some(&(owner, _)) => owner == t.tid,
        },
        TState::WaitJoin(tid) => emu.hle.threads.list.iter().find(|x| x.tid == tid).map_or(true, |x| x.state == TState::Done),
        TState::Done => false,
    }
}

fn switch_to(emu: &mut Emu, i: usize) {
    let cur = emu.hle.threads.cur;
    if i == cur {
        return;
    }
    let mut next = emu.hle.threads.list[i].ctx.take().expect("thread context missing");
    std::mem::swap(&mut emu.cpu, &mut next);
    emu.hle.threads.list[cur].ctx = Some(next);
    emu.cpu.clear_exclusive();
    emu.hle.threads.cur = i;
    emu.hle.threads.switches += 1;
}

/// Choose the next thread to run. `exclude_main` keeps thread 0 parked.
fn pick_next(emu: &mut Emu, exclude_main: bool) -> Option<usize> {
    let n = emu.hle.threads.list.len();
    let now = emu.plat.now_ms();
    let cur = emu.hle.threads.cur;
    for k in 1..=n {
        let i = (cur + k) % n;
        if exclude_main && i == 0 {
            continue;
        }
        if runnable(emu, i, now) {
            return Some(i);
        }
    }
    // Nobody is runnable: fast-forward the earliest sleeper.
    let mut best: Option<(usize, f64)> = None;
    for i in 0..n {
        if exclude_main && i == 0 {
            continue;
        }
        if let TState::Sleeping(until) = emu.hle.threads.list[i].state {
            if best.map_or(true, |(_, b)| until < b) {
                best = Some((i, until));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn resume(emu: &mut Emu, i: usize) {
    switch_to(emu, i);
    emu.hle.threads.list[i].state = TState::Ready;
}

/// Run `func` on the main thread until it returns, interleaving other threads.
pub fn host_call(emu: &mut Emu, func: u32, args: &[u32]) -> Result<u32, String> {
    if emu.hle.threads.cur != 0 {
        return Err("host_call while a background thread is current".into());
    }
    if emu.cpu.r[13] == 0 {
        init_main_stack(emu);
    }
    let saved_sp = emu.cpu.r[13];
    let saved = emu.cpu.clone();
    emu.setup_call(func, args, RETURN_MAGIC);
    emu.hle.threads.list[0].state = TState::Ready;
    loop {
        let slice = if emu.hle.threads.cur == 0 { MAIN_SLICE } else { BG_SLICE };
        match emu.run_slice(slice) {
            Stop::Return => {
                if emu.hle.threads.cur == 0 {
                    let r = emu.cpu.r[0];
                    let r1 = emu.cpu.r[1];
                    emu.cpu = saved;
                    emu.cpu.r[13] = saved_sp;
                    emu.cpu.r[0] = r;
                    emu.cpu.r[1] = r1;
                    return Ok(r);
                }
                return Err("background thread returned to host".into());
            }
            Stop::Budget => {
                if emu.hle.threads.list.len() > 1 {
                    if let Some(i) = pick_next(emu, false) {
                        resume(emu, i);
                    }
                }
            }
            Stop::Blocked => match pick_next(emu, false) {
                Some(i) => resume(emu, i),
                None => {
                    let st = emu.hle.threads.list[emu.hle.threads.cur].state;
                    return Err(format!("deadlock: no runnable threads (current state {:?})\n{}", st, emu.backtrace()));
                }
            },
            Stop::Fault(f) => {
                let tid = emu.hle.threads.cur_tid();
                return Err(format!("thread {}: {}\n{}", tid, f, emu.backtrace()));
            }
        }
    }
}

/// Give background threads up to `budget` instructions while the main thread is idle.
pub fn run_background(emu: &mut Emu, budget: u64) -> Result<(), String> {
    if emu.hle.threads.list.len() <= 1 {
        return Ok(());
    }
    let start = emu.icount;
    let now = emu.plat.now_ms();
    if !(1..emu.hle.threads.list.len()).any(|i| runnable(emu, i, now)) {
        return Ok(());
    }
    while emu.icount - start < budget {
        let now = emu.plat.now_ms();
        let cand = (1..emu.hle.threads.list.len()).map(|k| (emu.hle.threads.cur + k - 1) % (emu.hle.threads.list.len() - 1) + 1).find(|&i| runnable(emu, i, now));
        let i = match cand {
            Some(i) => i,
            None => break,
        };
        resume(emu, i);
        match emu.run_slice(BG_SLICE.min(budget)) {
            Stop::Return => return Err("background thread returned to host".into()),
            Stop::Budget | Stop::Blocked => {}
            Stop::Fault(f) => {
                let tid = emu.hle.threads.cur_tid();
                let bt = emu.backtrace();
                switch_to(emu, 0);
                return Err(format!("thread {}: {}\n{}", tid, f, bt));
            }
        }
    }
    switch_to(emu, 0);
    Ok(())
}

pub fn exit_current(emu: &mut Emu, rv: u32) {
    let cur = emu.hle.threads.cur;
    let t = &mut emu.hle.threads.list[cur];
    t.state = TState::Done;
    t.retval = rv;
    let tid = t.tid;
    // Release any mutexes it still holds.
    emu.hle.threads.mutexes.retain(|_, v| v.0 != tid);
    let msg = format!("[thread] thread {} exited ({})", tid, rv);
    emu.log(&msg);
}

pub fn sleep_ms(emu: &mut Emu, ms: f64) -> Flow {
    emu.ret(0);
    let lr = emu.cpu.r[14];
    emu.cpu.thumb = lr & 1 != 0;
    emu.cpu.pc = lr & !1;
    if emu.hle.threads.list.len() <= 1 {
        return Flow::Jump;
    }
    let now = emu.plat.now_ms();
    let cur = emu.hle.threads.cur;
    emu.hle.threads.list[cur].state = TState::Sleeping(now + ms.max(0.0));
    Flow::Block
}

fn sched_yield(emu: &mut Emu) -> Flow {
    sleep_ms(emu, 0.0)
}

fn pthread_create(emu: &mut Emu) -> Flow {
    let (out, attr, start, arg) = (emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3));
    let mut size = DEFAULT_STACK;
    if attr != 0 {
        let s = emu.mem.r32(attr + 8);
        if s >= 64 * 1024 && s <= 16 << 20 {
            size = s;
        }
    }
    let size = (size + 0xfff) & !0xfff;
    let lo = emu.hle.threads.stack_next;
    if lo + size > STACK_END - MAIN_STACK {
        emu.ret(11);
        return Flow::Ret;
    }
    emu.hle.threads.stack_next += size + 0x1000;
    let mut ctx = Cpu::new();
    ctx.r[13] = lo + size - 16;
    ctx.r[0] = arg;
    ctx.r[14] = THREAD_EXIT_MAGIC;
    ctx.thumb = start & 1 != 0;
    ctx.pc = start & !1;
    let errno = emu.hle.static_alloc(4, 4);
    let tid = emu.hle.threads.next_tid;
    emu.hle.threads.next_tid += 1;
    emu.hle.threads.list.push(Thread { tid, ctx: Some(ctx), state: TState::Ready, retval: 0, errno, tls: HashMap::new(), start });
    if out != 0 {
        emu.mem.w32(out, tid);
    }
    let msg = format!("[thread] created thread {} at {} (arg {:08x})", tid, emu.symbolize(start), arg);
    emu.log(&msg);
    emu.ret(0);
    Flow::Ret
}

fn pthread_join(emu: &mut Emu) -> Flow {
    let (tid, out) = (emu.arg(0), emu.arg(1));
    let t = match emu.hle.threads.list.iter().find(|t| t.tid == tid) {
        Some(t) => (t.state, t.retval),
        None => {
            emu.ret(3);
            return Flow::Ret;
        }
    };
    if t.0 == TState::Done {
        if out != 0 {
            emu.mem.w32(out, t.1);
        }
        emu.ret(0);
        return Flow::Ret;
    }
    let cur = emu.hle.threads.cur;
    emu.hle.threads.list[cur].state = TState::WaitJoin(tid);
    Flow::Block
}

fn pthread_exit(emu: &mut Emu) -> Flow {
    let rv = emu.arg(0);
    if emu.hle.threads.cur == 0 {
        return Flow::Fault("pthread_exit on main thread".into());
    }
    exit_current(emu, rv);
    Flow::Block
}

fn pthread_self(emu: &mut Emu) -> Flow {
    let t = emu.hle.threads.cur_tid();
    emu.ret(t);
    Flow::Ret
}

fn mutex_init(emu: &mut Emu) -> Flow {
    let (m, attr) = (emu.arg(0), emu.arg(1));
    let ty = if attr != 0 { emu.mem.r32(attr) } else { 0 };
    emu.mem.w32(m, match ty {
        1 => 0x4000,
        2 => 0x8000,
        _ => 0,
    });
    emu.hle.threads.mutexes.remove(&m);
    emu.ret(0);
    Flow::Ret
}
fn mutex_destroy(emu: &mut Emu) -> Flow {
    let m = emu.arg(0);
    emu.hle.threads.mutexes.remove(&m);
    emu.ret(0);
    Flow::Ret
}
fn try_lock(emu: &mut Emu, m: u32) -> bool {
    let tid = emu.hle.threads.cur_tid();
    match emu.hle.threads.mutexes.get_mut(&m) {
        None => {
            emu.hle.threads.mutexes.insert(m, (tid, 1));
            true
        }
        Some(e) if e.0 == tid => {
            e.1 += 1;
            true
        }
        Some(_) => false,
    }
}
fn mutex_lock(emu: &mut Emu) -> Flow {
    let m = emu.arg(0);
    if try_lock(emu, m) {
        emu.ret(0);
        return Flow::Ret;
    }
    let cur = emu.hle.threads.cur;
    emu.hle.threads.list[cur].state = TState::WaitMutex(m);
    Flow::Block
}
fn mutex_trylock(emu: &mut Emu) -> Flow {
    let m = emu.arg(0);
    let ok = try_lock(emu, m);
    emu.ret(if ok { 0 } else { 16 });
    Flow::Ret
}
fn mutex_unlock(emu: &mut Emu) -> Flow {
    let m = emu.arg(0);
    let tid = emu.hle.threads.cur_tid();
    let mut release = false;
    if let Some(e) = emu.hle.threads.mutexes.get_mut(&m) {
        if e.0 == tid {
            e.1 -= 1;
            release = e.1 == 0;
        }
    }
    if release {
        emu.hle.threads.mutexes.remove(&m);
    }
    emu.ret(0);
    Flow::Ret
}

fn key_create(emu: &mut Emu) -> Flow {
    let out = emu.arg(0);
    let k = emu.hle.threads.next_key;
    emu.hle.threads.next_key += 1;
    emu.mem.w32(out, k);
    emu.ret(0);
    Flow::Ret
}
fn getspecific(emu: &mut Emu) -> Flow {
    let k = emu.arg(0);
    let cur = emu.hle.threads.cur;
    let v = emu.hle.threads.list[cur].tls.get(&k).copied().unwrap_or(0);
    emu.ret(v);
    Flow::Ret
}
fn setspecific(emu: &mut Emu) -> Flow {
    let (k, v) = (emu.arg(0), emu.arg(1));
    let cur = emu.hle.threads.cur;
    emu.hle.threads.list[cur].tls.insert(k, v);
    emu.ret(0);
    Flow::Ret
}
