//! Software decoders for GPU texture formats browsers don't support
//! (PVRTC 2/4bpp, ETC1). Output is tightly packed RGBA8.

#[derive(Clone, Copy, Default)]
struct Px {
    r: i32,
    g: i32,
    b: i32,
    a: i32,
}

fn color_a(c: u32) -> Px {
    if c & 0x8000 != 0 {
        Px { r: ((c & 0x7c00) >> 10) as i32, g: ((c & 0x3e0) >> 5) as i32, b: ((c & 0x1e) | ((c & 0x1e) >> 4)) as i32, a: 0xf }
    } else {
        Px {
            r: (((c & 0xf00) >> 7) | ((c & 0xf00) >> 11)) as i32,
            g: (((c & 0xf0) >> 3) | ((c & 0xf0) >> 7)) as i32,
            b: (((c & 0xe) << 1) | ((c & 0xe) >> 2)) as i32,
            a: ((c & 0x7000) >> 11) as i32,
        }
    }
}

fn color_b(c: u32) -> Px {
    if c & 0x8000_0000 != 0 {
        Px { r: ((c & 0x7c00_0000) >> 26) as i32, g: ((c & 0x3e0_0000) >> 21) as i32, b: ((c & 0x1f_0000) >> 16) as i32, a: 0xf }
    } else {
        Px {
            r: (((c & 0xf00_0000) >> 23) | ((c & 0xf00_0000) >> 27)) as i32,
            g: (((c & 0xf0_0000) >> 19) | ((c & 0xf0_0000) >> 23)) as i32,
            b: (((c & 0xf_0000) >> 15) | ((c & 0xf_0000) >> 19)) as i32,
            a: ((c & 0x7000_0000) >> 27) as i32,
        }
    }
}

/// Bilinearly upscale four block colours over a ww x 4 pixel grid.
fn interpolate(p: Px, q: Px, r: Px, s: Px, out: &mut [Px; 32], two_bpp: bool) {
    let ww: i32 = if two_bpp { 8 } else { 4 };
    let wh: i32 = 4;
    let mut hp = Px { r: p.r * ww, g: p.g * ww, b: p.b * ww, a: p.a * ww };
    let mut hr = Px { r: r.r * ww, g: r.g * ww, b: r.b * ww, a: r.a * ww };
    let qp = Px { r: q.r - p.r, g: q.g - p.g, b: q.b - p.b, a: q.a - p.a };
    let sr = Px { r: s.r - r.r, g: s.g - r.g, b: s.b - r.b, a: s.a - r.a };
    for x in 0..ww {
        let mut res = Px { r: 4 * hp.r, g: 4 * hp.g, b: 4 * hp.b, a: 4 * hp.a };
        let dy = Px { r: hr.r - hp.r, g: hr.g - hp.g, b: hr.b - hp.b, a: hr.a - hp.a };
        for y in 0..wh {
            let o = &mut out[(y * ww + x) as usize];
            if two_bpp {
                o.r = (res.r >> 7) + (res.r >> 2);
                o.g = (res.g >> 7) + (res.g >> 2);
                o.b = (res.b >> 7) + (res.b >> 2);
                o.a = (res.a >> 5) + (res.a >> 1);
            } else {
                o.r = (res.r >> 6) + (res.r >> 1);
                o.g = (res.g >> 6) + (res.g >> 1);
                o.b = (res.b >> 6) + (res.b >> 1);
                o.a = (res.a >> 4) + res.a;
            }
            res.r += dy.r;
            res.g += dy.g;
            res.b += dy.b;
            res.a += dy.a;
        }
        hp.r += qp.r;
        hp.g += qp.g;
        hp.b += qp.b;
        hp.a += qp.a;
        hr.r += sr.r;
        hr.g += sr.g;
        hr.b += sr.b;
        hr.a += sr.a;
    }
}

/// Modulation values/modes for a 2x2 word neighbourhood, indexed [y][x].
struct Mods {
    val: [[i32; 16]; 8],
    mode: [[i32; 16]; 8],
}

