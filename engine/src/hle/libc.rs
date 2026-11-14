//! libc: memory, strings, ctype, time, process, misc stubs.

use super::{Flow, HleFn, State, VaArgs};
use crate::mem::Memory;
use crate::Emu;
use std::collections::HashMap;

pub const ERRNO_ENOENT: u32 = 2;
pub const ERRNO_EINVAL: u32 = 22;
pub const ERRNO_ENOSYS: u32 = 38;

pub fn init_data(st: &mut State, mem: &mut Memory) {
    // ctype table: _ctype_ points at a 257-entry table whose [0] is EOF.
    const U: u8 = 0x01;
    const L: u8 = 0x02;
    const N: u8 = 0x04;
    const S: u8 = 0x08;
    const P: u8 = 0x10;
    const C: u8 = 0x20;
    const X: u8 = 0x40;
    const B: u8 = 0x80;
    let table = st.static_alloc(260, 4);
    for c in 0..256u32 {
        let ch = c as u8;
        let mut f = 0u8;
        if c < 128 {
            if ch.is_ascii_uppercase() {
                f |= U;
            }
            if ch.is_ascii_lowercase() {
                f |= L;
            }
            if ch.is_ascii_digit() {
                f |= N;
            }
            if ch == b' ' || (9..=13).contains(&ch) {
                f |= S;
            }
            if ch.is_ascii_punctuation() {
                f |= P;
            }
            if ch < 32 || ch == 127 {
                f |= C;
            }
            if ch.is_ascii_hexdigit() {
                f |= X;
            }
            if ch == b' ' {
                f |= B;
            }
        }
        mem.w8(table + 1 + c, f);
    }
    let ctype_ptr = st.static_alloc(4, 4);
    mem.w32(ctype_ptr, table);
    st.data_syms.insert("_ctype_", ctype_ptr);

    let lower = st.static_alloc(257 * 2, 4);
    let upper = st.static_alloc(257 * 2, 4);
    mem.w16(lower, 0xffff);
    mem.w16(upper, 0xffff);
    for c in 0..256u32 {
        let ch = c as u8;
        mem.w16(lower + 2 + 2 * c, ch.to_ascii_lowercase() as u16);
        mem.w16(upper + 2 + 2 * c, ch.to_ascii_uppercase() as u16);
    }
    let lp = st.static_alloc(4, 4);
    let up = st.static_alloc(4, 4);
    mem.w32(lp, lower);
    mem.w32(up, upper);
    st.data_syms.insert("_tolower_tab_", lp);
    st.data_syms.insert("_toupper_tab_", up);

    let guard = st.static_alloc(4, 4);
    mem.w32(guard, 0xC0DE_F00D);
    st.data_syms.insert("__stack_chk_guard", guard);
    let dso = st.static_alloc(4, 4);
    st.data_syms.insert("__dso_handle", dso);
    let nothrow = st.static_alloc(4, 4);
    st.data_syms.insert("_ZSt7nothrow", nothrow);
}

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    macro_rules! reg {
        ($($name:literal => $f:expr),* $(,)?) => { $( r.insert($name, $f); )* };
    }
    reg! {
        "malloc" => malloc, "calloc" => calloc, "realloc" => realloc, "free" => free,
        "memalign" => memalign,
        "_Znwj" => malloc, "_Znaj" => malloc, "_ZdlPv" => free, "_ZdaPv" => free,
        "_ZnajRKSt9nothrow_t" => malloc, "_ZnwjRKSt9nothrow_t" => malloc,
        "memcpy" => memcpy, "memmove" => memcpy, "memset" => memset, "memcmp" => memcmp, "memchr" => memchr,
        "__aeabi_memcpy" => memcpy, "__aeabi_memcpy4" => memcpy, "__aeabi_memcpy8" => memcpy,
        "__aeabi_memmove" => memcpy, "__aeabi_memset" => aeabi_memset, "__aeabi_memset4" => aeabi_memset,
        "__aeabi_memclr" => aeabi_memclr, "__aeabi_memclr4" => aeabi_memclr, "__aeabi_memclr8" => aeabi_memclr,
        "strlen" => strlen, "strcmp" => strcmp, "strncmp" => strncmp, "strcasecmp" => strcasecmp,
        "strncasecmp" => strncasecmp, "strcpy" => strcpy, "strncpy" => strncpy, "strlcpy" => strlcpy,
        "strcat" => strcat, "strncat" => strncat, "strchr" => strchr, "strrchr" => strrchr,
        "strstr" => strstr, "strdup" => strdup, "strcspn" => strcspn, "strspn" => strspn, "strpbrk" => strpbrk,
        "strtok" => strtok, "strtol" => strtol, "strtoul" => strtoul, "strtod" => strtod, "atoi" => atoi, "atol" => atoi,
        "atof" => atof, "strerror" => strerror,
        "wcslen" => wcslen, "wcscpy" => wcscpy,
        "qsort" => qsort,
        "abort" => abort, "exit" => exit, "__assert2" => assert2,
        "__errno" => errno_loc,
        "__stack_chk_fail" => stack_chk_fail,
        "__cxa_guard_acquire" => cxa_guard_acquire, "__cxa_guard_release" => cxa_guard_release,
        "__cxa_guard_abort" => ret0,
        "__cxa_pure_virtual" => pure_virtual, "__aeabi_atexit" => ret0, "__cxa_atexit" => ret0, "atexit" => ret0,
        "__cxa_finalize" => ret0,
        "__aeabi_idiv" => aeabi_idiv, "__aeabi_idivmod" => aeabi_idivmod,
        "__aeabi_uidiv" => aeabi_uidiv, "__aeabi_uidivmod" => aeabi_uidivmod,
        "__aeabi_ldivmod" => aeabi_ldivmod, "__aeabi_uldivmod" => aeabi_uldivmod,
        "__aeabi_ul2d" => aeabi_ul2d,
        "__aeabi_unwind_cpp_pr0" => ret0, "__aeabi_unwind_cpp_pr1" => ret0,
        "gettimeofday" => gettimeofday, "time" => time, "gmtime" => gmtime, "localtime" => gmtime,
        "mktime" => mktime, "clock" => clock,
        "nanosleep" => nanosleep, "usleep" => usleep, "sleep" => sleep,
        "sysconf" => sysconf,
        "lrand48" => lrand48, "srand48" => srand48, "rand" => rand, "srand" => srand48,
        "setjmp" => setjmp, "longjmp" => longjmp, "_setjmp" => setjmp, "_longjmp" => longjmp,
        "sigaction" => ret0, "signal" => ret0, "raise" => ret0,
        "setlocale" => setlocale,
        "getpid" => getpid, "gethostname" => gethostname, "getenv" => ret0,
        "mmap" => mmap, "munmap" => munmap, "mprotect" => ret0, "cacheflush" => ret0,
        "ioctl" => ret_neg1, "fcntl" => ret0, "select" => ret0,
        "socket" => ret_neg1_enosys, "bind" => ret_neg1_enosys, "connect" => ret_neg1_enosys,
        "listen" => ret_neg1_enosys, "accept" => ret_neg1_enosys, "send" => ret_neg1_enosys,
        "sendto" => ret_neg1_enosys, "recv" => ret_neg1_enosys, "recvfrom" => ret_neg1_enosys,
        "setsockopt" => ret_neg1_enosys, "getsockopt" => ret_neg1_enosys, "getsockname" => ret_neg1_enosys,
        "getaddrinfo" => ret_neg1, "freeaddrinfo" => ret0, "gethostbyname" => ret0,
        "inet_addr" => inet_addr, "inet_ntoa" => inet_ntoa,
        "__android_log_print" => android_log_print, "__android_log_write" => android_log_write,
        "__android_log_vprint" => android_log_vprint,
        "dlopen" => ret0, "dlsym" => ret0, "dlclose" => ret0, "dlerror" => ret0,
    }
}

