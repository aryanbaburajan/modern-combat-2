//! Differential CPU tester backend (see tools/cputest/difftest.py).
//!
//! usage: cputest <batch-in> <results-out>
//!
//! Batch format (all little-endian u32 unless noted):
//!   "MC2T" nseg { addr len bytes[len] }*  win_base win_size nwin { bytes[win_size] }*  ncase
//!   case: id addr insn(hw1 | hw2<<16) r[16] cpsr(NZCVQ in bits 31..27) fpscr s[64] win_idx
//! Result format:
//!   "MC2R" ncase, then per case:
//!   id status(0=ok,1=fault,2=panic) r[16] cpsr(NZCVQ | T<<5) fpscr s[64] next_pc ninsts nchg { off val }*

use mc2engine::cpu::{thumb, Cpu};
use mc2engine::mem::Memory;
use std::io::{BufWriter, Read, Write};
use std::panic::{self, AssertUnwindSafe};

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        v
    }
    fn bytes(&mut self, n: usize) -> &'a [u8] {
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        s
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: cputest <batch-in> <results-out>");
        std::process::exit(2);
    }
    let mut data = Vec::new();
    std::fs::File::open(&args[1]).expect("open batch").read_to_end(&mut data).unwrap();
    let mut rd = Rd { b: &data, p: 0 };
    assert_eq!(rd.bytes(4), b"MC2T", "bad batch magic");
    let mut mem = Memory::new();
    let nseg = rd.u32();
    for _ in 0..nseg {
        let a = rd.u32();
        let n = rd.u32() as usize;
        mem.write_bytes(a, rd.bytes(n));
    }
    let win_base = rd.u32();
    let win_size = rd.u32() as usize;
    let nwin = rd.u32();
    let wins: Vec<&[u8]> = (0..nwin).map(|_| rd.bytes(win_size)).collect();
    let ncase = rd.u32();

    panic::set_hook(Box::new(|_| {}));
    let mut out = BufWriter::new(std::fs::File::create(&args[2]).expect("create results"));
    out.write_all(b"MC2R").unwrap();
    out.write_all(&ncase.to_le_bytes()).unwrap();
    let mut buf: Vec<u32> = Vec::with_capacity(128);

    for _ in 0..ncase {
        let id = rd.u32();
        let addr = rd.u32();
        let insn = rd.u32();
        let mut cpu = Cpu::new();
        for i in 0..16 {
            cpu.r[i] = rd.u32();
        }
        let cpsr = rd.u32();
        cpu.set_nzcv(cpsr);
        cpu.q = cpsr & (1 << 27) != 0;
        cpu.thumb = true;
        cpu.fpscr = rd.u32();
        for i in 0..64 {
            cpu.s[i] = rd.u32();
        }
        let wi = rd.u32() as usize;
        let win = wins[wi];
        mem.write_bytes(win_base, win);
        mem.w16(addr, insn as u16);
        mem.w16(addr + 2, (insn >> 16) as u16);
        cpu.pc = addr;

        let res = panic::catch_unwind(AssertUnwindSafe(|| {
            let blk = thumb::decode_block(&|a| mem.r16(a), addr, 1);
            let n = blk.insts.len() as u32;
            let next = cpu.exec_block(&mut mem, &blk);
            (next, n)
        }));
        let (status, next, ninst) = match res {
            Ok((next, n)) => (if cpu.fault.is_some() { 1 } else { 0 }, next, n),
            Err(_) => (2, 0, 0),
        };

        buf.clear();
        buf.push(id);
        buf.push(status);
        buf.extend_from_slice(&cpu.r);
        buf.push((cpu.cpsr() & 0xf800_0000) | (cpu.thumb as u32) << 5);
        buf.push(cpu.fpscr);
        buf.extend_from_slice(&cpu.s);
        buf.push(next);
        buf.push(ninst);
        let cur = mem.slice(win_base, win_size);
        let chg: Vec<(u32, u32)> =
            cur.iter().zip(win.iter()).enumerate().filter(|(_, (a, b))| a != b).map(|(i, (a, _))| (i as u32, *a as u32)).collect();
        buf.push(chg.len() as u32);
        for (o, v) in chg {
            buf.push(o);
            buf.push(v);
        }
        for w in &buf {
            out.write_all(&w.to_le_bytes()).unwrap();
        }
    }
    out.flush().unwrap();
}