fn unpack_mods(moddata: u32, colordata: u32, ox: usize, oy: usize, m: &mut Mods, two_bpp: bool) {
    let mut mode = (colordata & 1) as i32;
    let mut bits = moddata;
    if two_bpp {
        if mode != 0 {
            if bits & 1 != 0 {
                mode = if bits & (1 << 20) != 0 { 3 } else { 2 };
                if bits & (1 << 21) != 0 {
                    bits |= 1 << 20;
                } else {
                    bits &= !(1 << 20);
                }
            }
            if bits & 2 != 0 {
                bits |= 1;
            } else {
                bits &= !1;
            }
            for y in 0..4 {
                for x in 0..8 {
                    m.mode[oy + y][ox + x] = mode;
                    if (x ^ y) & 1 == 0 {
                        m.val[oy + y][ox + x] = (bits & 3) as i32;
                        bits >>= 2;
                    }
                }
            }
        } else {
            for y in 0..4 {
                for x in 0..8 {
                    m.mode[oy + y][ox + x] = 0;
                    m.val[oy + y][ox + x] = if bits & 1 != 0 { 3 } else { 0 };
                    bits >>= 1;
                }
            }
        }
    } else if mode != 0 {
        for y in 0..4 {
            for x in 0..4 {
                let v = (bits & 3) as i32;
                m.val[oy + y][ox + x] = match v {
                    1 => 4,
                    2 => 14,
                    3 => 8,
                    _ => 0,
                };
                bits >>= 2;
            }
        }
    } else {
        for y in 0..4 {
            for x in 0..4 {
                let mut v = (bits & 3) as i32 * 3;
                if v > 3 {
                    v -= 1;
                }
                m.val[oy + y][ox + x] = v;
                bits >>= 2;
            }
        }
    }
}

fn mod_value(m: &Mods, x: usize, y: usize, two_bpp: bool) -> i32 {
    if !two_bpp {
        return m.val[y][x];
    }
    const REP: [i32; 4] = [0, 3, 5, 8];
    let mode = m.mode[y][x];
    if mode == 0 || (x ^ y) & 1 == 0 {
        return REP[m.val[y][x] as usize];
    }
    match mode {
        1 => (REP[m.val[y - 1][x] as usize] + REP[m.val[y + 1][x] as usize] + REP[m.val[y][x - 1] as usize] + REP[m.val[y][x + 1] as usize] + 2) / 4,
        2 => (REP[m.val[y][x - 1] as usize] + REP[m.val[y][x + 1] as usize] + 1) / 2,
        _ => (REP[m.val[y - 1][x] as usize] + REP[m.val[y + 1][x] as usize] + 1) / 2,
    }
}

fn twiddle(ysize: u32, xsize: u32, ypos: u32, xpos: u32) -> u32 {
    let (mut min_dim, mut max_val) = (xsize, ypos);
    if ysize < xsize {
        min_dim = ysize;
        max_val = xpos;
    }
    let (mut tw, mut src, mut dst, mut shift) = (0u32, 1u32, 1u32, 0u32);
    while src < min_dim {
        if ypos & src != 0 {
            tw |= dst;
        }
        if xpos & src != 0 {
            tw |= dst << 1;
        }
        src <<= 1;
        dst <<= 2;
        shift += 1;
    }
    max_val >>= shift;
    tw | (max_val << (2 * shift))
}

/// Decode PVRTC1 data to RGBA8 (width*height*4 bytes).
pub fn decode_pvrtc(data: &[u8], width: u32, height: u32, two_bpp: bool) -> Vec<u8> {
    let ww: u32 = if two_bpp { 8 } else { 4 };
    let wh: u32 = 4;
    let pw = width.max(if two_bpp { 16 } else { 8 });
    let ph = height.max(8);
    let nx = (pw / ww).max(2);
    let ny = (ph / wh).max(2);
    let mut out = vec![0u8; (pw * ph * 4) as usize];
    let word = |idx: u32| -> (u32, u32) {
        let o = idx as usize * 8;
        if o + 8 > data.len() {
            return (0, 0);
        }
        (u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]), u32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]))
    };
    let mut up_a = [Px::default(); 32];
    let mut up_b = [Px::default(); 32];
    let mut mods = Mods { val: [[0; 16]; 8], mode: [[0; 16]; 8] };
    for wy in 0..ny {
        for wx in 0..nx {
            let x1 = (wx + 1) % nx;
            let y1 = (wy + 1) % ny;
            let ip = twiddle(ny, nx, wy, wx);
            let iq = twiddle(ny, nx, wy, x1);
            let ir = twiddle(ny, nx, y1, wx);
            let is = twiddle(ny, nx, y1, x1);
            let (pm, pc) = word(ip);
            let (qm, qc) = word(iq);
            let (rm, rc) = word(ir);
            let (sm, sc) = word(is);
            interpolate(color_a(pc), color_a(qc), color_a(rc), color_a(sc), &mut up_a, two_bpp);
            interpolate(color_b(pc), color_b(qc), color_b(rc), color_b(sc), &mut up_b, two_bpp);
            unpack_mods(pm, pc, 0, 0, &mut mods, two_bpp);
            unpack_mods(qm, qc, ww as usize, 0, &mut mods, two_bpp);
            unpack_mods(rm, rc, 0, wh as usize, &mut mods, two_bpp);
            unpack_mods(sm, sc, ww as usize, wh as usize, &mut mods, two_bpp);
            for y in 0..wh {
                for x in 0..ww {
                    let mut m = mod_value(&mods, (x + ww / 2) as usize, (y + wh / 2) as usize, two_bpp);
                    let punch = m > 10;
                    if punch {
                        m -= 10;
                    }
                    let i = (y * ww + x) as usize;
                    let a = up_a[i];
                    let b = up_b[i];
                    let r = (a.r * (8 - m) + b.r * m) / 8;
                    let g = (a.g * (8 - m) + b.g * m) / 8;
                    let bl = (a.b * (8 - m) + b.b * m) / 8;
                    let al = if punch { 0 } else { (a.a * (8 - m) + b.a * m) / 8 };
                    let px = (wx * ww + x + ww / 2) % pw;
                    let py = (wy * wh + y + wh / 2) % ph;
                    let o = ((py * pw + px) * 4) as usize;
                    out[o] = r.clamp(0, 255) as u8;
                    out[o + 1] = g.clamp(0, 255) as u8;
                    out[o + 2] = bl.clamp(0, 255) as u8;
                    out[o + 3] = al.clamp(0, 255) as u8;
                }
            }
        }
    }
    if pw == width && ph == height {
        return out;
    }
    let mut crop = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let s = (y * pw * 4) as usize;
        let d = (y * width * 4) as usize;
        crop[d..d + (width * 4) as usize].copy_from_slice(&out[s..s + (width * 4) as usize]);
    }
    crop
}