pub fn set_errno(emu: &mut Emu, v: u32) {
    let a = super::thread::errno_addr(emu);
    emu.mem.w32(a, v);
}

fn ret0(emu: &mut Emu) -> Flow {
    emu.ret(0);
    Flow::Ret
}
fn ret_neg1(emu: &mut Emu) -> Flow {
    emu.ret(u32::MAX);
    Flow::Ret
}
fn ret_neg1_enosys(emu: &mut Emu) -> Flow {
    set_errno(emu, ERRNO_ENOSYS);
    emu.ret(u32::MAX);
    Flow::Ret
}

// ---------------- memory ----------------

fn malloc(emu: &mut Emu) -> Flow {
    let n = emu.arg(0);
    let p = emu.hle.heap.malloc(&mut emu.mem, n);
    if p == 0 {
        let msg = format!("[hle] malloc({}) failed: out of guest memory (live {} MB)", n, emu.hle.heap.live_bytes >> 20);
        emu.log(&msg);
    }
    emu.ret(p);
    Flow::Ret
}
fn memalign(emu: &mut Emu) -> Flow {
    let (a, n) = (emu.arg(0), emu.arg(1));
    let p = emu.hle.heap.memalign(&mut emu.mem, a, n);
    emu.ret(p);
    Flow::Ret
}
fn calloc(emu: &mut Emu) -> Flow {
    let n = emu.arg(0).wrapping_mul(emu.arg(1));
    let p = emu.hle.heap.malloc(&mut emu.mem, n);
    if p != 0 {
        emu.mem.fill(p, 0, n as usize);
    }
    emu.ret(p);
    Flow::Ret
}
fn realloc(emu: &mut Emu) -> Flow {
    let (p, n) = (emu.arg(0), emu.arg(1));
    let r = emu.hle.heap.realloc(&mut emu.mem, p, n);
    emu.ret(r);
    Flow::Ret
}
fn free(emu: &mut Emu) -> Flow {
    let p = emu.arg(0);
    if !emu.hle.heap.free(&mut emu.mem, p) {
        let msg = format!("[hle] free of invalid pointer {:08x} from {}", p, emu.symbolize(emu.cpu.r[14]));
        emu.log(&msg);
    }
    Flow::Ret
}

