//! Thumb / Thumb-2 (+VFPv3) decoder.

use super::inst::*;

#[inline]
fn bit(v: u32, n: u32) -> bool {
    (v >> n) & 1 != 0
}
#[inline]
fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}
#[inline]
fn sext(v: u32, nbits: u32) -> i32 {
    let s = 32 - nbits;
    ((v << s) as i32) >> s
}

pub fn thumb_expand_imm(imm12: u32) -> (u32, Option<bool>) {
    if imm12 >> 10 == 0 {
        let b = imm12 & 0xff;
        let v = match (imm12 >> 8) & 3 {
            0 => b,
            1 => b << 16 | b,
            2 => b << 24 | b << 8,
            _ => b << 24 | b << 16 | b << 8 | b,
        };
        (v, None)
    } else {
        let unrot = 0x80 | (imm12 & 0x7f);
        let v = unrot.rotate_right(imm12 >> 7);
        (v, Some(v >> 31 != 0))
    }
}

fn decode_imm_shift(ty: u32, imm5: u32) -> (Sh, u8) {
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

fn shifted(rm: u8, sh: Sh, amt: u8) -> Op2 {
    if sh == Sh::Lsl && amt == 0 {
        Op2::Reg(rm)
    } else {
        Op2::Shift(rm, sh, amt)
    }
}

pub fn vfp_expand_imm(imm8: u32, dbl: bool) -> u64 {
    let sign = (imm8 >> 7) as u64;
    let b6 = ((imm8 >> 6) & 1) as u64;
    let low6 = (imm8 & 0x3f) as u64;
    if dbl {
        // sign : NOT(b6) : Replicate(b6, 8) : imm8<5:0> : Zeros(48)
        let exp = ((b6 ^ 1) << 10) | (if b6 != 0 { 0xff << 2 } else { 0 }) | (low6 >> 4);
        (sign << 63) | (exp << 52) | ((low6 & 0xf) << 48)
    } else {
        let exp = ((b6 ^ 1) << 7) | (if b6 != 0 { 0x1f << 2 } else { 0 }) | (low6 >> 4);
        (sign << 31) | (exp << 23) | ((low6 & 0xf) << 19)
    }
}

/// Does this instruction (potentially) write the PC / end a basic block?
pub fn ends_block(i: &Inst) -> bool {
    match *i {
        Inst::B { .. }
        | Inst::Bl { .. }
        | Inst::BlxImm { .. }
        | Inst::Bx { .. }
        | Inst::BlxReg { .. }
        | Inst::Cbz { .. }
        | Inst::Tb { .. }
        | Inst::Svc(_)
        | Inst::Bkpt(_)
        | Inst::Udf(_)
        | Inst::Invalid(_) => true,
        Inst::Alu { rd, op, .. } => rd == 15 && !matches!(op, Alu::Tst | Alu::Teq | Alu::Cmp | Alu::Cmn),
        Inst::Ld { rt, .. } => rt == 15,
        Inst::Ldm { list, .. } => list & 0x8000 != 0,
        Inst::Mul { rd, .. } | Inst::Movt { rd, .. } => rd == 15,
        _ => false,
    }
}

/// Decode one Thumb instruction at `pc`. Returns (inst, size, it_mask_if_IT).
/// `in_it` affects flag-setting of 16-bit ALU ops.
pub fn decode(hw1: u16, hw2: u16, pc: u32, in_it: bool) -> (Inst, u32, Option<(u8, u8)>) {
    let h = hw1 as u32;
    if (h >> 11) >= 0b11101 {
        (decode32(h, hw2 as u32, pc), 4, None)
    } else {
        let (i, it) = decode16(h, pc, in_it);
        (i, 2, it)
    }
}

fn decode16(h: u32, pc: u32, in_it: bool) -> (Inst, Option<(u8, u8)>) {
    let s = !in_it;
    let rd = (h & 7) as u8;
    let rn3 = ((h >> 3) & 7) as u8;
    use Inst::*;
    let i = match h >> 10 {
        0b000000..=0b001111 => {
            let op = bits(h, 13, 11);
            match op {
                0 | 1 | 2 => {
                    let imm5 = bits(h, 10, 6);
                    let rm = rn3;
                    if op == 0 && imm5 == 0 {
                        Alu { op: self::Alu::Mov, s, rd, rn: 0, op2: Op2::Reg(rm) }
                    } else {
                        let (sh, amt) = decode_imm_shift(op, imm5);
                        Alu { op: self::Alu::Mov, s, rd, rn: 0, op2: Op2::Shift(rm, sh, amt) }
                    }
                }
                3 => {
                    let rn = rn3;
                    let x = bits(h, 8, 6);
                    match bits(h, 10, 9) {
                        0 => Alu { op: self::Alu::Add, s, rd, rn, op2: Op2::Reg(x as u8) },
                        1 => Alu { op: self::Alu::Sub, s, rd, rn, op2: Op2::Reg(x as u8) },
                        2 => Alu { op: self::Alu::Add, s, rd, rn, op2: Op2::Imm(x) },
                        _ => Alu { op: self::Alu::Sub, s, rd, rn, op2: Op2::Imm(x) },
                    }
                }
                _ => {
                    let r = bits(h, 10, 8) as u8;
                    let imm = h & 0xff;
                    match op {
                        4 => Alu { op: self::Alu::Mov, s, rd: r, rn: 0, op2: Op2::Imm(imm) },
                        5 => Alu { op: self::Alu::Cmp, s: true, rd: 0, rn: r, op2: Op2::Imm(imm) },
                        6 => Alu { op: self::Alu::Add, s, rd: r, rn: r, op2: Op2::Imm(imm) },
                        _ => Alu { op: self::Alu::Sub, s, rd: r, rn: r, op2: Op2::Imm(imm) },
                    }
                }
            }
        }
        0b010000 => {
            let rm = rn3;
            let rdn = rd;
            use self::Alu as A;
            match bits(h, 9, 6) {
                0 => Alu { op: A::And, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                1 => Alu { op: A::Eor, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                2 => Alu { op: A::Mov, s, rd: rdn, rn: 0, op2: Op2::ShiftReg(rdn, Sh::Lsl, rm) },
                3 => Alu { op: A::Mov, s, rd: rdn, rn: 0, op2: Op2::ShiftReg(rdn, Sh::Lsr, rm) },
                4 => Alu { op: A::Mov, s, rd: rdn, rn: 0, op2: Op2::ShiftReg(rdn, Sh::Asr, rm) },
                5 => Alu { op: A::Adc, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                6 => Alu { op: A::Sbc, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                7 => Alu { op: A::Mov, s, rd: rdn, rn: 0, op2: Op2::ShiftReg(rdn, Sh::Ror, rm) },
                8 => Alu { op: A::Tst, s: true, rd: 0, rn: rdn, op2: Op2::Reg(rm) },
                9 => Alu { op: A::Rsb, s, rd: rdn, rn: rm, op2: Op2::Imm(0) },
                10 => Alu { op: A::Cmp, s: true, rd: 0, rn: rdn, op2: Op2::Reg(rm) },
                11 => Alu { op: A::Cmn, s: true, rd: 0, rn: rdn, op2: Op2::Reg(rm) },
                12 => Alu { op: A::Orr, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                13 => Mul { s, rd: rdn, rn: rm, rm: rdn },
                14 => Alu { op: A::Bic, s, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                _ => Alu { op: A::Mvn, s, rd: rdn, rn: 0, op2: Op2::Reg(rm) },
            }
        }
        0b010001 => {
            let rdn = ((h >> 4) & 8 | (h & 7)) as u8;
            let rm = bits(h, 6, 3) as u8;
            match bits(h, 9, 8) {
                0 => Alu { op: self::Alu::Add, s: false, rd: rdn, rn: rdn, op2: Op2::Reg(rm) },
                1 => Alu { op: self::Alu::Cmp, s: true, rd: 0, rn: rdn, op2: Op2::Reg(rm) },
                2 => Alu { op: self::Alu::Mov, s: false, rd: rdn, rn: 0, op2: Op2::Reg(rm) },
                _ => {
                    if bit(h, 7) {
                        BlxReg { rm }
                    } else {
                        Bx { rm }
                    }
                }
            }
        }
        0b010010 | 0b010011 => {
            let rt = bits(h, 10, 8) as u8;
            let addr = (pc + 4) & !3;
            Ld { sz: Sz::W, rt, a: Addr::Lit(addr + (h & 0xff) * 4) }
        }
        0b010100..=0b010111 => {
            let rm = bits(h, 8, 6) as u8;
            let a = Addr::Reg { rn: rn3, rm, sh: 0, add: true, pre: true, wb: false };
            match bits(h, 11, 9) {
                0 => St { sz: Sz::W, rt: rd, a },
                1 => St { sz: Sz::H, rt: rd, a },
                2 => St { sz: Sz::B, rt: rd, a },
                3 => Ld { sz: Sz::Sb, rt: rd, a },
                4 => Ld { sz: Sz::W, rt: rd, a },
                5 => Ld { sz: Sz::H, rt: rd, a },
                6 => Ld { sz: Sz::B, rt: rd, a },
                _ => Ld { sz: Sz::Sh, rt: rd, a },
            }
        }
        0b011000..=0b100011 => {
            let imm5 = bits(h, 10, 6) as i32;
            let load = bit(h, 11);
            let (sz, scale) = match h >> 12 {
                0b0110 => (Sz::W, 4),
                0b0111 => (Sz::B, 1),
                _ => (Sz::H, 2),
            };
            let a = Addr::Imm { rn: rn3, off: imm5 * scale, pre: true, wb: false };
            if load {
                Ld { sz, rt: rd, a }
            } else {
                St { sz, rt: rd, a }
            }
        }
        0b100100..=0b100111 => {
            let rt = bits(h, 10, 8) as u8;
            let a = Addr::Imm { rn: 13, off: (h & 0xff) as i32 * 4, pre: true, wb: false };
            if bit(h, 11) {
                Ld { sz: Sz::W, rt, a }
            } else {
                St { sz: Sz::W, rt, a }
            }
        }
        0b101000 | 0b101001 => {
            let r = bits(h, 10, 8) as u8;
            Alu { op: self::Alu::Mov, s: false, rd: r, rn: 0, op2: Op2::Imm(((pc + 4) & !3) + (h & 0xff) * 4) }
        }
        0b101010 | 0b101011 => {
            let r = bits(h, 10, 8) as u8;
            Alu { op: self::Alu::Add, s: false, rd: r, rn: 13, op2: Op2::Imm((h & 0xff) * 4) }
        }
        0b101100..=0b101111 => return decode_misc16(h, pc),
        0b110000 | 0b110001 => {
            let rn = bits(h, 10, 8) as u8;
            Stm { rn, list: (h & 0xff) as u16, wb: true, db: false }
        }
        0b110010 | 0b110011 => {
            let rn = bits(h, 10, 8) as u8;
            let list = (h & 0xff) as u16;
            Ldm { rn, list, wb: list & (1 << rn) == 0, db: false }
        }
        0b110100..=0b110111 => {
            let cond = bits(h, 11, 8);
            match cond {
                14 => Udf(h & 0xff),
                15 => Svc(h & 0xff),
                _ => {
                    let target = (pc + 4).wrapping_add((sext(h & 0xff, 8) * 2) as u32);
                    // Conditional branch: encode condition via the IT mechanism.
                    return (B { target }, Some((cond as u8, 0xff)));
                }
            }
        }
        0b111000 | 0b111001 => {
            let target = (pc + 4).wrapping_add((sext(h & 0x7ff, 11) * 2) as u32);
            B { target }
        }
        _ => Invalid(h),
    };
    (i, None)
}

fn decode_misc16(h: u32, pc: u32) -> (Inst, Option<(u8, u8)>) {
    use Inst::*;
    let rd = (h & 7) as u8;
    let rm = bits(h, 5, 3) as u8;
    let i = match bits(h, 11, 5) {
        0b0000000..=0b0000011 => Alu { op: self::Alu::Add, s: false, rd: 13, rn: 13, op2: Op2::Imm((h & 0x7f) * 4) },
        0b0000100..=0b0000111 => Alu { op: self::Alu::Sub, s: false, rd: 13, rn: 13, op2: Op2::Imm((h & 0x7f) * 4) },
        0b0010000..=0b0010111 => {
            let k = match bits(h, 7, 6) {
                0 => ExtK::Sxth,
                1 => ExtK::Sxtb,
                2 => ExtK::Uxth,
                _ => ExtK::Uxtb,
            };
            Ext { k, rd, rn: 15, rm, rot: 0 }
        }
        0b0001000..=0b0001111 | 0b0011000..=0b0011111 | 0b1001000..=0b1001111 | 0b1011000..=0b1011111 => {
            let nz = bit(h, 11);
            let imm = (bits(h, 9, 9) << 6) | (bits(h, 7, 3) << 1);
            Cbz { rn: rd, target: pc + 4 + imm, nz }
        }
        0b0100000..=0b0101111 => {
            let list = (h & 0xff) as u16 | if bit(h, 8) { 1 << 14 } else { 0 };
            Stm { rn: 13, list, wb: true, db: true }
        }
        0b0110011 => Nop, // CPS
        0b1010000..=0b1010001 => Rev { rd, rm },
        0b1010010..=0b1010011 => Rev16 { rd, rm },
        0b1010110..=0b1010111 => Revsh { rd, rm },
        0b1100000..=0b1101111 => {
            let list = (h & 0xff) as u16 | if bit(h, 8) { 1 << 15 } else { 0 };
            Ldm { rn: 13, list, wb: true, db: false }
        }
        0b1110000..=0b1110111 => Bkpt(h & 0xff),
        0b1111000..=0b1111111 => {
            let mask = h & 0xf;
            if mask != 0 {
                let firstcond = bits(h, 7, 4) as u8;
                return (Nop, Some((firstcond, mask as u8)));
            }
            Nop
        }
        _ => Invalid(h),
    };
    (i, None)
}

fn decode32(h1: u32, h2: u32, pc: u32) -> Inst {
    use Inst::*;
    let op1 = bits(h1, 12, 11);
    let op2 = bits(h1, 10, 4);
    let rn = (h1 & 0xf) as u8;
    match op1 {
        1 => {
            if op2 & 0b1100100 == 0 {
                // load/store multiple
                let l = bit(h1, 4);
                let wb = bit(h1, 5);
                let list = h2 as u16;
                match bits(h1, 8, 7) {
                    1 => {
                        if l {
                            Ldm { rn, list, wb, db: false }
                        } else {
                            Stm { rn, list, wb, db: false }
                        }
                    }
                    2 => {
                        if l {
                            Ldm { rn, list, wb, db: true }
                        } else {
                            Stm { rn, list, wb, db: true }
                        }
                    }
                    _ => Invalid(h1 << 16 | h2),
                }
            } else if op2 & 0b1100100 == 0b0000100 {
                decode_dual_excl(h1, h2, pc)
            } else if op2 & 0b1100000 == 0b0100000 {
                decode_dp_shifted(h1, h2)
            } else {
                decode_vfp((0xe << 28) | ((h1 & 0x0fff) << 16) | h2, pc)
            }
        }
        2 => {
            if !bit(h2, 15) {
                if !bit(op2, 5) {
                    decode_dp_modimm(h1, h2)
                } else {
                    decode_dp_plainimm(h1, h2, pc)
                }
            } else {
                decode_branch_misc(h1, h2, pc)
            }
        }
        _ => {
            if op2 & 0b1110001 == 0b0000000 {
                decode_ldst_single(h1, h2, pc, false)
            } else if op2 & 0b1100111 == 0b0000001 || op2 & 0b1100111 == 0b0000011 || op2 & 0b1100111 == 0b0000101 {
                decode_ldst_single(h1, h2, pc, true)
            } else if op2 & 0b1110000 == 0b0100000 {
                decode_dp_reg(h1, h2)
            } else if op2 & 0b1111000 == 0b0110000 {
                decode_mul(h1, h2)
            } else if op2 & 0b1111000 == 0b0111000 {
                decode_mull(h1, h2)
            } else if op2 & 0b1000000 != 0 {
                decode_vfp((0xe << 28) | ((h1 & 0x0fff) << 16) | h2, pc)
            } else {
                Invalid(h1 << 16 | h2)
            }
        }
    }
}

fn decode_dual_excl(h1: u32, h2: u32, pc: u32) -> Inst {
    use Inst::*;
    let rn = (h1 & 0xf) as u8;
    let rt = bits(h2, 15, 12) as u8;
    let rt2 = bits(h2, 11, 8) as u8;
    let o1 = bits(h1, 8, 7);
    let o2 = bits(h1, 5, 4);
    let o3 = bits(h2, 7, 4);
    if o1 == 0 && o2 == 0 {
        return Strex { sz: Sz::W, rd: rt2, rt, rn, off: (h2 & 0xff) * 4 };
    }
    if o1 == 0 && o2 == 1 {
        return Ldrex { sz: Sz::W, rt, rn, off: (h2 & 0xff) * 4 };
    }
    if o1 & 2 != 0 || o2 & 2 != 0 {
        let p = bit(h1, 8);
        let u = bit(h1, 7);
        let w = bit(h1, 5);
        let l = bit(h1, 4);
        let imm = ((h2 & 0xff) * 4) as i32;
        let off = if u { imm } else { -imm };
        let a = if rn == 15 {
            Addr::Lit(((pc + 4) & !3).wrapping_add(off as u32))
        } else {
            Addr::Imm { rn, off, pre: p, wb: w }
        };
        return if l { Ldrd { rt, rt2, a } } else { Strd { rt, rt2, a } };
    }
    let rd = (h2 & 0xf) as u8;
    if o1 == 1 && o2 == 0 {
        return match o3 {
            4 => Strex { sz: Sz::B, rd, rt, rn, off: 0 },
            5 => Strex { sz: Sz::H, rd, rt, rn, off: 0 },
            7 => Strexd { rd, rt, rt2, rn },
            _ => Invalid(h1 << 16 | h2),
        };
    }
    if o1 == 1 && o2 == 1 {
        return match o3 {
            0 => Tb { rn, rm: rd, half: false },
            1 => Tb { rn, rm: rd, half: true },
            4 => Ldrex { sz: Sz::B, rt, rn, off: 0 },
            5 => Ldrex { sz: Sz::H, rt, rn, off: 0 },
            7 => Ldrexd { rt, rt2, rn },
            _ => Invalid(h1 << 16 | h2),
        };
    }
    Invalid(h1 << 16 | h2)
}

fn dp_op(op: u32, rd: u8, rn: u8, s: bool) -> Option<(Alu, u8, u8, bool)> {
    use self::Alu as A;
    Some(match op {
        0 => {
            if rd == 15 && s {
                (A::Tst, 0, rn, true)
            } else {
                (A::And, rd, rn, s)
            }
        }
        1 => (A::Bic, rd, rn, s),
        2 => {
            if rn == 15 {
                (A::Mov, rd, 0, s)
            } else {
                (A::Orr, rd, rn, s)
            }
        }
        3 => {
            if rn == 15 {
                (A::Mvn, rd, 0, s)
            } else {
                (A::Orn, rd, rn, s)
            }
        }
        4 => {
            if rd == 15 && s {
                (A::Teq, 0, rn, true)
            } else {
                (A::Eor, rd, rn, s)
            }
        }
        8 => {
            if rd == 15 && s {
                (A::Cmn, 0, rn, true)
            } else {
                (A::Add, rd, rn, s)
            }
        }
        10 => (A::Adc, rd, rn, s),
        11 => (A::Sbc, rd, rn, s),
        13 => {
            if rd == 15 && s {
                (A::Cmp, 0, rn, true)
            } else {
                (A::Sub, rd, rn, s)
            }
        }
        14 => (A::Rsb, rd, rn, s),
        _ => return None,
    })
}

fn decode_dp_shifted(h1: u32, h2: u32) -> Inst {
    let op = bits(h1, 8, 5);
    let s = bit(h1, 4);
    let rn = (h1 & 0xf) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = (h2 & 0xf) as u8;
    let imm5 = (bits(h2, 14, 12) << 2) | bits(h2, 7, 6);
    let (sh, amt) = decode_imm_shift(bits(h2, 5, 4), imm5);
    match dp_op(op, rd, rn, s) {
        Some((alu, rd, rn, s)) => Inst::Alu { op: alu, s, rd, rn, op2: shifted(rm, sh, amt) },
        None => Inst::Invalid(h1 << 16 | h2),
    }
}

fn decode_dp_modimm(h1: u32, h2: u32) -> Inst {
    let op = bits(h1, 8, 5);
    let s = bit(h1, 4);
    let rn = (h1 & 0xf) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let imm12 = (bits(h1, 10, 10) << 11) | (bits(h2, 14, 12) << 8) | (h2 & 0xff);
    let (v, c) = thumb_expand_imm(imm12);
    let op2 = match c {
        Some(c) => Op2::ImmC(v, c),
        None => Op2::Imm(v),
    };
    match dp_op(op, rd, rn, s) {
        Some((alu, rd, rn, s)) => Inst::Alu { op: alu, s, rd, rn, op2 },
        None => Inst::Invalid(h1 << 16 | h2),
    }
}

fn decode_dp_plainimm(h1: u32, h2: u32, pc: u32) -> Inst {
    use Inst::*;
    let op = bits(h1, 8, 4);
    let rn = (h1 & 0xf) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let imm12 = (bits(h1, 10, 10) << 11) | (bits(h2, 14, 12) << 8) | (h2 & 0xff);
    let lsb = ((bits(h2, 14, 12) << 2) | bits(h2, 7, 6)) as u8;
    let w5 = (h2 & 0x1f) as u8;
    match op {
        0b00000 => {
            if rn == 15 {
                Alu { op: self::Alu::Mov, s: false, rd, rn: 0, op2: Op2::Imm(((pc + 4) & !3) + imm12) }
            } else {
                Alu { op: self::Alu::Add, s: false, rd, rn, op2: Op2::Imm(imm12) }
            }
        }
        0b01010 => {
            if rn == 15 {
                Alu { op: self::Alu::Mov, s: false, rd, rn: 0, op2: Op2::Imm(((pc + 4) & !3).wrapping_sub(imm12)) }
            } else {
                Alu { op: self::Alu::Sub, s: false, rd, rn, op2: Op2::Imm(imm12) }
            }
        }
        0b00100 => {
            let imm16 = ((h1 & 0xf) << 12) | imm12;
            Alu { op: self::Alu::Mov, s: false, rd, rn: 0, op2: Op2::Imm(imm16) }
        }
        0b01100 => {
            let imm16 = ((h1 & 0xf) << 12) | imm12;
            Movt { rd, imm: imm16 as u16 }
        }
        0b10000 | 0b10010 | 0b11000 | 0b11010 => {
            let sh = if bit(h1, 5) { Sh::Asr } else { Sh::Lsl };
            if sh == Sh::Asr && lsb == 0 {
                return Invalid(h1 << 16 | h2); // SSAT16/USAT16
            }
            let unsigned = op & 0b01000 != 0;
            Ssat { rd, rn, sat: w5 + if unsigned { 0 } else { 1 }, sh, amt: lsb, unsigned }
        }
        0b10100 => Bfx { signed: true, rd, rn, lsb, width: w5 + 1 },
        0b11100 => Bfx { signed: false, rd, rn, lsb, width: w5 + 1 },
        0b10110 => {
            let msb = w5;
            if msb < lsb {
                return Invalid(h1 << 16 | h2);
            }
            Bfi { rd, rn, lsb, width: msb - lsb + 1 }
        }
        _ => Invalid(h1 << 16 | h2),
    }
}

fn decode_branch_misc(h1: u32, h2: u32, pc: u32) -> Inst {
    use Inst::*;
    let op1 = bits(h2, 14, 12);
    let s = bits(h1, 10, 10);
    let j1 = bits(h2, 13, 13);
    let j2 = bits(h2, 11, 11);
    if op1 & 0b101 == 0b000 {
        let cond = bits(h1, 9, 6);
        if cond & 0b1110 != 0b1110 {
            let imm = (s << 20) | (j2 << 19) | (j1 << 18) | (bits(h1, 5, 0) << 12) | ((h2 & 0x7ff) << 1);
            let target = (pc + 4).wrapping_add(sext(imm, 21) as u32);
            // Condition is attached by the caller (see decode_block); we return a marker.
            return Inst::Alu { op: self::Alu::Cmp, s: false, rd: 0xf0 | cond as u8, rn: 0, op2: Op2::Imm(target) };
        }
        // misc control
        let op = bits(h1, 10, 4);
        match op {
            0b0111000 | 0b0111001 => return Msr { rn: (h1 & 0xf) as u8 },
            0b0111010 => return Nop, // hints
            0b0111011 => {
                return match bits(h2, 7, 4) {
                    2 => Clrex,
                    _ => Nop, // DSB/DMB/ISB
                };
            }
            0b0111110 | 0b0111111 => return Mrs { rd: bits(h2, 11, 8) as u8 },
            _ => return Invalid(h1 << 16 | h2),
        }
    }
    let i1 = 1 ^ (j1 ^ s);
    let i2 = 1 ^ (j2 ^ s);
    let imm = (s << 24) | (i1 << 23) | (i2 << 22) | (bits(h1, 9, 0) << 12) | ((h2 & 0x7ff) << 1);
    let off = sext(imm, 25) as u32;
    match op1 {
        0b001 | 0b011 => B { target: (pc + 4).wrapping_add(off) },
        0b100 | 0b110 => BlxImm { target: ((pc + 4) & !3).wrapping_add(off & !3) },
        0b101 | 0b111 => Bl { target: (pc + 4).wrapping_add(off) },
        _ => Invalid(h1 << 16 | h2),
    }
}

fn decode_ldst_single(h1: u32, h2: u32, pc: u32, load: bool) -> Inst {
    use Inst::*;
    let signed = bit(h1, 8);
    let size = bits(h1, 6, 5);
    let rn = (h1 & 0xf) as u8;
    let rt = bits(h2, 15, 12) as u8;
    let sz = match (size, signed) {
        (0, false) => Sz::B,
        (0, true) => Sz::Sb,
        (1, false) => Sz::H,
        (1, true) => Sz::Sh,
        (2, false) => Sz::W,
        _ => return Invalid(h1 << 16 | h2),
    };
    // Preload hints.
    if load && rt == 15 && sz != Sz::W {
        return Nop;
    }
    let a = if rn == 15 {
        if !load {
            return Invalid(h1 << 16 | h2);
        }
        let imm = h2 & 0xfff;
        let base = (pc + 4) & !3;
        Addr::Lit(if bit(h1, 7) { base.wrapping_add(imm) } else { base.wrapping_sub(imm) })
    } else if bit(h1, 7) {
        Addr::Imm { rn, off: (h2 & 0xfff) as i32, pre: true, wb: false }
    } else if bit(h2, 11) {
        let p = bit(h2, 10);
        let u = bit(h2, 9);
        let w = bit(h2, 8);
        let imm = (h2 & 0xff) as i32;
        Addr::Imm { rn, off: if u { imm } else { -imm }, pre: p, wb: w }
    } else if bits(h2, 11, 6) == 0 {
        Addr::Reg { rn, rm: (h2 & 0xf) as u8, sh: bits(h2, 5, 4) as u8, add: true, pre: true, wb: false }
    } else {
        return Invalid(h1 << 16 | h2);
    };
    if load {
        Ld { sz, rt, a }
    } else {
        St { sz, rt, a }
    }
}

fn decode_dp_reg(h1: u32, h2: u32) -> Inst {
    use Inst::*;
    let op1 = bits(h1, 7, 4);
    let op2 = bits(h2, 7, 4);
    let rn = (h1 & 0xf) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = (h2 & 0xf) as u8;
    if bits(h2, 15, 12) != 0xf {
        return Invalid(h1 << 16 | h2);
    }
    if op1 & 0b1000 == 0 && op2 == 0 {
        let s = bit(h1, 4);
        let sh = match bits(h1, 6, 5) {
            0 => Sh::Lsl,
            1 => Sh::Lsr,
            2 => Sh::Asr,
            _ => Sh::Ror,
        };
        return Alu { op: self::Alu::Mov, s, rd, rn: 0, op2: Op2::ShiftReg(rn, sh, rm) };
    }
    if op1 & 0b1000 == 0 && op2 & 0b1000 != 0 {
        let rot = (bits(h2, 5, 4) * 8) as u8;
        let k = match op1 {
            0 => ExtK::Sxth,
            1 => ExtK::Uxth,
            4 => ExtK::Sxtb,
            5 => ExtK::Uxtb,
            _ => return Invalid(h1 << 16 | h2),
        };
        return Ext { k, rd, rn, rm, rot };
    }
    if op1 & 0b1100 == 0b1000 && op2 & 0b1100 == 0b1000 {
        return match (op1 & 3, op2 & 3) {
            (1, 0) => Rev { rd, rm },
            (1, 1) => Rev16 { rd, rm },
            (1, 2) => Rbit { rd, rm },
            (1, 3) => Revsh { rd, rm },
            (3, 0) => Clz { rd, rm },
            _ => Invalid(h1 << 16 | h2),
        };
    }
    Invalid(h1 << 16 | h2)
}

fn decode_mul(h1: u32, h2: u32) -> Inst {
    use Inst::*;
    let op1 = bits(h1, 6, 4);
    let op2 = bits(h2, 5, 4);
    let rn = (h1 & 0xf) as u8;
    let ra = bits(h2, 15, 12) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = (h2 & 0xf) as u8;
    match (op1, op2) {
        (0, 0) => {
            if ra == 15 {
                Mul { s: false, rd, rn, rm }
            } else {
                Mla { rd, rn, rm, ra, sub: false }
            }
        }
        (0, 1) => Mla { rd, rn, rm, ra, sub: true },
        (1, _) => SmulXY { rd, rn, rm, nt: bit(h2, 5), mt: bit(h2, 4), ra },
        _ => Invalid(h1 << 16 | h2),
    }
}

fn decode_mull(h1: u32, h2: u32) -> Inst {
    use Inst::*;
    let op1 = bits(h1, 6, 4);
    let op2 = bits(h2, 7, 4);
    let rn = (h1 & 0xf) as u8;
    let lo = bits(h2, 15, 12) as u8;
    let hi = bits(h2, 11, 8) as u8;
    let rm = (h2 & 0xf) as u8;
    match (op1, op2) {
        (0, 0) => MulL { k: self::MulL::Smull, lo, hi, rn, rm },
        (2, 0) => MulL { k: self::MulL::Umull, lo, hi, rn, rm },
        (4, 0) => MulL { k: self::MulL::Smlal, lo, hi, rn, rm },
        (6, 0) => MulL { k: self::MulL::Umlal, lo, hi, rn, rm },
        (1, 15) => Div { signed: true, rd: hi, rn, rm },
        (3, 15) => Div { signed: false, rd: hi, rn, rm },
        _ => Invalid(h1 << 16 | h2),
    }
}

/// Decode a VFP instruction given in ARM layout (cond in [31:28]).
pub fn decode_vfp(w: u32, pc: u32) -> Inst {
    use Inst::*;
    let coproc = bits(w, 11, 8);
    if coproc & 0b1110 != 0b1010 {
        return Invalid(w);
    }
    let dbl = coproc & 1 != 0;
    let d = bits(w, 22, 22);
    let vd4 = bits(w, 15, 12);
    let vn4 = bits(w, 19, 16);
    let n = bits(w, 7, 7);
    let vm4 = bits(w, 3, 0);
    let m = bits(w, 5, 5);
    let vd = if dbl { (d << 4 | vd4) as u8 } else { (vd4 << 1 | d) as u8 };
    let vn = if dbl { (n << 4 | vn4) as u8 } else { (vn4 << 1 | n) as u8 };
    let vm = if dbl { (m << 4 | vm4) as u8 } else { (vm4 << 1 | m) as u8 };
    let rn = vn4 as u8;
    let rt = vd4 as u8;

    match bits(w, 27, 24) {
        0b1100 | 0b1101 => {
            let p = bit(w, 24);
            let u = bit(w, 23);
            let wb = bit(w, 21);
            let l = bit(w, 20);
            if bits(w, 27, 21) == 0b1100010 {
                // 64-bit transfers
                let to_core = l;
                let rt2 = vn4 as u8;
                if dbl {
                    return Vmov2D { to_core, rt, rt2, dm: vm };
                } else {
                    return Vmov2S { to_core, rt, rt2, sm: vm };
                }
            }
            let imm = ((w & 0xff) * 4) as i32;
            if p && !wb {
                let off = if u { imm } else { -imm };
                let a = if rn == 15 {
                    Addr::Lit(((pc + 4) & !3).wrapping_add(off as u32))
                } else {
                    Addr::Imm { rn, off, pre: true, wb: false }
                };
                return VMem { load: l, dbl, vd, a };
            }
            if p == u {
                return Invalid(w);
            }
            let cnt = (w & 0xff) as u8;
            let n = if dbl { cnt / 2 } else { cnt };
            VMulti { load: l, dbl, vd, n, rn, wb, db: p }
        }
        0b1110 => {
            if !bit(w, 4) {
                decode_vfp_dp(w, dbl, vd, vn, vm)
            } else {
                let l = bit(w, 20);
                let opc = bits(w, 23, 21);
                if !bit(w, 8) {
                    if opc == 0 {
                        return VmovS { to_core: l, rt, sn: (vn4 << 1 | n) as u8 };
                    }
                    if opc == 7 {
                        if vn4 != 1 {
                            // FPSID/MVFR etc: treat as reading zero
                            return if l { Vmrs { rt: rt | 0x80 } } else { Nop };
                        }
                        return if l { Vmrs { rt } } else { Vmsr { rt } };
                    }
                    Invalid(w)
                } else {
                    // scalar transfers (32-bit only)
                    if bits(w, 6, 5) != 0 || bit(w, 22) {
                        return Invalid(w);
                    }
                    let dd = (n << 4 | vn4) as u8;
                    let idx = bits(w, 21, 21) as u8;
                    VmovScalar { to_core: l, rt, dd, idx }
                }
            }
        }
        _ => Invalid(w),
    }
}

fn decode_vfp_dp(w: u32, dbl: bool, vd: u8, vn: u8, vm: u8) -> Inst {
    use Inst::*;
    let opc1 = (bits(w, 23, 23) << 2) | bits(w, 21, 20);
    let op6 = bit(w, 6);
    match opc1 {
        0b000 => V2 { op: if op6 { VOp::Mls } else { VOp::Mla }, dbl, vd, vn, vm },
        0b001 => V2 { op: if op6 { VOp::Nmla } else { VOp::Nmls }, dbl, vd, vn, vm },
        0b010 => V2 { op: if op6 { VOp::Nmul } else { VOp::Mul }, dbl, vd, vn, vm },
        0b011 => V2 { op: if op6 { VOp::Sub } else { VOp::Add }, dbl, vd, vn, vm },
        0b100 if !op6 => V2 { op: VOp::Div, dbl, vd, vn, vm },
        0b111 => {
            if !op6 {
                let imm8 = (bits(w, 19, 16) << 4) | bits(w, 3, 0);
                return VmovImm { dbl, vd, bits: vfp_expand_imm(imm8, dbl) };
            }
            let opc2 = bits(w, 19, 16);
            let o7 = bit(w, 7);
            match opc2 {
                0b0000 => V1 { op: if o7 { VUn::Abs } else { VUn::Mov }, dbl, vd, vm },
                0b0001 => V1 { op: if o7 { VUn::Sqrt } else { VUn::Neg }, dbl, vd, vm },
                0b0100 => Vcmp { dbl, vd, vm },
                0b0101 => Vcmp { dbl, vd, vm: 0xff },
                0b0111 => {
                    if !o7 {
                        return Invalid(w);
                    }
                    // vd precision is the opposite of sz
                    let d = bits(w, 22, 22);
                    let vd4 = bits(w, 15, 12);
                    let vd2 = if dbl { (vd4 << 1 | d) as u8 } else { (d << 4 | vd4) as u8 };
                    VcvtFF { to_dbl: !dbl, vd: vd2, vm }
                }
                0b1000 => {
                    // int -> float; vm is always a single register
                    let m = bits(w, 5, 5);
                    let vm4 = bits(w, 3, 0);
                    VcvtIF { dbl, signed: o7, vd, vm: (vm4 << 1 | m) as u8 }
                }
                0b1100 | 0b1101 => {
                    let d = bits(w, 22, 22);
                    let vd4 = bits(w, 15, 12);
                    VcvtFI { dbl, signed: opc2 & 1 != 0, rz: o7, vd: (vd4 << 1 | d) as u8, vm }
                }
                0b1010 | 0b1011 | 0b1110 | 0b1111 => {
                    let to_fixed = opc2 & 0b0100 != 0;
                    let signed = opc2 & 1 == 0;
                    let half = !o7;
                    let size = if half { 16 } else { 32 };
                    let imm = (bits(w, 3, 0) << 1) | bits(w, 5, 5);
                    let fbits = (size - imm as i32) as u8;
                    VcvtFix { dbl, signed, half, to_fixed, vd, fbits }
                }
                _ => Invalid(w),
            }
        }
        _ => Invalid(w),
    }
}

/// Decode a basic block of Thumb code starting at `pc`.
pub fn decode_block(fetch: &dyn Fn(u32) -> u16, start: u32, max: usize) -> Block {
    let mut insts = Vec::new();
    let mut pc = start;
    // IT state: (firstcond, mask) per architecture.
    let mut it: Option<(u8, u8)> = None;
    loop {
        let hw1 = fetch(pc);
        let hw2 = fetch(pc + 2);
        let in_it = it.is_some();
        let (mut inst, size, newit) = decode(hw1, hw2, pc, in_it);
        let mut cond = COND_AL;
        if let Some((fc, mask)) = it {
            cond = fc;
            // advance IT state
            let nm = (mask << 1) & 0xf;
            if mask & 0x7 == 0 {
                it = None;
            } else {
                let newfc = (fc & 0xe) | ((mask >> 3) & 1);
                it = Some((newfc, nm));
            }
        }
        // 32-bit conditional branch marker
        if let Inst::Alu { op: Alu::Cmp, s: false, rd, op2: Op2::Imm(t), .. } = inst {
            if rd & 0xf0 == 0xf0 {
                cond = rd & 0xf;
                inst = Inst::B { target: t };
            }
        }
        match newit {
            Some((c, 0xff)) => {
                // 16-bit conditional branch
                cond = c;
            }
            Some((fc, mask)) => {
                it = Some((fc, mask));
                pc += size;
                continue;
            }
            None => {}
        }
        insts.push(Decoded { inst, cond, pc });
        pc += size;
        if ends_block(&inst) || (insts.len() >= max && it.is_none()) {
            break;
        }
    }
    Block { insts, end: pc, arm: false }
}
