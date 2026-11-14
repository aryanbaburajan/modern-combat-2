//! Decoded instruction representation shared by the Thumb and ARM decoders.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sh {
    Lsl,
    Lsr,
    Asr,
    Ror,
    Rrx,
}

#[derive(Clone, Copy, Debug)]
pub enum Op2 {
    /// Immediate; carry flag unaffected.
    Imm(u32),
    /// Modified immediate whose shifter carry-out is `c` (used by logical ops with S).
    ImmC(u32, bool),
    Reg(u8),
    /// Register shifted by an immediate (amount already decoded: 1..=32, or Rrx).
    Shift(u8, Sh, u8),
    /// Register shifted by the bottom byte of another register.
    ShiftReg(u8, Sh, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    And,
    Eor,
    Sub,
    Rsb,
    Add,
    Adc,
    Sbc,
    Rsc,
    Orr,
    Orn,
    Bic,
    Mov,
    Mvn,
    Tst,
    Teq,
    Cmp,
    Cmn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sz {
    B,
    Sb,
    H,
    Sh,
    W,
}

#[derive(Clone, Copy, Debug)]
pub enum Addr {
    /// `[rn, #off]` / `[rn], #off` / `[rn, #off]!`
    Imm { rn: u8, off: i32, pre: bool, wb: bool },
    /// `[rn, ±rm, LSL #sh]`, optionally with writeback (ARM only).
    Reg { rn: u8, rm: u8, sh: u8, add: bool, pre: bool, wb: bool },
    /// Absolute (PC-relative resolved at decode time).
    Lit(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulL {
    Umull,
    Smull,
    Umlal,
    Smlal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtK {
    Sxtb,
    Sxth,
    Uxtb,
    Uxth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VOp {
    Add,
    Sub,
    Mul,
    Nmul,
    Div,
    Mla,
    Mls,
    Nmla,
    Nmls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VUn {
    Mov,
    Abs,
    Neg,
    Sqrt,
}

#[derive(Clone, Copy, Debug)]
pub enum Inst {
    Nop,
    Alu { op: Alu, s: bool, rd: u8, rn: u8, op2: Op2 },
    Mul { s: bool, rd: u8, rn: u8, rm: u8 },
    Mla { rd: u8, rn: u8, rm: u8, ra: u8, sub: bool },
    MulL { k: MulL, lo: u8, hi: u8, rn: u8, rm: u8 },
    SmulXY { rd: u8, rn: u8, rm: u8, nt: bool, mt: bool, ra: u8 },
    Div { signed: bool, rd: u8, rn: u8, rm: u8 },
    Movt { rd: u8, imm: u16 },
    Bfx { signed: bool, rd: u8, rn: u8, lsb: u8, width: u8 },
    Bfi { rd: u8, rn: u8, lsb: u8, width: u8 },
    Ext { k: ExtK, rd: u8, rn: u8, rm: u8, rot: u8 },
    Clz { rd: u8, rm: u8 },
    Rev { rd: u8, rm: u8 },
    Rev16 { rd: u8, rm: u8 },
    Revsh { rd: u8, rm: u8 },
    Rbit { rd: u8, rm: u8 },
    Ssat { rd: u8, rn: u8, sat: u8, sh: Sh, amt: u8, unsigned: bool },
    Ld { sz: Sz, rt: u8, a: Addr },
    St { sz: Sz, rt: u8, a: Addr },
    Ldrd { rt: u8, rt2: u8, a: Addr },
    Strd { rt: u8, rt2: u8, a: Addr },
    Ldm { rn: u8, list: u16, wb: bool, db: bool },
    Stm { rn: u8, list: u16, wb: bool, db: bool },
    Ldrex { sz: Sz, rt: u8, rn: u8, off: u32 },
    Strex { sz: Sz, rd: u8, rt: u8, rn: u8, off: u32 },
    Ldrexd { rt: u8, rt2: u8, rn: u8 },
    Strexd { rd: u8, rt: u8, rt2: u8, rn: u8 },
    Clrex,
    B { target: u32 },
    Bl { target: u32 },
    /// Thumb -> ARM call.
    BlxImm { target: u32 },
    Bx { rm: u8 },
    BlxReg { rm: u8 },
    Cbz { rn: u8, target: u32, nz: bool },
    Tb { rn: u8, rm: u8, half: bool },
    Svc(u32),
    Bkpt(u32),
    Udf(u32),
    Mrs { rd: u8 },
    Msr { rn: u8 },
    // ---- VFP ----
    VMem { load: bool, dbl: bool, vd: u8, a: Addr },
    VMulti { load: bool, dbl: bool, vd: u8, n: u8, rn: u8, wb: bool, db: bool },
    VmovS { to_core: bool, rt: u8, sn: u8 },
    Vmov2D { to_core: bool, rt: u8, rt2: u8, dm: u8 },
    Vmov2S { to_core: bool, rt: u8, rt2: u8, sm: u8 },
    VmovScalar { to_core: bool, rt: u8, dd: u8, idx: u8 },
    VmovImm { dbl: bool, vd: u8, bits: u64 },
    V2 { op: VOp, dbl: bool, vd: u8, vn: u8, vm: u8 },
    V1 { op: VUn, dbl: bool, vd: u8, vm: u8 },
    /// `vm == 0xff` compares against +0.0.
    Vcmp { dbl: bool, vd: u8, vm: u8 },
    /// f32 <-> f64; `to_dbl` gives the destination precision.
    VcvtFF { to_dbl: bool, vd: u8, vm: u8 },
    /// Integer (in a single register) -> float.
    VcvtIF { dbl: bool, signed: bool, vd: u8, vm: u8 },
    /// Float -> integer (into a single register).
    VcvtFI { dbl: bool, signed: bool, rz: bool, vd: u8, vm: u8 },
    /// Fixed-point conversions (in place). `to_fixed` float->fixed.
    VcvtFix { dbl: bool, signed: bool, half: bool, to_fixed: bool, vd: u8, fbits: u8 },
    Vmrs { rt: u8 },
    Vmsr { rt: u8 },
    /// Unknown/unsupported encoding.
    Invalid(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct Decoded {
    pub inst: Inst,
    pub cond: u8,
    pub pc: u32,
}

pub const COND_AL: u8 = 14;

pub struct Block {
    pub insts: Vec<Decoded>,
    /// Address following the last instruction.
    pub end: u32,
    pub arm: bool,
}