fn memcpy(emu: &mut Emu) -> Flow {
    let (d, s, n) = (emu.arg(0), emu.arg(1), emu.arg(2));
    emu.mem.copy_within(d, s, n as usize);
    emu.ret(d);
    Flow::Ret
}
fn memset(emu: &mut Emu) -> Flow {
    let (d, c, n) = (emu.arg(0), emu.arg(1), emu.arg(2));
    emu.mem.fill(d, c as u8, n as usize);
    emu.ret(d);
    Flow::Ret
}
fn aeabi_memset(emu: &mut Emu) -> Flow {
    // __aeabi_memset(dest, n, c)
    let (d, n, c) = (emu.arg(0), emu.arg(1), emu.arg(2));
    emu.mem.fill(d, c as u8, n as usize);
    Flow::Ret
}
fn aeabi_memclr(emu: &mut Emu) -> Flow {
    let (d, n) = (emu.arg(0), emu.arg(1));
    emu.mem.fill(d, 0, n as usize);
    Flow::Ret
}
fn memcmp(emu: &mut Emu) -> Flow {
    let (a, b, n) = (emu.arg(0), emu.arg(1), emu.arg(2) as usize);
    let r = {
        let x = emu.mem.slice(a, n);
        let y = emu.mem.slice(b, n);
        let mut r = 0i32;
        for i in 0..n.min(x.len()).min(y.len()) {
            if x[i] != y[i] {
                r = x[i] as i32 - y[i] as i32;
                break;
            }
        }
        r
    };
    emu.ret(r as u32);
    Flow::Ret
}
fn memchr(emu: &mut Emu) -> Flow {
    let (a, c, n) = (emu.arg(0), emu.arg(1) as u8, emu.arg(2) as usize);
    let r = emu.mem.slice(a, n).iter().position(|&b| b == c).map(|i| a + i as u32).unwrap_or(0);
    emu.ret(r);
    Flow::Ret
}

// ---------------- strings ----------------

fn strlen(emu: &mut Emu) -> Flow {
    let n = emu.mem.strlen(emu.arg(0));
    emu.ret(n as u32);
    Flow::Ret
}

