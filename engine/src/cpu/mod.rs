//! ARMv7-A user-mode CPU interpreter (Thumb-2 + VFPv3, minimal A32).

pub mod arm;
pub mod inst;
pub mod thumb;

use crate::mem::Memory;
use inst::*;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Cpu {
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub q: bool,
    pub thumb: bool,
    /// VFP register file: s[0..64] (d[i] = s[2i] | s[2i+1] << 32).
    pub s: [u32; 64],
    pub fpscr: u32,
    pub excl: u32,
    /// Address of the next block to execute.
    pub pc: u32,
    /// Set when an executed instruction cannot be handled.
    pub fault: Option<String>,
}

const NO_EXCL: u32 = 0xffff_ffff;

#[inline(always)]
fn add_c(a: u32, b: u32, c: bool) -> (u32, bool, bool) {
    let r64 = a as u64 + b as u64 + c as u64;
    let r = r64 as u32;
    (r, r64 >> 32 != 0, ((a ^ r) & (b ^ r)) >> 31 != 0)
}

#[inline(always)]
fn shift_c(v: u32, ty: Sh, amt: u32, cin: bool) -> (u32, bool) {
    match ty {
        Sh::Lsl => {
            if amt == 0 {
                (v, cin)
            } else if amt < 32 {
                (v << amt, (v >> (32 - amt)) & 1 != 0)
            } else if amt == 32 {
                (0, v & 1 != 0)
            } else {
                (0, false)
            }
        }
        Sh::Lsr => {
            if amt == 0 {
                (v, cin)
            } else if amt < 32 {
                (v >> amt, (v >> (amt - 1)) & 1 != 0)
            } else if amt == 32 {
                (0, v >> 31 != 0)
            } else {
                (0, false)
            }
        }
        Sh::Asr => {
            if amt == 0 {
                (v, cin)
            } else if amt < 32 {
                (((v as i32) >> amt) as u32, (v >> (amt - 1)) & 1 != 0)
            } else {
                (((v as i32) >> 31) as u32, v >> 31 != 0)
            }
        }
        Sh::Ror => {
            if amt == 0 {
                (v, cin)
            } else {
                let a = amt & 31;
                let r = if a == 0 { v } else { v.rotate_right(a) };
                (r, r >> 31 != 0)
            }
        }
        Sh::Rrx => (((cin as u32) << 31) | (v >> 1), v & 1 != 0),
    }
}

impl Cpu {
    pub fn new() -> Cpu {
        Cpu {
            r: [0; 16],
            n: false,
            z: false,
            c: false,
            v: false,
            q: false,
            thumb: true,
            s: [0; 64],
            fpscr: 0,
            excl: NO_EXCL,
            pc: 0,
            fault: None,
        }
    }

    pub fn cpsr(&self) -> u32 {
        (self.n as u32) << 31
            | (self.z as u32) << 30
            | (self.c as u32) << 29
            | (self.v as u32) << 28
            | (self.q as u32) << 27
            | (self.thumb as u32) << 5
            | 0x10
    }

    pub fn set_nzcv(&mut self, v: u32) {
        self.n = v & (1 << 31) != 0;
        self.z = v & (1 << 30) != 0;
        self.c = v & (1 << 29) != 0;
        self.v = v & (1 << 28) != 0;
    }

    pub fn clear_exclusive(&mut self) {
        self.excl = NO_EXCL;
    }

    #[inline(always)]
    pub fn fs(&self, i: u8) -> f32 {
        f32::from_bits(self.s[i as usize])
    }
    #[inline(always)]
    pub fn set_fs(&mut self, i: u8, v: f32) {
        self.s[i as usize] = v.to_bits();
    }
    #[inline(always)]
    pub fn d_bits(&self, i: u8) -> u64 {
        let i = i as usize * 2;
        self.s[i] as u64 | (self.s[i + 1] as u64) << 32
    }
    #[inline(always)]
    pub fn set_d_bits(&mut self, i: u8, v: u64) {
        let i = i as usize * 2;
        self.s[i] = v as u32;
        self.s[i + 1] = (v >> 32) as u32;
    }
    #[inline(always)]
    pub fn fd(&self, i: u8) -> f64 {
        f64::from_bits(self.d_bits(i))
    }
    #[inline(always)]
    pub fn set_fd(&mut self, i: u8, v: f64) {
        self.set_d_bits(i, v.to_bits())
    }

