//! libm (softfp ABI: floats in core registers).

use super::{Flow, HleFn};
use crate::Emu;
use std::collections::HashMap;

fn d_sin(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.sin()); Flow::Ret }
fn f_sin(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.sin()); Flow::Ret }
fn d_cos(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.cos()); Flow::Ret }
fn f_cos(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.cos()); Flow::Ret }
fn d_tan(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.tan()); Flow::Ret }
fn f_tan(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.tan()); Flow::Ret }
fn d_asin(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.asin()); Flow::Ret }
fn f_asin(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.asin()); Flow::Ret }
fn d_acos(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.acos()); Flow::Ret }
fn f_acos(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.acos()); Flow::Ret }
fn d_atan(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.atan()); Flow::Ret }
fn f_atan(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.atan()); Flow::Ret }
fn d_sinh(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.sinh()); Flow::Ret }
fn f_sinh(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.sinh()); Flow::Ret }
fn d_cosh(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.cosh()); Flow::Ret }
fn f_cosh(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.cosh()); Flow::Ret }
fn d_tanh(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.tanh()); Flow::Ret }
fn f_tanh(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.tanh()); Flow::Ret }
fn d_exp(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.exp()); Flow::Ret }
fn f_exp(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.exp()); Flow::Ret }
fn d_log(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.ln()); Flow::Ret }
fn f_log(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.ln()); Flow::Ret }
fn d_log10(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.log10()); Flow::Ret }
fn f_log10(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.log10()); Flow::Ret }
fn d_sqrt(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.sqrt()); Flow::Ret }
fn f_sqrt(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.sqrt()); Flow::Ret }
fn d_ceil(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.ceil()); Flow::Ret }
fn f_ceil(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.ceil()); Flow::Ret }
fn d_floor(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.floor()); Flow::Ret }
fn f_floor(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.floor()); Flow::Ret }
fn d_exp2(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.exp2()); Flow::Ret }
fn f_exp2(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.exp2()); Flow::Ret }
fn d_trunc(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.trunc()); Flow::Ret }
fn f_trunc(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.trunc()); Flow::Ret }
fn d_round(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.round()); Flow::Ret }
fn f_round(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.round()); Flow::Ret }
fn d_fabs(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.abs()); Flow::Ret }
fn f_fabs(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.abs()); Flow::Ret }
fn d_log2(e: &mut Emu) -> Flow { let x = e.arg_f64(0); e.ret_f64(x.log2()); Flow::Ret }
fn f_log2(e: &mut Emu) -> Flow { let x = e.arg_f32(0); e.ret_f32(x.log2()); Flow::Ret }
fn d_atan2(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let b = e.arg_f64(2); e.ret_f64(a.atan2(b)); Flow::Ret }
fn f_atan2(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let b = e.arg_f32(1); e.ret_f32(a.atan2(b)); Flow::Ret }
fn d_pow(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let b = e.arg_f64(2); e.ret_f64(a.powf(b)); Flow::Ret }
fn f_pow(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let b = e.arg_f32(1); e.ret_f32(a.powf(b)); Flow::Ret }
fn d_fmod(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let b = e.arg_f64(2); e.ret_f64(a % b); Flow::Ret }
fn f_fmod(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let b = e.arg_f32(1); e.ret_f32(a % b); Flow::Ret }
fn d_hypot(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let b = e.arg_f64(2); e.ret_f64(a.hypot(b)); Flow::Ret }
fn f_hypot(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let b = e.arg_f32(1); e.ret_f32(a.hypot(b)); Flow::Ret }
fn d_ldexp(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let n = e.arg(2) as i32; e.ret_f64(a * 2f64.powi(n)); Flow::Ret }
fn f_ldexp(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let n = e.arg(1) as i32; e.ret_f32(a * 2f32.powi(n)); Flow::Ret }
fn d_frexp(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let p = e.arg(2); let (m, x) = frexp(a); e.mem.w32(p, x as u32); e.ret_f64(m); Flow::Ret }
fn d_modf(e: &mut Emu) -> Flow { let a = e.arg_f64(0); let p = e.arg(2); let i = a.trunc(); e.mem.w64(p, i.to_bits()); e.ret_f64(a - i); Flow::Ret }
fn f_modf(e: &mut Emu) -> Flow { let a = e.arg_f32(0); let p = e.arg(1); let i = a.trunc(); e.mem.w32(p, i.to_bits()); e.ret_f32(a - i); Flow::Ret }
fn frexp(v: f64) -> (f64, i32) {
    if v == 0.0 || !v.is_finite() { return (v, 0); }
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 { let (m, e) = frexp(v * 2f64.powi(54)); return (m, e - 54); }
    let m = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
    (m, exp - 1022)
}

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    r.insert("sin", d_sin);
    r.insert("sinf", f_sin);
    r.insert("cos", d_cos);
    r.insert("cosf", f_cos);
    r.insert("tan", d_tan);
    r.insert("tanf", f_tan);
    r.insert("asin", d_asin);
    r.insert("asinf", f_asin);
    r.insert("acos", d_acos);
    r.insert("acosf", f_acos);
    r.insert("atan", d_atan);
    r.insert("atanf", f_atan);
    r.insert("sinh", d_sinh);
    r.insert("sinhf", f_sinh);
    r.insert("cosh", d_cosh);
    r.insert("coshf", f_cosh);
    r.insert("tanh", d_tanh);
    r.insert("tanhf", f_tanh);
    r.insert("exp", d_exp);
    r.insert("expf", f_exp);
    r.insert("log", d_log);
    r.insert("logf", f_log);
    r.insert("log10", d_log10);
    r.insert("log10f", f_log10);
    r.insert("sqrt", d_sqrt);
    r.insert("sqrtf", f_sqrt);
    r.insert("ceil", d_ceil);
    r.insert("ceilf", f_ceil);
    r.insert("floor", d_floor);
    r.insert("floorf", f_floor);
    r.insert("exp2", d_exp2);
    r.insert("exp2f", f_exp2);
    r.insert("trunc", d_trunc);
    r.insert("truncf", f_trunc);
    r.insert("round", d_round);
    r.insert("roundf", f_round);
    r.insert("fabs", d_fabs);
    r.insert("fabsf", f_fabs);
    r.insert("log2", d_log2);
    r.insert("log2f", f_log2);
    r.insert("atan2", d_atan2);
    r.insert("atan2f", f_atan2);
    r.insert("pow", d_pow);
    r.insert("powf", f_pow);
    r.insert("fmod", d_fmod);
    r.insert("fmodf", f_fmod);
    r.insert("hypot", d_hypot);
    r.insert("hypotf", f_hypot);
    r.insert("ldexp", d_ldexp);
    r.insert("ldexpf", f_ldexp);
    r.insert("frexp", d_frexp);
    r.insert("modf", d_modf);
    r.insert("modff", f_modf);
}