fn cmp_bytes(mem: &Memory, a: u32, b: u32, n: usize, fold: bool) -> i32 {
    for i in 0..n as u32 {
        let mut x = mem.r8(a + i);
        let mut y = mem.r8(b + i);
        if fold {
            x = x.to_ascii_lowercase();
            y = y.to_ascii_lowercase();
        }
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
    }
    0
}
fn strcmp(emu: &mut Emu) -> Flow {
    let r = cmp_bytes(&emu.mem, emu.arg(0), emu.arg(1), usize::MAX, false);
    emu.ret(r as u32);
    Flow::Ret
}
fn strncmp(emu: &mut Emu) -> Flow {
    let r = cmp_bytes(&emu.mem, emu.arg(0), emu.arg(1), emu.arg(2) as usize, false);
    emu.ret(r as u32);
    Flow::Ret
}
fn strcasecmp(emu: &mut Emu) -> Flow {
    let r = cmp_bytes(&emu.mem, emu.arg(0), emu.arg(1), usize::MAX, true);
    emu.ret(r as u32);
    Flow::Ret
}
fn strncasecmp(emu: &mut Emu) -> Flow {
    let r = cmp_bytes(&emu.mem, emu.arg(0), emu.arg(1), emu.arg(2) as usize, true);
    emu.ret(r as u32);
    Flow::Ret
}
fn strcpy(emu: &mut Emu) -> Flow {
    let (d, s) = (emu.arg(0), emu.arg(1));
    let n = emu.mem.strlen(s);
    emu.mem.copy_within(d, s, n + 1);
    emu.ret(d);
    Flow::Ret
}
fn strncpy(emu: &mut Emu) -> Flow {
    let (d, s, n) = (emu.arg(0), emu.arg(1), emu.arg(2) as usize);
    let l = emu.mem.strlen(s).min(n);
    emu.mem.copy_within(d, s, l);
    if l < n {
        emu.mem.fill(d + l as u32, 0, n - l);
    }
    emu.ret(d);
    Flow::Ret
}
fn strlcpy(emu: &mut Emu) -> Flow {
    let (d, s, n) = (emu.arg(0), emu.arg(1), emu.arg(2) as usize);
    let l = emu.mem.strlen(s);
    if n > 0 {
        let c = l.min(n - 1);
        emu.mem.copy_within(d, s, c);
        emu.mem.w8(d + c as u32, 0);
    }
    emu.ret(l as u32);
    Flow::Ret
}
fn strcat(emu: &mut Emu) -> Flow {
    let (d, s) = (emu.arg(0), emu.arg(1));
    let dl = emu.mem.strlen(d);
    let sl = emu.mem.strlen(s);
    emu.mem.copy_within(d + dl as u32, s, sl + 1);
    emu.ret(d);
    Flow::Ret
}
fn strncat(emu: &mut Emu) -> Flow {
    let (d, s, n) = (emu.arg(0), emu.arg(1), emu.arg(2) as usize);
    let dl = emu.mem.strlen(d);
    let sl = emu.mem.strlen(s).min(n);
    emu.mem.copy_within(d + dl as u32, s, sl);
    emu.mem.w8(d + (dl + sl) as u32, 0);
    emu.ret(d);
    Flow::Ret
}
fn strchr(emu: &mut Emu) -> Flow {
    let (s, c) = (emu.arg(0), emu.arg(1) as u8);
    let mut a = s;
    let r = loop {
        let b = emu.mem.r8(a);
        if b == c {
            break a;
        }
        if b == 0 {
            break 0;
        }
        a += 1;
    };
    emu.ret(r);
    Flow::Ret
}
fn strrchr(emu: &mut Emu) -> Flow {
    let (s, c) = (emu.arg(0), emu.arg(1) as u8);
    let n = emu.mem.strlen(s);
    let r = if c == 0 {
        s + n as u32
    } else {
        emu.mem.slice(s, n).iter().rposition(|&b| b == c).map(|i| s + i as u32).unwrap_or(0)
    };
    emu.ret(r);
    Flow::Ret
}
fn strstr(emu: &mut Emu) -> Flow {
    let (h, n) = (emu.arg(0), emu.arg(1));
    let hay = emu.mem.cstr_bytes(h).to_vec();
    let nee = emu.mem.cstr_bytes(n).to_vec();
    let r = if nee.is_empty() {
        h
    } else {
        hay.windows(nee.len()).position(|w| w == &nee[..]).map(|i| h + i as u32).unwrap_or(0)
    };
    emu.ret(r);
    Flow::Ret
}
fn strdup(emu: &mut Emu) -> Flow {
    let s = emu.arg(0);
    let n = emu.mem.strlen(s);
    let p = emu.hle.heap.malloc(&mut emu.mem, n as u32 + 1);
    emu.mem.copy_within(p, s, n + 1);
    emu.ret(p);
    Flow::Ret
}
fn strcspn(emu: &mut Emu) -> Flow {
    let s = emu.mem.cstr_bytes(emu.arg(0));
    let rej = emu.mem.cstr_bytes(emu.arg(1));
    let n = s.iter().position(|b| rej.contains(b)).unwrap_or(s.len());
    emu.ret(n as u32);
    Flow::Ret
}
fn strspn(emu: &mut Emu) -> Flow {
    let s = emu.mem.cstr_bytes(emu.arg(0));
    let acc = emu.mem.cstr_bytes(emu.arg(1));
    let n = s.iter().position(|b| !acc.contains(b)).unwrap_or(s.len());
    emu.ret(n as u32);
    Flow::Ret
}
fn strpbrk(emu: &mut Emu) -> Flow {
    let p = emu.arg(0);
    let s = emu.mem.cstr_bytes(p);
    let acc = emu.mem.cstr_bytes(emu.arg(1));
    let r = s.iter().position(|b| acc.contains(b)).map(|i| p + i as u32).unwrap_or(0);
    emu.ret(r);
    Flow::Ret
}
fn strtok(emu: &mut Emu) -> Flow {
    let mut s = emu.arg(0);
    let delim = emu.mem.cstr_bytes(emu.arg(1)).to_vec();
    if s == 0 {
        s = emu.hle.strtok_save;
    }
    if s == 0 {
        emu.ret(0);
        return Flow::Ret;
    }
    while emu.mem.r8(s) != 0 && delim.contains(&emu.mem.r8(s)) {
        s += 1;
    }
    if emu.mem.r8(s) == 0 {
        emu.hle.strtok_save = 0;
        emu.ret(0);
        return Flow::Ret;
    }
    let tok = s;
    while emu.mem.r8(s) != 0 && !delim.contains(&emu.mem.r8(s)) {
        s += 1;
    }
    if emu.mem.r8(s) != 0 {
        emu.mem.w8(s, 0);
        emu.hle.strtok_save = s + 1;
    } else {
        emu.hle.strtok_save = 0;
    }
    emu.ret(tok);
    Flow::Ret
}