    #[inline(always)]
    pub fn cond(&self, c: u8) -> bool {
        match c {
            0 => self.z,
            1 => !self.z,
            2 => self.c,
            3 => !self.c,
            4 => self.n,
            5 => !self.n,
            6 => self.v,
            7 => !self.v,
            8 => self.c && !self.z,
            9 => !self.c || self.z,
            10 => self.n == self.v,
            11 => self.n != self.v,
            12 => !self.z && self.n == self.v,
            13 => self.z || self.n != self.v,
            _ => true,
        }
    }

    #[inline(always)]
    fn op2(&self, o: Op2) -> (u32, bool) {
        match o {
            Op2::Imm(v) => (v, self.c),
            Op2::ImmC(v, c) => (v, c),
            Op2::Reg(r) => (self.r[r as usize], self.c),
            Op2::Shift(r, sh, a) => shift_c(self.r[r as usize], sh, a as u32, self.c),
            Op2::ShiftReg(r, sh, rs) => shift_c(self.r[r as usize], sh, self.r[rs as usize] & 0xff, self.c),
        }
    }

    #[inline(always)]
    fn addr(&self, a: &Addr) -> (u32, Option<(u8, u32)>) {
        match *a {
            Addr::Imm { rn, off, pre, wb } => {
                let base = self.r[rn as usize];
                let oa = base.wrapping_add(off as u32);
                (if pre { oa } else { base }, if wb { Some((rn, oa)) } else { None })
            }
            Addr::Reg { rn, rm, sh, add, pre, wb } => {
                let base = self.r[rn as usize];
                let off = self.r[rm as usize] << sh;
                let oa = if add { base.wrapping_add(off) } else { base.wrapping_sub(off) };
                (if pre { oa } else { base }, if wb { Some((rn, oa)) } else { None })
            }
            Addr::Lit(v) => (v, None),
        }
    }

    #[inline(always)]
    fn load(mem: &Memory, sz: Sz, a: u32) -> u32 {
        match sz {
            Sz::B => mem.r8(a) as u32,
            Sz::Sb => mem.r8(a) as i8 as i32 as u32,
            Sz::H => mem.r16(a) as u32,
            Sz::Sh => mem.r16(a) as i16 as i32 as u32,
            Sz::W => mem.r32(a),
        }
    }

    #[inline(always)]
    fn store(mem: &mut Memory, sz: Sz, a: u32, v: u32) {
        match sz {
            Sz::B | Sz::Sb => mem.w8(a, v as u8),
            Sz::H | Sz::Sh => mem.w16(a, v as u16),
            Sz::W => mem.w32(a, v),
        }
    }

    #[inline(always)]
    fn set_nz(&mut self, r: u32) {
        self.n = (r as i32) < 0;
        self.z = r == 0;
    }

    /// Execute a decoded block. Returns the next PC (mode in `self.thumb`).
    pub fn exec_block(&mut self, mem: &mut Memory, blk: &Block) -> u32 {
        let pcoff = if blk.arm { 8 } else { 4 };
        for d in blk.insts.iter() {
            if d.cond != COND_AL && !self.cond(d.cond) {
                continue;
            }
            self.r[15] = d.pc.wrapping_add(pcoff);
            if let Some(next) = self.exec(mem, d, blk.arm) {
                return next;
            }
        }
        blk.end
    }

