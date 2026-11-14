//! printf / scanf family.

use super::{Flow, VaArgs};
use crate::Emu;

fn fmt_exp(v: f64, prec: usize, upper: bool) -> String {
    if v == 0.0 {
        let mut s = format!("{:.*}", prec, 0.0);
        s += if upper { "E+00" } else { "e+00" };
        return s;
    }
    let s = format!("{:.*e}", prec, v);
    // Rust: 1.500000e3 -> C: 1.500000e+03
    let (mant, exp) = s.split_once('e').unwrap();
    let e: i32 = exp.parse().unwrap_or(0);
    format!("{}{}{}{:02}", mant, if upper { 'E' } else { 'e' }, if e < 0 { '-' } else { '+' }, e.abs())
}

fn strip_zeros(s: &mut String) {
    if let Some(epos) = s.find(|c| c == 'e' || c == 'E') {
        let (m, e) = s.split_at(epos);
        let mut m = m.to_string();
        if m.contains('.') {
            while m.ends_with('0') {
                m.pop();
            }
            if m.ends_with('.') {
                m.pop();
            }
        }
        *s = m + e;
    } else if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
}

fn fmt_float(v: f64, conv: u8, prec: Option<usize>, alt: bool) -> String {
    if v.is_nan() {
        return if conv.is_ascii_uppercase() { "NAN".into() } else { "nan".into() };
    }
    if v.is_infinite() {
        let s = if v < 0.0 { "-inf" } else { "inf" };
        return if conv.is_ascii_uppercase() { s.to_uppercase() } else { s.into() };
    }
    let p = prec.unwrap_or(6);
    match conv {
        b'f' | b'F' => format!("{:.*}", p, v),
        b'e' | b'E' => fmt_exp(v, p, conv == b'E'),
        _ => {
            let p = if p == 0 { 1 } else { p };
            let x = if v == 0.0 {
                0
            } else {
                let s = format!("{:.*e}", p - 1, v);
                s.split_once('e').unwrap().1.parse::<i32>().unwrap_or(0)
            };
            let mut s = if (x as i64) < p as i64 && x >= -4 {
                format!("{:.*}", (p as i32 - 1 - x).max(0) as usize, v)
            } else {
                fmt_exp(v, p - 1, conv == b'G')
            };
            if !alt {
                strip_zeros(&mut s);
            }
            s
        }
    }
}