/// Parse an integer like strtol. Returns (value, bytes consumed).
pub fn parse_int(b: &[u8], base: u32, signed: bool) -> (i64, usize, bool) {
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || (9..=13).contains(&b[i])) {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut base = base;
    if (base == 0 || base == 16) && i + 1 < b.len() && b[i] == b'0' && (b[i + 1] | 0x20) == b'x' && i + 2 < b.len() && (b[i + 2] as char).is_ascii_hexdigit() {
        i += 2;
        base = 16;
    } else if base == 0 && i < b.len() && b[i] == b'0' {
        base = 8;
    } else if base == 0 {
        base = 10;
    }
    let start = i;
    let mut v: u64 = 0;
    let mut overflow = false;
    while i < b.len() {
        let d = match (b[i] as char).to_digit(base) {
            Some(d) => d as u64,
            None => break,
        };
        v = match v.checked_mul(base as u64).and_then(|x| x.checked_add(d)) {
            Some(x) => x,
            None => {
                overflow = true;
                u64::MAX
            }
        };
        i += 1;
    }
    if i == start {
        return (0, 0, false);
    }
    let _ = signed;
    let r = if neg { (v as i64).wrapping_neg() } else { v as i64 };
    (r, i, overflow)
}

fn strtol(emu: &mut Emu) -> Flow {
    let (s, end, base) = (emu.arg(0), emu.arg(1), emu.arg(2));
    let b = emu.mem.cstr_bytes(s).to_vec();
    let (v, n, _) = parse_int(&b, base, true);
    if end != 0 {
        emu.mem.w32(end, s + n as u32);
    }
    let v = v.clamp(i32::MIN as i64, i32::MAX as i64);
    emu.ret(v as i32 as u32);
    Flow::Ret
}
fn strtoul(emu: &mut Emu) -> Flow {
    let (s, end, base) = (emu.arg(0), emu.arg(1), emu.arg(2));
    let b = emu.mem.cstr_bytes(s).to_vec();
    let (v, n, _) = parse_int(&b, base, false);
    if end != 0 {
        emu.mem.w32(end, s + n as u32);
    }
    emu.ret(v as u32);
    Flow::Ret
}
fn atoi(emu: &mut Emu) -> Flow {
    let b = emu.mem.cstr_bytes(emu.arg(0)).to_vec();
    let (v, _, _) = parse_int(&b, 10, true);
    emu.ret(v as i32 as u32);
    Flow::Ret
}

/// Parse a floating-point number prefix. Returns (value, bytes consumed).
pub fn parse_float(b: &[u8]) -> (f64, usize) {
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || (9..=13).contains(&b[i])) {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == ds || (i == ds + 1 && b[ds] == b'.') {
        // try inf/nan
        let rest = &b[ds..];
        let lower: Vec<u8> = rest.iter().take(3).map(|c| c.to_ascii_lowercase()).collect();
        if lower == b"inf" || lower == b"nan" {
            let neg = ds > start && b[start] == b'-';
            let v = if lower == b"inf" { f64::INFINITY } else { f64::NAN };
            return (if neg { -v } else { v }, ds + 3);
        }
        return (0.0, 0);
    }
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let s = std::str::from_utf8(&b[start..i]).unwrap_or("0");
    (s.parse::<f64>().unwrap_or(0.0), i)
}

fn strtod(emu: &mut Emu) -> Flow {
    let (s, end) = (emu.arg(0), emu.arg(1));
    let b = emu.mem.cstr_bytes(s).to_vec();
    let (v, n) = parse_float(&b);
    if end != 0 {
        emu.mem.w32(end, s + n as u32);
    }
    emu.ret_f64(v);
    Flow::Ret
}
fn atof(emu: &mut Emu) -> Flow {
    let b = emu.mem.cstr_bytes(emu.arg(0)).to_vec();
    let (v, _) = parse_float(&b);
    emu.ret_f64(v);
    Flow::Ret
}
fn strerror(emu: &mut Emu) -> Flow {
    let a = emu.hle.scratch + 3072;
    emu.mem.write_cstr(a, b"error");
    emu.ret(a);
    Flow::Ret
}

fn wcslen(emu: &mut Emu) -> Flow {
    let n = emu.mem.wstrlen(emu.arg(0));
    emu.ret(n as u32);
    Flow::Ret
}
fn wcscpy(emu: &mut Emu) -> Flow {
    let (d, s) = (emu.arg(0), emu.arg(1));
    let n = emu.mem.wstrlen(s);
    emu.mem.copy_within(d, s, (n + 1) * 4);
    emu.ret(d);
    Flow::Ret
}