    /// Execute one instruction. Returns Some(next_pc) if control flow changed.
    #[inline(always)]
    fn exec(&mut self, mem: &mut Memory, d: &Decoded, arm: bool) -> Option<u32> {
        match d.inst {
            Inst::Nop => {}
            Inst::Alu { op, s, rd, rn, op2 } => {
                let a = self.r[rn as usize];
                let r: u32;
                match op {
                    Alu::Add | Alu::Sub | Alu::Rsb | Alu::Adc | Alu::Sbc | Alu::Rsc | Alu::Cmp | Alu::Cmn => {
                        let b = match op2 {
                            Op2::Imm(v) | Op2::ImmC(v, _) => v,
                            Op2::Reg(x) => self.r[x as usize],
                            _ => self.op2(op2).0,
                        };
                        let (res, c, v) = match op {
                            Alu::Add | Alu::Cmn => add_c(a, b, false),
                            Alu::Sub | Alu::Cmp => add_c(a, !b, true),
                            Alu::Rsb => add_c(!a, b, true),
                            Alu::Adc => add_c(a, b, self.c),
                            Alu::Sbc => add_c(a, !b, self.c),
                            _ => add_c(!a, b, self.c),
                        };
                        if s {
                            self.set_nz(res);
                            self.c = c;
                            self.v = v;
                        }
                        if matches!(op, Alu::Cmp | Alu::Cmn) {
                            return None;
                        }
                        r = res;
                    }
                    _ => {
                        let (b, sc) = self.op2(op2);
                        let res = match op {
                            Alu::And | Alu::Tst => a & b,
                            Alu::Eor | Alu::Teq => a ^ b,
                            Alu::Orr => a | b,
                            Alu::Orn => a | !b,
                            Alu::Bic => a & !b,
                            Alu::Mov => b,
                            _ => !b, // Mvn
                        };
                        if s {
                            self.set_nz(res);
                            self.c = sc;
                        }
                        if matches!(op, Alu::Tst | Alu::Teq) {
                            return None;
                        }
                        r = res;
                    }
                }
                if rd == 15 {
                    if arm {
                        return Some(self.bx_write(r));
                    }
                    return Some(r & !1);
                }
                self.r[rd as usize] = r;
            }
            Inst::Mul { s, rd, rn, rm } => {
                let r = self.r[rn as usize].wrapping_mul(self.r[rm as usize]);
                if s {
                    self.set_nz(r);
                }
                self.r[rd as usize] = r;
            }
            Inst::Mla { rd, rn, rm, ra, sub } => {
                let p = self.r[rn as usize].wrapping_mul(self.r[rm as usize]);
                let a = self.r[ra as usize];
                self.r[rd as usize] = if sub { a.wrapping_sub(p) } else { a.wrapping_add(p) };
            }
            Inst::MulL { k, lo, hi, rn, rm } => {
                let x = self.r[rn as usize];
                let y = self.r[rm as usize];
                let acc = (self.r[hi as usize] as u64) << 32 | self.r[lo as usize] as u64;
                let r: u64 = match k {
                    MulL::Umull => x as u64 * y as u64,
                    MulL::Smull => (x as i32 as i64 * y as i32 as i64) as u64,
                    MulL::Umlal => (x as u64 * y as u64).wrapping_add(acc),
                    MulL::Smlal => ((x as i32 as i64 * y as i32 as i64) as u64).wrapping_add(acc),
                };
                self.r[lo as usize] = r as u32;
                self.r[hi as usize] = (r >> 32) as u32;
            }
            Inst::SmulXY { rd, rn, rm, nt, mt, ra } => {
                let x = self.r[rn as usize];
                let y = self.r[rm as usize];
                let a = (if nt { x >> 16 } else { x }) as u16 as i16 as i32;
                let b = (if mt { y >> 16 } else { y }) as u16 as i16 as i32;
                let mut r = a.wrapping_mul(b);
                if ra != 15 {
                    let (s, o) = r.overflowing_add(self.r[ra as usize] as i32);
                    if o {
                        self.q = true;
                    }
                    r = s;
                }
                self.r[rd as usize] = r as u32;
            }
            Inst::Div { signed, rd, rn, rm } => {
                let x = self.r[rn as usize];
                let y = self.r[rm as usize];
                self.r[rd as usize] = if y == 0 {
                    0
                } else if signed {
                    (x as i32).wrapping_div(y as i32) as u32
                } else {
                    x / y
                };
            }
            Inst::Movt { rd, imm } => {
                self.r[rd as usize] = (self.r[rd as usize] & 0xffff) | (imm as u32) << 16;
            }
            Inst::Bfx { signed, rd, rn, lsb, width } => {
                let v = self.r[rn as usize];
                let w = width as u32;
                let l = lsb as u32;
                self.r[rd as usize] = if signed {
                    (((v << (32 - l - w).min(31)) as i32) >> (32 - w)) as u32
                } else if w >= 32 {
                    v >> l
                } else {
                    (v >> l) & ((1u32 << w) - 1)
                };
                if signed && l + w > 32 {
                    self.r[rd as usize] = 0;
                }
            }
            Inst::Bfi { rd, rn, lsb, width } => {
                let w = width as u32;
                let l = lsb as u32;
                let m = if w >= 32 { 0xffff_ffff } else { ((1u32 << w) - 1) << l };
                let src = if rn == 15 { 0 } else { self.r[rn as usize] << l };
                self.r[rd as usize] = (self.r[rd as usize] & !m) | (src & m);
            }
            Inst::Ext { k, rd, rn, rm, rot } => {
                let v = self.r[rm as usize].rotate_right(rot as u32);
                let e = match k {
                    ExtK::Sxtb => v as u8 as i8 as i32 as u32,
                    ExtK::Sxth => v as u16 as i16 as i32 as u32,
                    ExtK::Uxtb => v & 0xff,
                    ExtK::Uxth => v & 0xffff,
                };
                self.r[rd as usize] = if rn == 15 { e } else { self.r[rn as usize].wrapping_add(e) };
            }
            Inst::Clz { rd, rm } => self.r[rd as usize] = self.r[rm as usize].leading_zeros(),
            Inst::Rev { rd, rm } => self.r[rd as usize] = self.r[rm as usize].swap_bytes(),
            Inst::Rev16 { rd, rm } => {
                let v = self.r[rm as usize];
                self.r[rd as usize] = ((v & 0x00ff_00ff) << 8) | ((v >> 8) & 0x00ff_00ff);
            }
            Inst::Revsh { rd, rm } => {
                self.r[rd as usize] = (self.r[rm as usize] as u16).swap_bytes() as i16 as i32 as u32;
            }
            Inst::Rbit { rd, rm } => self.r[rd as usize] = self.r[rm as usize].reverse_bits(),
            Inst::Ssat { rd, rn, sat, sh, amt, unsigned } => {
                let v = shift_c(self.r[rn as usize], sh, amt as u32, self.c).0 as i32 as i64;
                let (lo, hi) = if unsigned {
                    (0i64, (1i64 << sat) - 1)
                } else {
                    (-(1i64 << (sat - 1)), (1i64 << (sat - 1)) - 1)
                };
                let r = v.clamp(lo, hi);
                if r != v {
                    self.q = true;
                }
                self.r[rd as usize] = r as u32;
            }
            Inst::Ld { sz, rt, a } => {
                let (addr, wb) = self.addr(&a);
                let v = Self::load(mem, sz, addr);
                if let Some((rn, nv)) = wb {
                    self.r[rn as usize] = nv;
                }
                if rt == 15 {
                    return Some(self.bx_write(v));
                }
                self.r[rt as usize] = v;
            }
            Inst::St { sz, rt, a } => {
                let (addr, wb) = self.addr(&a);
                let v = self.r[rt as usize];
                Self::store(mem, sz, addr, v);
                if let Some((rn, nv)) = wb {
                    self.r[rn as usize] = nv;
                }
            }
            Inst::Ldrd { rt, rt2, a } => {
                let (addr, wb) = self.addr(&a);
                let v1 = mem.r32(addr);
                let v2 = mem.r32(addr.wrapping_add(4));
                if let Some((rn, nv)) = wb {
                    self.r[rn as usize] = nv;
                }
                self.r[rt as usize] = v1;
                self.r[rt2 as usize] = v2;
            }
            Inst::Strd { rt, rt2, a } => {
                let (addr, wb) = self.addr(&a);
                mem.w32(addr, self.r[rt as usize]);
                mem.w32(addr.wrapping_add(4), self.r[rt2 as usize]);
                if let Some((rn, nv)) = wb {
                    self.r[rn as usize] = nv;
                }
            }
            Inst::Ldm { rn, list, wb, db } => {
                let cnt = list.count_ones();
                let base = self.r[rn as usize];
                let mut a = if db { base.wrapping_sub(cnt * 4) } else { base };
                let nb = if db { base.wrapping_sub(cnt * 4) } else { base.wrapping_add(cnt * 4) };
                if wb {
                    self.r[rn as usize] = nb;
                }
                let mut newpc = None;
                for i in 0..16 {
                    if list & (1 << i) != 0 {
                        let v = mem.r32(a);
                        if i == 15 {
                            newpc = Some(v);
                        } else {
                            self.r[i] = v;
                        }
                        a = a.wrapping_add(4);
                    }
                }
                if let Some(v) = newpc {
                    return Some(self.bx_write(v));
                }
            }
            Inst::Stm { rn, list, wb, db } => {
                let cnt = list.count_ones();
                let base = self.r[rn as usize];
                let mut a = if db { base.wrapping_sub(cnt * 4) } else { base };
                for i in 0..16 {
                    if list & (1 << i) != 0 {
                        mem.w32(a, self.r[i]);
                        a = a.wrapping_add(4);
                    }
                }
                if wb {
                    self.r[rn as usize] = if db { base.wrapping_sub(cnt * 4) } else { base.wrapping_add(cnt * 4) };
                }
            }
            Inst::Ldrex { sz, rt, rn, off } => {
                let a = self.r[rn as usize].wrapping_add(off);
                self.excl = a;
                self.r[rt as usize] = Self::load(mem, sz, a);
            }
            Inst::Strex { sz, rd, rt, rn, off } => {
                let a = self.r[rn as usize].wrapping_add(off);
                if self.excl == a {
                    Self::store(mem, sz, a, self.r[rt as usize]);
                    self.r[rd as usize] = 0;
                } else {
                    self.r[rd as usize] = 1;
                }
                self.excl = NO_EXCL;
            }
            Inst::Ldrexd { rt, rt2, rn } => {
                let a = self.r[rn as usize];
                self.excl = a;
                self.r[rt as usize] = mem.r32(a);
                self.r[rt2 as usize] = mem.r32(a + 4);
            }
            Inst::Strexd { rd, rt, rt2, rn } => {
                let a = self.r[rn as usize];
                if self.excl == a {
                    mem.w32(a, self.r[rt as usize]);
                    mem.w32(a + 4, self.r[rt2 as usize]);
                    self.r[rd as usize] = 0;
                } else {
                    self.r[rd as usize] = 1;
                }
                self.excl = NO_EXCL;
            }
            Inst::Clrex => self.excl = NO_EXCL,
            Inst::B { target } => return Some(target),
            Inst::Bl { target } => {
                self.r[14] = if arm { d.pc + 4 } else { (d.pc + 4) | 1 };
                return Some(target);
            }
            Inst::BlxImm { target } => {
                self.r[14] = if arm { d.pc + 4 } else { (d.pc + 4) | 1 };
                self.thumb = arm;
                return Some(target & !1);
            }
            Inst::Bx { rm } => {
                let v = self.r[rm as usize];
                return Some(self.bx_write(v));
            }
            Inst::BlxReg { rm } => {
                let v = self.r[rm as usize];
                self.r[14] = if arm { d.pc + 4 } else { (d.pc + 2) | 1 };
                return Some(self.bx_write(v));
            }
            Inst::Cbz { rn, target, nz } => {
                if (self.r[rn as usize] != 0) == nz {
                    return Some(target);
                }
                return Some(d.pc + 2);
            }
            Inst::Tb { rn, rm, half } => {
                let base = self.r[rn as usize];
                let idx = self.r[rm as usize];
                let off = if half { mem.r16(base.wrapping_add(idx * 2)) as u32 } else { mem.r8(base.wrapping_add(idx)) as u32 };
                return Some(d.pc.wrapping_add(4).wrapping_add(off * 2));
            }
            Inst::Svc(n) => {
                self.fault = Some(format!("svc #{}", n));
                return Some(d.pc);
            }
            Inst::Bkpt(n) => {
                self.fault = Some(format!("bkpt #{}", n));
                return Some(d.pc);
            }
            Inst::Udf(n) => {
                self.fault = Some(format!("udf #{}", n));
                return Some(d.pc);
            }
            Inst::Invalid(w) => {
                self.fault = Some(format!("invalid instruction {:08x}", w));
                return Some(d.pc);
            }
            Inst::Mrs { rd } => self.r[rd as usize] = self.cpsr() & 0xf800_0000,
            Inst::Msr { rn } => {
                let v = self.r[rn as usize];
                self.set_nzcv(v);
                self.q = v & (1 << 27) != 0;
            }
            // ---------------- VFP ----------------
            Inst::VMem { load, dbl, vd, a } => {
                let (addr, _) = self.addr(&a);
                if load {
                    if dbl {
                        let v = mem.r64(addr);
                        self.set_d_bits(vd, v);
                    } else {
                        self.s[vd as usize] = mem.r32(addr);
                    }
                } else if dbl {
                    mem.w64(addr, self.d_bits(vd));
                } else {
                    mem.w32(addr, self.s[vd as usize]);
                }
            }
            Inst::VMulti { load, dbl, vd, n, rn, wb, db } => {
                let sz = if dbl { 8 } else { 4 };
                let total = n as u32 * sz;
                let base = self.r[rn as usize];
                let mut a = if db { base.wrapping_sub(total) } else { base };
                if wb {
                    self.r[rn as usize] = if db { base.wrapping_sub(total) } else { base.wrapping_add(total) };
                }
                for i in 0..n {
                    let reg = vd.wrapping_add(i);
                    if dbl {
                        if load {
                            let v = mem.r64(a);
                            self.set_d_bits(reg & 31, v);
                        } else {
                            mem.w64(a, self.d_bits(reg & 31));
                        }
                    } else if load {
                        self.s[(reg & 63) as usize] = mem.r32(a);
                    } else {
                        mem.w32(a, self.s[(reg & 63) as usize]);
                    }
                    a = a.wrapping_add(sz);
                }
            }
            Inst::VmovS { to_core, rt, sn } => {
                if to_core {
                    self.r[rt as usize] = self.s[sn as usize];
                } else {
                    self.s[sn as usize] = self.r[rt as usize];
                }
            }
            Inst::Vmov2D { to_core, rt, rt2, dm } => {
                if to_core {
                    let v = self.d_bits(dm);
                    self.r[rt as usize] = v as u32;
                    self.r[rt2 as usize] = (v >> 32) as u32;
                } else {
                    let v = self.r[rt as usize] as u64 | (self.r[rt2 as usize] as u64) << 32;
                    self.set_d_bits(dm, v);
                }
            }
            Inst::Vmov2S { to_core, rt, rt2, sm } => {
                if to_core {
                    self.r[rt as usize] = self.s[sm as usize];
                    self.r[rt2 as usize] = self.s[(sm as usize + 1) & 63];
                } else {
                    self.s[sm as usize] = self.r[rt as usize];
                    self.s[(sm as usize + 1) & 63] = self.r[rt2 as usize];
                }
            }
            Inst::VmovScalar { to_core, rt, dd, idx } => {
                let si = dd as usize * 2 + idx as usize;
                if to_core {
                    self.r[rt as usize] = self.s[si];
                } else {
                    self.s[si] = self.r[rt as usize];
                }
            }
            Inst::VmovImm { dbl, vd, bits } => {
                if dbl {
                    self.set_d_bits(vd, bits);
                } else {
                    self.s[vd as usize] = bits as u32;
                }
            }
            Inst::V2 { op, dbl, vd, vn, vm } => {
                if dbl {
                    let a = self.fd(vn);
                    let b = self.fd(vm);
                    let dv = self.fd(vd);
                    let r = match op {
                        VOp::Add => a + b,
                        VOp::Sub => a - b,
                        VOp::Mul => a * b,
                        VOp::Nmul => -(a * b),
                        VOp::Div => a / b,
                        VOp::Mla => dv + a * b,
                        VOp::Mls => dv - a * b,
                        VOp::Nmla => -dv - a * b,
                        VOp::Nmls => -dv + a * b,
                    };
                    self.set_fd(vd, r);
                } else {
                    let a = self.fs(vn);
                    let b = self.fs(vm);
                    let dv = self.fs(vd);
                    let r = match op {
                        VOp::Add => a + b,
                        VOp::Sub => a - b,
                        VOp::Mul => a * b,
                        VOp::Nmul => -(a * b),
                        VOp::Div => a / b,
                        VOp::Mla => dv + a * b,
                        VOp::Mls => dv - a * b,
                        VOp::Nmla => -dv - a * b,
                        VOp::Nmls => -dv + a * b,
                    };
                    self.set_fs(vd, r);
                }
            }
            Inst::V1 { op, dbl, vd, vm } => {
                if dbl {
                    let b = self.d_bits(vm);
                    let r = match op {
                        VUn::Mov => b,
                        VUn::Abs => b & !(1u64 << 63),
                        VUn::Neg => b ^ (1u64 << 63),
                        VUn::Sqrt => f64::from_bits(b).sqrt().to_bits(),
                    };
                    self.set_d_bits(vd, r);
                } else {
                    let b = self.s[vm as usize];
                    let r = match op {
                        VUn::Mov => b,
                        VUn::Abs => b & !(1u32 << 31),
                        VUn::Neg => b ^ (1u32 << 31),
                        VUn::Sqrt => f32::from_bits(b).sqrt().to_bits(),
                    };
                    self.s[vd as usize] = r;
                }
            }
            Inst::Vcmp { dbl, vd, vm } => {
                let (a, b) = if dbl {
                    (self.fd(vd), if vm == 0xff { 0.0 } else { self.fd(vm) })
                } else {
                    (self.fs(vd) as f64, if vm == 0xff { 0.0 } else { self.fs(vm) as f64 })
                };
                let nzcv: u32 = if a.is_nan() || b.is_nan() {
                    0b0011
                } else if a == b {
                    0b0110
                } else if a < b {
                    0b1000
                } else {
                    0b0010
                };
                self.fpscr = (self.fpscr & 0x0fff_ffff) | nzcv << 28;
            }
            Inst::VcvtFF { to_dbl, vd, vm } => {
                if to_dbl {
                    let v = self.fs(vm) as f64;
                    self.set_fd(vd, v);
                } else {
                    let v = self.fd(vm) as f32;
                    self.set_fs(vd, v);
                }
            }
            Inst::VcvtIF { dbl, signed, vd, vm } => {
                let i = self.s[vm as usize];
                let f = if signed { i as i32 as f64 } else { i as f64 };
                if dbl {
                    self.set_fd(vd, f);
                } else {
                    let f32v = if signed { i as i32 as f32 } else { i as f32 };
                    self.set_fs(vd, f32v);
                }
            }
            Inst::VcvtFI { dbl, signed, rz, vd, vm } => {
                let mut f = if dbl { self.fd(vm) } else { self.fs(vm) as f64 };
                if !rz {
                    f = f.round_ties_even();
                }
                self.s[vd as usize] = if signed { f as i32 as u32 } else { f as u32 };
            }
            Inst::VcvtFix { dbl, signed, half, to_fixed, vd, fbits } => {
                let scale = (2.0f64).powi(fbits as i32);
                if to_fixed {
                    let f = if dbl { self.fd(vd) } else { self.fs(vd) as f64 } * scale;
                    let v: u32 = match (signed, half) {
                        (true, true) => (f as i16) as i32 as u32,
                        (true, false) => f as i32 as u32,
                        (false, true) => (f as u16) as u32,
                        (false, false) => f as u32,
                    };
                    if dbl {
                        let ext = if signed { v as i32 as i64 as u64 } else { v as u64 };
                        self.set_d_bits(vd, ext);
                    } else {
                        self.s[vd as usize] = v;
                    }
                } else {
                    let raw = if dbl { self.d_bits(vd) as u32 } else { self.s[vd as usize] };
                    let i: f64 = match (signed, half) {
                        (true, true) => raw as u16 as i16 as f64,
                        (true, false) => raw as i32 as f64,
                        (false, true) => (raw & 0xffff) as f64,
                        (false, false) => raw as f64,
                    };
                    let f = i / scale;
                    if dbl {
                        self.set_fd(vd, f);
                    } else {
                        self.set_fs(vd, f as f32);
                    }
                }
            }
            Inst::Vmrs { rt } => {
                if rt & 0x80 != 0 {
                    self.r[(rt & 0xf) as usize] = 0;
                } else if rt == 15 {
                    let f = self.fpscr;
                    self.set_nzcv(f);
                } else {
                    self.r[rt as usize] = self.fpscr;
                }
            }
            Inst::Vmsr { rt } => self.fpscr = self.r[rt as usize],
        }
        None
    }