/// Core formatter over a sequence of character units.
fn format_units(emu: &Emu, fmt: &[u32], va: &mut VaArgs, wide: bool) -> Vec<u32> {
    let mem = &emu.mem;
    let mut out: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' as u32 {
            out.push(c);
            continue;
        }
        let (mut left, mut plus, mut space, mut alt, mut zero) = (false, false, false, false, false);
        while i < fmt.len() {
            match fmt[i] as u8 {
                b'-' => left = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                b'0' => zero = true,
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        if i < fmt.len() && fmt[i] == b'*' as u32 {
            let w = va.u32(mem) as i32;
            if w < 0 {
                left = true;
            }
            width = w.unsigned_abs() as usize;
            i += 1;
        } else {
            while i < fmt.len() && (fmt[i] as u8).is_ascii_digit() && fmt[i] < 128 {
                width = width * 10 + (fmt[i] - b'0' as u32) as usize;
                i += 1;
            }
        }
        let mut prec: Option<usize> = None;
        if i < fmt.len() && fmt[i] == b'.' as u32 {
            i += 1;
            if i < fmt.len() && fmt[i] == b'*' as u32 {
                let p = va.u32(mem) as i32;
                prec = if p < 0 { None } else { Some(p as usize) };
                i += 1;
            } else {
                let mut p = 0;
                while i < fmt.len() && fmt[i] < 128 && (fmt[i] as u8).is_ascii_digit() {
                    p = p * 10 + (fmt[i] - b'0' as u32) as usize;
                    i += 1;
                }
                prec = Some(p);
            }
        }
        let mut lng = 0; // 0 int, 1 long, 2 long long, -1 short, -2 char
        while i < fmt.len() {
            match fmt[i] as u8 {
                b'h' => lng -= 1,
                b'l' => lng += 1,
                b'L' | b'q' | b'j' => lng = 2,
                b'z' | b't' => lng = 0,
                _ => break,
            }
            i += 1;
        }
        if i >= fmt.len() {
            break;
        }
        let conv = fmt[i] as u8;
        i += 1;
        let mut body: Vec<u32> = Vec::new();
        let mut sign = String::new();
        let mut numeric = false;
        match conv {
            b'%' => body.push(b'%' as u32),
            b'd' | b'i' => {
                let v: i64 = if lng >= 2 {
                    va.u64(mem) as i64
                } else {
                    let x = va.u32(mem);
                    match lng {
                        -1 => x as i16 as i64,
                        -2 => x as i8 as i64,
                        _ => x as i32 as i64,
                    }
                };
                if v < 0 {
                    sign = "-".into();
                } else if plus {
                    sign = "+".into();
                } else if space {
                    sign = " ".into();
                }
                let mut digits = v.unsigned_abs().to_string();
                if let Some(p) = prec {
                    if p == 0 && v == 0 {
                        digits.clear();
                    }
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                }
                body.extend(digits.bytes().map(|b| b as u32));
                numeric = prec.is_none();
            }
            b'u' | b'x' | b'X' | b'o' => {
                let v: u64 = if lng >= 2 {
                    va.u64(mem)
                } else {
                    let x = va.u32(mem);
                    match lng {
                        -1 => x as u16 as u64,
                        -2 => x as u8 as u64,
                        _ => x as u64,
                    }
                };
                let mut digits = match conv {
                    b'u' => v.to_string(),
                    b'x' => format!("{:x}", v),
                    b'X' => format!("{:X}", v),
                    _ => format!("{:o}", v),
                };
                if let Some(p) = prec {
                    if p == 0 && v == 0 {
                        digits.clear();
                    }
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                }
                if alt && v != 0 {
                    match conv {
                        b'x' => sign = "0x".into(),
                        b'X' => sign = "0X".into(),
                        b'o' => digits.insert(0, '0'),
                        _ => {}
                    }
                }
                body.extend(digits.bytes().map(|b| b as u32));
                numeric = prec.is_none();
            }
            b'p' => {
                let v = va.u32(mem);
                body.extend(format!("0x{:x}", v).bytes().map(|b| b as u32));
            }
            b'c' => {
                let v = va.u32(mem);
                body.push(if wide || lng > 0 { v } else { v & 0xff });
            }
            b'C' => body.push(va.u32(mem)),
            b's' | b'S' => {
                let p = va.u32(mem);
                let wide_arg = conv == b'S' || lng > 0;
                if p == 0 {
                    body.extend(b"(null)".iter().map(|&b| b as u32));
                } else if wide_arg {
                    let mut a = p;
                    loop {
                        let ch = mem.r32(a);
                        if ch == 0 || prec.map_or(false, |m| body.len() >= m) {
                            break;
                        }
                        body.push(ch);
                        a += 4;
                    }
                } else {
                    let s = mem.cstr_bytes(p);
                    let s = match prec {
                        Some(m) => &s[..m.min(s.len())],
                        None => s,
                    };
                    body.extend(s.iter().map(|&b| b as u32));
                }
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let v = va.f64(mem);
                let mut s = fmt_float(v, conv, prec, alt);
                if s.starts_with('-') {
                    s.remove(0);
                    sign = "-".into();
                } else if plus {
                    sign = "+".into();
                } else if space {
                    sign = " ".into();
                }
                body.extend(s.bytes().map(|b| b as u32));
                numeric = v.is_finite();
            }
            b'n' => {
                let p = va.u32(mem);
                let _ = p; // writing would need &mut; rarely used
            }
            _ => {
                body.push(b'%' as u32);
                body.push(conv as u32);
            }
        }
        let len = sign.len() + body.len();
        if len >= width {
            out.extend(sign.bytes().map(|b| b as u32));
            out.extend(body);
        } else if left {
            out.extend(sign.bytes().map(|b| b as u32));
            out.extend(body);
            out.extend(std::iter::repeat(b' ' as u32).take(width - len));
        } else if zero && numeric {
            out.extend(sign.bytes().map(|b| b as u32));
            out.extend(std::iter::repeat(b'0' as u32).take(width - len));
            out.extend(body);
        } else {
            out.extend(std::iter::repeat(b' ' as u32).take(width - len));
            out.extend(sign.bytes().map(|b| b as u32));
            out.extend(body);
        }
    }
    out
}

/// Format a narrow printf string at `fmt_addr`.
pub fn format(emu: &Emu, fmt_addr: u32, va: &mut VaArgs) -> Vec<u8> {
    let f: Vec<u32> = emu.mem.cstr_bytes(fmt_addr).iter().map(|&b| b as u32).collect();
    format_units(emu, &f, va, false).into_iter().map(|c| c as u8).collect()
}