fn qsort(emu: &mut Emu) -> Flow {
    let (base, n, size, cmp) = (emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3));
    if n < 2 || size == 0 {
        return Flow::Ret;
    }
    // Copy elements out, sort indices with the guest comparator, write back.
    let tmp_a = emu.hle.heap.malloc(&mut emu.mem, size);
    let tmp_b = emu.hle.heap.malloc(&mut emu.mem, size);
    let elems: Vec<Vec<u8>> = (0..n).map(|i| emu.mem.slice(base + i * size, size as usize).to_vec()).collect();
    let mut idx: Vec<usize> = (0..n as usize).collect();
    // merge sort (stable, comparator called O(n log n) times)
    let mut buf = idx.clone();
    fn msort(emu: &mut Emu, v: &mut [usize], buf: &mut [usize], elems: &[Vec<u8>], cmp: u32, ta: u32, tb: u32) {
        let n = v.len();
        if n < 2 {
            return;
        }
        let mid = n / 2;
        {
            let (l, r) = v.split_at_mut(mid);
            let (bl, br) = buf.split_at_mut(mid);
            msort(emu, l, bl, elems, cmp, ta, tb);
            msort(emu, r, br, elems, cmp, ta, tb);
        }
        let (mut i, mut j, mut k) = (0, mid, 0);
        while i < mid && j < n {
            emu.mem.write_bytes(ta, &elems[v[i]]);
            emu.mem.write_bytes(tb, &elems[v[j]]);
            let r = emu.call_nested(cmp, &[ta, tb]).0 as i32;
            if r <= 0 {
                buf[k] = v[i];
                i += 1;
            } else {
                buf[k] = v[j];
                j += 1;
            }
            k += 1;
        }
        while i < mid {
            buf[k] = v[i];
            i += 1;
            k += 1;
        }
        while j < n {
            buf[k] = v[j];
            j += 1;
            k += 1;
        }
        v.copy_from_slice(&buf[..n]);
    }
    msort(emu, &mut idx, &mut buf, &elems, cmp, tmp_a, tmp_b);
    for (i, &e) in idx.iter().enumerate() {
        emu.mem.write_bytes(base + i as u32 * size, &elems[e]);
    }
    emu.hle.heap.free(&mut emu.mem, tmp_a);
    emu.hle.heap.free(&mut emu.mem, tmp_b);
    Flow::Ret
}

// ---------------- process / errors ----------------

fn abort(emu: &mut Emu) -> Flow {
    let bt = emu.backtrace();
    Flow::Fault(format!("abort() called\n{}", bt))
}
fn exit(emu: &mut Emu) -> Flow {
    Flow::Fault(format!("exit({}) called", emu.arg(0) as i32))
}
fn assert2(emu: &mut Emu) -> Flow {
    let file = emu.mem.cstr(emu.arg(0));
    let line = emu.arg(1);
    let func = emu.mem.cstr(emu.arg(2));
    let expr = emu.mem.cstr(emu.arg(3));
    Flow::Fault(format!("assertion failed: {} at {}:{} ({})", expr, file, line, func))
}
fn stack_chk_fail(emu: &mut Emu) -> Flow {
    let bt = emu.backtrace();
    Flow::Fault(format!("stack smashing detected\n{}", bt))
}
fn pure_virtual(emu: &mut Emu) -> Flow {
    let bt = emu.backtrace();
    Flow::Fault(format!("pure virtual call\n{}", bt))
}
fn errno_loc(emu: &mut Emu) -> Flow {
    let a = super::thread::errno_addr(emu);
    emu.ret(a);
    Flow::Ret
}
fn cxa_guard_acquire(emu: &mut Emu) -> Flow {
    let g = emu.arg(0);
    let done = emu.mem.r8(g) != 0;
    emu.ret(if done { 0 } else { 1 });
    Flow::Ret
}
fn cxa_guard_release(emu: &mut Emu) -> Flow {
    let g = emu.arg(0);
    emu.mem.w32(g, 1);
    Flow::Ret
}

// ---------------- AEABI helpers ----------------

fn aeabi_idiv(emu: &mut Emu) -> Flow {
    let (a, b) = (emu.arg(0) as i32, emu.arg(1) as i32);
    emu.ret(if b == 0 { 0 } else { a.wrapping_div(b) as u32 });
    Flow::Ret
}
fn aeabi_idivmod(emu: &mut Emu) -> Flow {
    let (a, b) = (emu.arg(0) as i32, emu.arg(1) as i32);
    if b == 0 {
        emu.cpu.r[0] = 0;
        emu.cpu.r[1] = a as u32;
    } else {
        emu.cpu.r[0] = a.wrapping_div(b) as u32;
        emu.cpu.r[1] = a.wrapping_rem(b) as u32;
    }
    Flow::Ret
}
fn aeabi_uidiv(emu: &mut Emu) -> Flow {
    let (a, b) = (emu.arg(0), emu.arg(1));
    emu.ret(if b == 0 { 0 } else { a / b });
    Flow::Ret
}
fn aeabi_uidivmod(emu: &mut Emu) -> Flow {
    let (a, b) = (emu.arg(0), emu.arg(1));
    if b == 0 {
        emu.cpu.r[0] = 0;
        emu.cpu.r[1] = a;
    } else {
        emu.cpu.r[0] = a / b;
        emu.cpu.r[1] = a % b;
    }
    Flow::Ret
}
fn aeabi_ldivmod(emu: &mut Emu) -> Flow {
    let a = emu.arg64(0) as i64;
    let b = emu.arg64(2) as i64;
    let (q, r) = if b == 0 { (0, a) } else { (a.wrapping_div(b), a.wrapping_rem(b)) };
    emu.cpu.r[0] = q as u32;
    emu.cpu.r[1] = (q as u64 >> 32) as u32;
    emu.cpu.r[2] = r as u32;
    emu.cpu.r[3] = (r as u64 >> 32) as u32;
    Flow::Ret
}
fn aeabi_uldivmod(emu: &mut Emu) -> Flow {
    let a = emu.arg64(0);
    let b = emu.arg64(2);
    let (q, r) = if b == 0 { (0, a) } else { (a / b, a % b) };
    emu.cpu.r[0] = q as u32;
    emu.cpu.r[1] = (q >> 32) as u32;
    emu.cpu.r[2] = r as u32;
    emu.cpu.r[3] = (r >> 32) as u32;
    Flow::Ret
}
fn aeabi_ul2d(emu: &mut Emu) -> Flow {
    let a = emu.arg64(0);
    emu.ret_f64(a as f64);
    Flow::Ret
}