    /// Interworking branch: bit 0 selects Thumb.
    #[inline(always)]
    fn bx_write(&mut self, v: u32) -> u32 {
        self.thumb = v & 1 != 0;
        if self.thumb {
            v & !1
        } else {
            v & !3
        }
    }
}

/// Decoded-block cache keyed by (pc, mode).
pub struct BlockCache {
    blocks: Vec<Block>,
    text_start: u32,
    text_end: u32,
    direct: Vec<u32>,
    other: HashMap<u32, u32>,
    pub max_len: usize,
}

impl BlockCache {
    pub fn new(text_start: u32, text_end: u32) -> BlockCache {
        let n = ((text_end - text_start) / 2) as usize + 1;
        BlockCache { blocks: Vec::new(), text_start, text_end, direct: vec![u32::MAX; n], other: HashMap::new(), max_len: 64 }
    }

    pub fn invalidate(&mut self) {
        self.blocks.clear();
        for d in self.direct.iter_mut() {
            *d = u32::MAX;
        }
        self.other.clear();
    }

    #[inline(always)]
    pub fn get(&mut self, mem: &Memory, pc: u32, thumb: bool) -> &Block {
        let idx = if thumb && pc >= self.text_start && pc < self.text_end {
            let slot = ((pc - self.text_start) >> 1) as usize;
            let b = self.direct[slot];
            if b != u32::MAX {
                b
            } else {
                let b = self.decode(mem, pc, thumb);
                self.direct[slot] = b;
                b
            }
        } else {
            let key = pc | thumb as u32;
            match self.other.get(&key) {
                Some(&b) => b,
                None => {
                    let b = self.decode(mem, pc, thumb);
                    self.other.insert(key, b);
                    b
                }
            }
        };
        &self.blocks[idx as usize]
    }

    fn decode(&mut self, mem: &Memory, pc: u32, thumb: bool) -> u32 {
        let blk = if thumb {
            thumb::decode_block(&|a| mem.r16(a), pc, self.max_len)
        } else {
            arm::decode_block(&|a| mem.r32(a), pc, self.max_len)
        };
        self.blocks.push(blk);
        (self.blocks.len() - 1) as u32
    }
}