pub fn pvrtc_size(width: u32, height: u32, two_bpp: bool) -> usize {
    if two_bpp {
        (width.max(16) * height.max(8) / 4) as usize
    } else {
        (width.max(8) * height.max(8) / 2) as usize
    }
}

/// Decode ETC1 RGB to RGBA8.
pub fn decode_etc1(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    const TABLE: [[i32; 4]; 8] = [[2, 8, -2, -8], [5, 17, -5, -17], [9, 29, -9, -29], [13, 42, -13, -42], [18, 60, -18, -60], [24, 80, -24, -80], [33, 106, -33, -106], [47, 183, -47, -183]];
    let bw = (width + 3) / 4;
    let bh = (height + 3) / 4;
    let mut out = vec![0u8; (width * height * 4) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let o = ((by * bw + bx) * 8) as usize;
            if o + 8 > data.len() {
                break;
            }
            let hi = u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            let lo = u32::from_be_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]);
            let diff = hi & 2 != 0;
            let flip = hi & 1 != 0;
            let (c1, c2): ([i32; 3], [i32; 3]) = if diff {
                let r = (hi >> 27) as i32;
                let g = ((hi >> 19) & 0x1f) as i32;
                let b = ((hi >> 11) & 0x1f) as i32;
                let sx = |v: u32| ((v as i32) << 29) >> 29;
                let dr = sx(hi >> 24);
                let dg = sx(hi >> 16);
                let db = sx(hi >> 8);
                let e5 = |v: i32| (v << 3) | (v >> 2);
                ([e5(r), e5(g), e5(b)], [e5((r + dr) & 31), e5((g + dg) & 31), e5((b + db) & 31)])
            } else {
                let e4 = |v: u32| ((v & 0xf) as i32) * 17;
                ([e4(hi >> 28), e4(hi >> 20), e4(hi >> 12)], [e4(hi >> 24), e4(hi >> 16), e4(hi >> 8)])
            };
            let t1 = ((hi >> 5) & 7) as usize;
            let t2 = ((hi >> 2) & 7) as usize;
            for i in 0..16u32 {
                let x = i / 4;
                let y = i % 4;
                let second = if flip { y >= 2 } else { x >= 2 };
                let (c, t) = if second { (c2, t2) } else { (c1, t1) };
                let msb = (lo >> (16 + i)) & 1;
                let lsb = (lo >> i) & 1;
                let idx = match (msb, lsb) {
                    (0, 0) => 0,
                    (0, 1) => 1,
                    (1, 0) => 2,
                    _ => 3,
                };
                let d = TABLE[t][idx];
                let px = bx * 4 + x;
                let py = by * 4 + y;
                if px >= width || py >= height {
                    continue;
                }
                let oo = ((py * width + px) * 4) as usize;
                out[oo] = (c[0] + d).clamp(0, 255) as u8;
                out[oo + 1] = (c[1] + d).clamp(0, 255) as u8;
                out[oo + 2] = (c[2] + d).clamp(0, 255) as u8;
                out[oo + 3] = 255;
            }
        }
    }
    out
}