// ---------------- time ----------------

fn gettimeofday(emu: &mut Emu) -> Flow {
    let tv = emu.arg(0);
    let t = emu.plat.wall_time();
    if tv != 0 {
        emu.mem.w32(tv, t.floor() as u32);
        emu.mem.w32(tv + 4, ((t - t.floor()) * 1e6) as u32);
    }
    emu.ret(0);
    Flow::Ret
}
fn time(emu: &mut Emu) -> Flow {
    let p = emu.arg(0);
    let t = emu.plat.wall_time() as u32;
    if p != 0 {
        emu.mem.w32(p, t);
    }
    emu.ret(t);
    Flow::Ret
}
fn clock(emu: &mut Emu) -> Flow {
    let t = emu.plat.now_ms();
    emu.ret((t * 1000.0) as u32);
    Flow::Ret
}

fn days_to_civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn gmtime(emu: &mut Emu) -> Flow {
    let t = emu.mem.r32(emu.arg(0)) as i32 as i64;
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = days_to_civil(days);
    let tm = emu.hle.scratch + 2048;
    let wday = (days + 4).rem_euclid(7);
    let ydays_before = {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let cum = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        cum[(m - 1) as usize] + if leap && m > 2 { 1 } else { 0 }
    };
    let vals = [secs % 60, (secs / 60) % 60, secs / 3600, d as i64, m as i64 - 1, y - 1900, wday, ydays_before + d as i64 - 1, 0, 0, 0];
    for (i, v) in vals.iter().enumerate() {
        emu.mem.w32(tm + 4 * i as u32, *v as u32);
    }
    emu.ret(tm);
    Flow::Ret
}
fn mktime(emu: &mut Emu) -> Flow {
    let tm = emu.arg(0);
    let r = |i: u32| emu.mem.r32(tm + 4 * i) as i32 as i64;
    let (sec, min, hour, mday, mon, year) = (r(0), r(1), r(2), r(3), r(4), r(5) + 1900);
    let y = if mon < 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mon + 9) % 12;
    let doy = (153 * mp + 2) / 5 + mday - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let t = days * 86400 + hour * 3600 + min * 60 + sec;
    emu.ret(t as u32);
    Flow::Ret
}

fn nanosleep(emu: &mut Emu) -> Flow {
    let ts = emu.arg(0);
    let ms = emu.mem.r32(ts) as f64 * 1000.0 + emu.mem.r32(ts + 4) as f64 / 1e6;
    super::thread::sleep_ms(emu, ms)
}
fn usleep(emu: &mut Emu) -> Flow {
    let ms = emu.arg(0) as f64 / 1000.0;
    super::thread::sleep_ms(emu, ms)
}
fn sleep(emu: &mut Emu) -> Flow {
    let ms = emu.arg(0) as f64 * 1000.0;
    super::thread::sleep_ms(emu, ms)
}

fn sysconf(emu: &mut Emu) -> Flow {
    let v = match emu.arg(0) {
        0x27 => 4096, // _SC_PAGESIZE
        0x60 | 0x61 => 1, // NPROCESSORS
        6 => 100,        // CLK_TCK
        _ => 0,
    };
    emu.ret(v);
    Flow::Ret
}

fn lrand48(emu: &mut Emu) -> Flow {
    let x = emu.hle.rand48.wrapping_mul(0x5DEECE66D).wrapping_add(0xB) & 0xFFFF_FFFF_FFFF;
    emu.hle.rand48 = x;
    emu.ret((x >> 17) as u32);
    Flow::Ret
}
fn rand(emu: &mut Emu) -> Flow {
    lrand48(emu)
}
fn srand48(emu: &mut Emu) -> Flow {
    emu.hle.rand48 = ((emu.arg(0) as u64) << 16) | 0x330E;
    Flow::Ret
}

