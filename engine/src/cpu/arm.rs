//! Minimal ARM (A32) decoder. The game library only contains ARM code in its
//! PLT, so this covers data-processing, single loads/stores, LDM/STM and
//! branches — enough for PLT stubs and simple veneers.

use super::inst::*;
use super::thumb::ends_block;

fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}

fn imm_shift(ty: u32, imm5: u32) -> (Sh, u8) {
    match ty {
        0 => (Sh::Lsl, imm5 as u8),
        1 => (Sh::Lsr, if imm5 == 0 { 32 } else { imm5 as u8 }),
        2 => (Sh::Asr, if imm5 == 0 { 32 } else { imm5 as u8 }),
        _ => {
            if imm5 == 0 {
                (Sh::Rrx, 1)
            } else {
                (Sh::Ror, imm5 as u8)
            }
        }
    }
}

pub fn decode(w: u32, pc: u32) -> Inst {
    use Inst::*;
    let rn = bits(w, 19, 16) as u8;
    let rd = bits(w, 15, 12) as u8;
    if w & 0x0fff_fff0 == 0x012f_ff10 {
        return Bx { rm: (w & 0xf) as u8 };
    }
    if w & 0x0fff_fff0 == 0x012f_ff30 {
        return BlxReg { rm: (w & 0xf) as u8 };
    }
    match bits(w, 27, 25) {
        0b000 | 0b001 => {
            let imm = bits(w, 25, 25) != 0;
            if !imm && (w & 0x90) == 0x90 {
                return Invalid(w);
            }
            let opc = bits(w, 24, 21);
            let s = bits(w, 20, 20) != 0;
            let op2 = if imm {
                let rot = bits(w, 11, 8) * 2;
                let v = (w & 0xff).rotate_right(rot);
                if rot == 0 {
                    Op2::Imm(v)
                } else {
                    Op2::ImmC(v, v >> 31 != 0)
                }
            } else if bits(w, 4, 4) == 0 {
                let (sh, amt) = imm_shift(bits(w, 6, 5), bits(w, 11, 7));
                if sh == Sh::Lsl && amt == 0 {
                    Op2::Reg((w & 0xf) as u8)
                } else {
                    Op2::Shift((w & 0xf) as u8, sh, amt)
                }
            } else {
                let sh = match bits(w, 6, 5) {
                    0 => Sh::Lsl,
                    1 => Sh::Lsr,
                    2 => Sh::Asr,
                    _ => Sh::Ror,
                };
                Op2::ShiftReg((w & 0xf) as u8, sh, bits(w, 11, 8) as u8)
            };
            let op = match opc {
                0 => self::Alu::And,
                1 => self::Alu::Eor,
                2 => self::Alu::Sub,
                3 => self::Alu::Rsb,
                4 => self::Alu::Add,
                5 => self::Alu::Adc,
                6 => self::Alu::Sbc,
                7 => self::Alu::Rsc,
                8 => self::Alu::Tst,
                9 => self::Alu::Teq,
                10 => self::Alu::Cmp,
                11 => self::Alu::Cmn,
                12 => self::Alu::Orr,
                13 => self::Alu::Mov,
                14 => self::Alu::Bic,
                _ => self::Alu::Mvn,
            };
            if matches!(op, self::Alu::Tst | self::Alu::Teq | self::Alu::Cmp | self::Alu::Cmn) && !s {
                return Invalid(w);
            }
            let rn = if rn == 15 && !matches!(op, self::Alu::Mov | self::Alu::Mvn) {
                // PC-relative: resolve operand at execution via r15 (set to pc+8)
                15
            } else {
                rn
            };
            let _ = pc;
            Alu { op, s, rd, rn, op2 }
        }
        0b010 | 0b011 => {
            let reg = bits(w, 25, 25) != 0;
            if reg && bits(w, 4, 4) != 0 {
                return Invalid(w);
            }
            let p = bits(w, 24, 24) != 0;
            let u = bits(w, 23, 23) != 0;
            let b = bits(w, 22, 22) != 0;
            let wb = bits(w, 21, 21) != 0 || !p;
            let l = bits(w, 20, 20) != 0;
            let a = if reg {
                let (sh, amt) = imm_shift(bits(w, 6, 5), bits(w, 11, 7));
                if sh != Sh::Lsl {
                    return Invalid(w);
                }
                Addr::Reg { rn, rm: (w & 0xf) as u8, sh: amt, add: u, pre: p, wb }
            } else {
                let imm = (w & 0xfff) as i32;
                Addr::Imm { rn, off: if u { imm } else { -imm }, pre: p, wb }
            };
            let sz = if b { Sz::B } else { Sz::W };
            if l {
                Ld { sz, rt: rd, a }
            } else {
                St { sz, rt: rd, a }
            }
        }
        0b100 => {
            let p = bits(w, 24, 24) != 0;
            let u = bits(w, 23, 23) != 0;
            let wb = bits(w, 21, 21) != 0;
            let l = bits(w, 20, 20) != 0;
            let list = (w & 0xffff) as u16;
            let db = match (p, u) {
                (false, true) => false,
                (true, false) => true,
                _ => return Invalid(w),
            };
            if l {
                Ldm { rn, list, wb, db }
            } else {
                Stm { rn, list, wb, db }
            }
        }
        0b101 => {
            let off = (((w & 0xff_ffff) << 8) as i32 >> 6) as u32;
            let target = pc.wrapping_add(8).wrapping_add(off);
            if bits(w, 31, 28) == 0xf {
                // BLX imm (to Thumb)
                let h = bits(w, 24, 24) << 1;
                return BlxImm { target: target.wrapping_add(h) | 1 };
            }
            if bits(w, 24, 24) != 0 {
                Bl { target }
            } else {
                B { target }
            }
        }
        _ => Invalid(w),
    }
}

pub fn decode_block(fetch: &dyn Fn(u32) -> u32, start: u32, max: usize) -> Block {
    let mut insts = Vec::new();
    let mut pc = start;
    loop {
        let w = fetch(pc);
        let cond = (w >> 28) as u8;
        let inst = decode(w, pc);
        let cond = if cond == 15 { COND_AL } else { cond };
        insts.push(Decoded { inst, cond, pc });
        pc += 4;
        if ends_block(&inst) || insts.len() >= max {
            break;
        }
    }
    Block { insts, end: pc, arm: true }
}