pub fn format_wide(emu: &Emu, fmt_addr: u32, va: &mut VaArgs) -> Vec<u32> {
    let n = emu.mem.wstrlen(fmt_addr);
    let f: Vec<u32> = (0..n).map(|i| emu.mem.r32(fmt_addr + 4 * i as u32)).collect();
    format_units(emu, &f, va, true)
}

fn write_out(emu: &mut Emu, buf: u32, cap: Option<usize>, s: &[u8]) -> u32 {
    let n = match cap {
        Some(0) => return s.len() as u32,
        Some(c) => s.len().min(c - 1),
        None => s.len(),
    };
    emu.mem.write_bytes(buf, &s[..n]);
    emu.mem.w8(buf + n as u32, 0);
    s.len() as u32
}

pub fn sprintf(emu: &mut Emu) -> Flow {
    let mut va = VaArgs::from_call(emu, 2);
    let s = format(emu, emu.arg(1), &mut va);
    let r = write_out(emu, emu.arg(0), None, &s);
    emu.ret(r);
    Flow::Ret
}
pub fn snprintf(emu: &mut Emu) -> Flow {
    let mut va = VaArgs::from_call(emu, 3);
    let s = format(emu, emu.arg(2), &mut va);
    let r = write_out(emu, emu.arg(0), Some(emu.arg(1) as usize), &s);
    emu.ret(r);
    Flow::Ret
}
pub fn vsprintf(emu: &mut Emu) -> Flow {
    let mut va = VaArgs::from_va_list(emu.arg(2));
    let s = format(emu, emu.arg(1), &mut va);
    let r = write_out(emu, emu.arg(0), None, &s);
    emu.ret(r);
    Flow::Ret
}
pub fn vsnprintf(emu: &mut Emu) -> Flow {
    let mut va = VaArgs::from_va_list(emu.arg(3));
    let s = format(emu, emu.arg(2), &mut va);
    let r = write_out(emu, emu.arg(0), Some(emu.arg(1) as usize), &s);
    emu.ret(r);
    Flow::Ret
}
pub fn swprintf(emu: &mut Emu) -> Flow {
    let (buf, cap) = (emu.arg(0), emu.arg(1) as usize);
    let mut va = VaArgs::from_call(emu, 3);
    let s = format_wide(emu, emu.arg(2), &mut va);
    if cap == 0 {
        emu.ret(u32::MAX);
        return Flow::Ret;
    }
    let n = s.len().min(cap - 1);
    for (i, &c) in s[..n].iter().enumerate() {
        emu.mem.w32(buf + 4 * i as u32, c);
    }
    emu.mem.w32(buf + 4 * n as u32, 0);
    emu.ret(if s.len() >= cap { u32::MAX } else { s.len() as u32 });
    Flow::Ret
}
pub fn printf(emu: &mut Emu) -> Flow {
    let mut va = VaArgs::from_call(emu, 1);
    let s = format(emu, emu.arg(0), &mut va);
    let msg = String::from_utf8_lossy(&s).trim_end().to_string();
    emu.log(&format!("[stdout] {}", msg));
    emu.ret(s.len() as u32);
    Flow::Ret
}

// ---------------- scanf ----------------

pub fn sscanf(emu: &mut Emu) -> Flow {
    let input = emu.mem.cstr_bytes(emu.arg(0)).to_vec();
    let fmt = emu.mem.cstr_bytes(emu.arg(1)).to_vec();
    let mut va = VaArgs::from_call(emu, 2);
    let r = scan(emu, &input, &fmt, &mut va);
    emu.ret(r as u32);
    Flow::Ret
}