// jmp_buf layout (ours): r4-r11, sp, lr, d8-d15, fpscr
fn setjmp(emu: &mut Emu) -> Flow {
    let b = emu.arg(0);
    for i in 0..8 {
        emu.mem.w32(b + 4 * i, emu.cpu.r[4 + i as usize]);
    }
    emu.mem.w32(b + 32, emu.cpu.r[13]);
    emu.mem.w32(b + 36, emu.cpu.r[14]);
    for i in 0..8u32 {
        emu.mem.w64(b + 40 + 8 * i, emu.cpu.d_bits(8 + i as u8));
    }
    emu.mem.w32(b + 104, emu.cpu.fpscr);
    emu.ret(0);
    Flow::Ret
}
fn longjmp(emu: &mut Emu) -> Flow {
    let b = emu.arg(0);
    let v = emu.arg(1);
    for i in 0..8 {
        emu.cpu.r[4 + i as usize] = emu.mem.r32(b + 4 * i);
    }
    emu.cpu.r[13] = emu.mem.r32(b + 32);
    emu.cpu.r[14] = emu.mem.r32(b + 36);
    for i in 0..8u32 {
        let d = emu.mem.r64(b + 40 + 8 * i);
        emu.cpu.set_d_bits(8 + i as u8, d);
    }
    emu.cpu.fpscr = emu.mem.r32(b + 104);
    emu.ret(if v == 0 { 1 } else { v });
    Flow::Ret
}

fn setlocale(emu: &mut Emu) -> Flow {
    let a = emu.hle.scratch + 3584;
    emu.mem.write_cstr(a, b"C");
    emu.ret(a);
    Flow::Ret
}
fn getpid(emu: &mut Emu) -> Flow {
    emu.ret(1234);
    Flow::Ret
}
fn gethostname(emu: &mut Emu) -> Flow {
    let (p, n) = (emu.arg(0), emu.arg(1));
    if n > 9 {
        emu.mem.write_cstr(p, b"localhost");
    }
    emu.ret(0);
    Flow::Ret
}

fn mmap(emu: &mut Emu) -> Flow {
    let (_addr, len, _prot, flags, fd, off) = (emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3), emu.arg(4) as i32, emu.arg(5));
    let p = emu.hle.heap.memalign(&mut emu.mem, 4096, len);
    if p == 0 {
        emu.ret(u32::MAX);
        return Flow::Ret;
    }
    emu.mem.fill(p, 0, len as usize);
    if flags & 0x20 == 0 && fd >= 0 {
        super::stdio::pread_into(emu, fd, p, len, off as u64);
    }
    emu.ret(p);
    Flow::Ret
}
fn munmap(emu: &mut Emu) -> Flow {
    let p = emu.arg(0);
    emu.hle.heap.free(&mut emu.mem, p);
    emu.ret(0);
    Flow::Ret
}

fn inet_addr(emu: &mut Emu) -> Flow {
    let s = emu.mem.cstr(emu.arg(0));
    let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
    let v = if parts.len() == 4 { parts[0] | parts[1] << 8 | parts[2] << 16 | parts[3] << 24 } else { u32::MAX };
    emu.ret(v);
    Flow::Ret
}
fn inet_ntoa(emu: &mut Emu) -> Flow {
    let v = emu.arg(0);
    let s = format!("{}.{}.{}.{}", v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, v >> 24);
    let a = emu.hle.scratch + 3840;
    emu.mem.write_cstr(a, s.as_bytes());
    emu.ret(a);
    Flow::Ret
}

// ---------------- logging ----------------

fn prio_char(p: u32) -> char {
    match p {
        2 => 'V',
        3 => 'D',
        4 => 'I',
        5 => 'W',
        6 => 'E',
        7 => 'F',
        _ => '?',
    }
}
fn android_log_print(emu: &mut Emu) -> Flow {
    let prio = emu.arg(0);
    let tag = emu.mem.cstr(emu.arg(1));
    let fmt_addr = emu.arg(2);
    let mut va = VaArgs::from_call(emu, 3);
    let s = super::fmt::format(emu, fmt_addr, &mut va);
    let msg = format!("{}/{}: {}", prio_char(prio), tag, String::from_utf8_lossy(&s).trim_end());
    emu.log(&msg);
    emu.ret(0);
    Flow::Ret
}
fn android_log_vprint(emu: &mut Emu) -> Flow {
    let prio = emu.arg(0);
    let tag = emu.mem.cstr(emu.arg(1));
    let fmt_addr = emu.arg(2);
    let mut va = VaArgs::from_va_list(emu.arg(3));
    let s = super::fmt::format(emu, fmt_addr, &mut va);
    let msg = format!("{}/{}: {}", prio_char(prio), tag, String::from_utf8_lossy(&s).trim_end());
    emu.log(&msg);
    emu.ret(0);
    Flow::Ret
}
fn android_log_write(emu: &mut Emu) -> Flow {
    let prio = emu.arg(0);
    let tag = emu.mem.cstr(emu.arg(1));
    let text = emu.mem.cstr(emu.arg(2));
    let msg = format!("{}/{}: {}", prio_char(prio), tag, text.trim_end());
    emu.log(&msg);
    emu.ret(0);
    Flow::Ret
}
