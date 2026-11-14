//! POSIX socket imports transported through the browser's room relay.
use super::{Flow, HleFn};
use crate::Emu;
use std::collections::HashMap;

pub fn call(emu: &mut Emu, op: u32) -> Flow {
    let mut args = [emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3), emu.arg(4), emu.arg(5)];
    if op == 18 { args[5] = emu.hle.scratch + 3200; }
    if op == 20 {
        args[5] = emu.hle.heap.malloc(&mut emu.mem, 64);
        if args[5] == 0 { emu.ret((-10i32) as u32); return Flow::Ret; }
    }
    let result = emu.plat.net(op, &args, &mut emu.mem);
    if op == 20 {
        if result != 0 { emu.hle.heap.free(&mut emu.mem, args[5]); }
        emu.ret(result as u32);
        return Flow::Ret;
    }
    if result < 0 {
        super::libc::set_errno(emu, (-result) as u32);
        emu.ret(u32::MAX);
    } else { emu.ret(result as u32); }
    Flow::Ret
}

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    r.insert("socket", |e| call(e, 0));
    r.insert("bind", |e| call(e, 1));
    r.insert("connect", |e| call(e, 2));
    r.insert("listen", |e| call(e, 3));
    r.insert("accept", |e| call(e, 4));
    r.insert("send", |e| call(e, 5));
    r.insert("sendto", |e| call(e, 6));
    r.insert("recv", |e| call(e, 7));
    r.insert("recvfrom", |e| call(e, 8));
    r.insert("setsockopt", |e| call(e, 9));
    r.insert("getsockopt", |e| call(e, 10));
    r.insert("getsockname", |e| call(e, 11));
    r.insert("select", |e| call(e, 12));
    r.insert("fcntl", |e| if e.arg(0) >= 512 { call(e, 13) } else { e.ret(0); Flow::Ret });
    r.insert("ioctl", |e| call(e, 14));
    r.insert("gethostbyname", |e| call(e, 18));
    r.insert("getaddrinfo", |e| call(e, 20));
    r.insert("freeaddrinfo", |e| { let p = e.arg(0); e.hle.heap.free(&mut e.mem, p); e.ret(0); Flow::Ret });
    r.insert("gethostname", |e| call(e, 19));
    r.insert("shutdown", |e| call(e, 15));
}