pub fn scan(emu: &mut Emu, inp: &[u8], fmt: &[u8], va: &mut VaArgs) -> i32 {
    let mut ip = 0usize;
    let mut fp = 0usize;
    let mut count = 0i32;
    let ws = |c: u8| c == b' ' || (9..=13).contains(&c);
    while fp < fmt.len() {
        let c = fmt[fp];
        if ws(c) {
            while fp < fmt.len() && ws(fmt[fp]) {
                fp += 1;
            }
            while ip < inp.len() && ws(inp[ip]) {
                ip += 1;
            }
            continue;
        }
        if c != b'%' {
            if ip < inp.len() && inp[ip] == c {
                ip += 1;
                fp += 1;
                continue;
            }
            break;
        }
        fp += 1;
        if fp < fmt.len() && fmt[fp] == b'%' {
            if ip < inp.len() && inp[ip] == b'%' {
                ip += 1;
                fp += 1;
                continue;
            }
            break;
        }
        let mut suppress = false;
        if fp < fmt.len() && fmt[fp] == b'*' {
            suppress = true;
            fp += 1;
        }
        let mut width = 0usize;
        while fp < fmt.len() && fmt[fp].is_ascii_digit() {
            width = width * 10 + (fmt[fp] - b'0') as usize;
            fp += 1;
        }
        let mut lng = 0;
        while fp < fmt.len() && matches!(fmt[fp], b'h' | b'l' | b'L' | b'q' | b'z' | b'j') {
            match fmt[fp] {
                b'h' => lng -= 1,
                b'l' => lng += 1,
                b'L' | b'q' | b'j' => lng = 2,
                _ => {}
            }
            fp += 1;
        }
        if fp >= fmt.len() {
            break;
        }
        let conv = fmt[fp];
        fp += 1;
        if conv != b'c' && conv != b'[' && conv != b'n' {
            while ip < inp.len() && ws(inp[ip]) {
                ip += 1;
            }
        }
        let maxw = if width == 0 { usize::MAX } else { width };
        let rest = &inp[ip..inp.len().min(ip.saturating_add(maxw))];
        match conv {
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' => {
                let base = match conv {
                    b'd' | b'u' => 10,
                    b'i' => 0,
                    b'o' => 8,
                    _ => 16,
                };
                let (v, n, _) = super::libc::parse_int(rest, base, conv != b'u');
                if n == 0 {
                    break;
                }
                ip += n;
                if !suppress {
                    let p = va.u32(&emu.mem);
                    match lng {
                        -2 => emu.mem.w8(p, v as u8),
                        -1 => emu.mem.w16(p, v as u16),
                        2 => emu.mem.w64(p, v as u64),
                        _ => emu.mem.w32(p, v as u32),
                    }
                    count += 1;
                }
            }
            b'f' | b'e' | b'g' | b'E' | b'G' | b'a' => {
                let (v, n) = super::libc::parse_float(rest);
                if n == 0 {
                    break;
                }
                ip += n;
                if !suppress {
                    let p = va.u32(&emu.mem);
                    if lng >= 1 {
                        emu.mem.w64(p, v.to_bits());
                    } else {
                        emu.mem.w32(p, (v as f32).to_bits());
                    }
                    count += 1;
                }
            }
            b's' => {
                let n = rest.iter().position(|&b| ws(b)).unwrap_or(rest.len());
                if n == 0 {
                    break;
                }
                if !suppress {
                    let p = va.u32(&emu.mem);
                    let s = rest[..n].to_vec();
                    emu.mem.write_cstr(p, &s);
                    count += 1;
                }
                ip += n;
            }
            b'c' => {
                let w = if width == 0 { 1 } else { width };
                if inp.len() - ip < w {
                    break;
                }
                if !suppress {
                    let p = va.u32(&emu.mem);
                    let s = inp[ip..ip + w].to_vec();
                    emu.mem.write_bytes(p, &s);
                    count += 1;
                }
                ip += w;
            }
            b'[' => {
                let mut neg = false;
                if fp < fmt.len() && fmt[fp] == b'^' {
                    neg = true;
                    fp += 1;
                }
                let mut set = Vec::new();
                if fp < fmt.len() && fmt[fp] == b']' {
                    set.push(b']');
                    fp += 1;
                }
                while fp < fmt.len() && fmt[fp] != b']' {
                    if fp + 2 < fmt.len() && fmt[fp + 1] == b'-' && fmt[fp + 2] != b']' {
                        for ch in fmt[fp]..=fmt[fp + 2] {
                            set.push(ch);
                        }
                        fp += 3;
                    } else {
                        set.push(fmt[fp]);
                        fp += 1;
                    }
                }
                fp += 1;
                let n = rest.iter().position(|b| set.contains(b) == neg).unwrap_or(rest.len());
                if n == 0 {
                    break;
                }
                if !suppress {
                    let p = va.u32(&emu.mem);
                    let s = rest[..n].to_vec();
                    emu.mem.write_cstr(p, &s);
                    count += 1;
                }
                ip += n;
            }
            b'n' => {
                if !suppress {
                    let p = va.u32(&emu.mem);
                    emu.mem.w32(p, ip as u32);
                }
            }
            _ => break,
        }
    }
    if count == 0 && ip >= inp.len() && !inp.is_empty() && fmt.iter().any(|&c| c == b'%') {
        return count;
    }
    if inp.is_empty() && count == 0 {
        return -1;
    }
    count
}
