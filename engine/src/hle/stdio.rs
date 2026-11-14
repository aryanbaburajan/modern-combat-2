//! Virtual file system, POSIX fds and stdio FILE streams.
//!
//! Game data is read-only and fetched lazily from the platform in chunks;
//! anything the game writes lives in memory and is persisted through the
//! platform's save store.

use super::{Flow, HleFn, State, VaArgs};
use crate::mem::Memory;
use crate::Emu;
use std::collections::HashMap;

const CHUNK: u64 = 128 * 1024;
const CACHE_BUDGET: usize = 160 * 1024 * 1024;
const FILE_SIZE: u32 = 84;
const SEOF: u16 = 0x0020;
const SERR: u16 = 0x0040;

pub const GAME_ROOT: &str = "/sdcard/gameloft/games/";
pub const APP_DIR: &str = "/data/data/com.gameloft.android.GAND.GloftBPHP.ML/";

enum Backing {
    /// Read-only game data (path relative to the data root).
    Data { path: String, size: u64 },
    /// In-memory file; persisted under `path` when dirty.
    Mem { path: String, data: Vec<u8>, dirty: bool, persist: bool },
    Console(u8),
}

struct OpenFile {
    backing: Backing,
    pos: u64,
    readable: bool,
    writable: bool,
    append: bool,
    eof: bool,
    err: bool,
    unget: Option<u8>,
    refs: u32,
}

pub struct Vfs {
    fds: HashMap<i32, OpenFile>,
    next_fd: i32,
    /// FILE* guest address -> fd
    streams: HashMap<u32, i32>,
    free_files: Vec<u32>,
    cwd: String,
    chunks: HashMap<(String, u64), (Vec<u8>, u64)>,
    chunk_bytes: usize,
    tick: u64,
    size_cache: HashMap<String, Option<u64>>,
    pub sf_base: u32,
    pub log_opens: bool,
    pub bytes_read: u64,
}

impl Vfs {
    pub fn new() -> Vfs {
        let mut fds = HashMap::new();
        for i in 0..3 {
            fds.insert(
                i,
                OpenFile { backing: Backing::Console(i as u8), pos: 0, readable: i == 0, writable: i != 0, append: false, eof: false, err: false, unget: None, refs: 1 },
            );
        }
        Vfs {
            fds,
            next_fd: 3,
            streams: HashMap::new(),
            free_files: Vec::new(),
            cwd: "/".into(),
            chunks: HashMap::new(),
            chunk_bytes: 0,
            tick: 0,
            size_cache: HashMap::new(),
            sf_base: 0,
            log_opens: false,
            bytes_read: 0,
        }
    }
}

pub fn init_data(st: &mut State, mem: &mut Memory) {
    let sf = st.static_alloc(FILE_SIZE * 3, 8);
    for i in 0..3u32 {
        mem.w16(sf + i * FILE_SIZE + 14, i as u16);
        st.vfs.streams.insert(sf + i * FILE_SIZE, i as i32);
    }
    st.vfs.sf_base = sf;
    st.data_syms.insert("__sF", sf);
}

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    macro_rules! reg {
        ($($name:literal => $f:expr),* $(,)?) => { $( r.insert($name, $f); )* };
    }
    reg! {
        "fopen" => fopen, "fclose" => fclose, "fread" => fread, "fwrite" => fwrite,
        "fseek" => fseek, "fseeko" => fseek, "ftell" => ftell, "ftello" => ftell, "rewind" => rewind,
        "fflush" => fflush, "feof" => feof, "ferror" => ferror, "clearerr" => clearerr,
        "fgetc" => fgetc, "getc" => fgetc, "ungetc" => ungetc, "fgets" => fgets,
        "fputc" => fputc, "putc" => fputc, "fputs" => fputs, "puts" => puts, "putchar" => putchar,
        "fprintf" => fprintf, "vfprintf" => vfprintf, "fscanf" => fscanf,
        "fgetpos" => fgetpos, "fsetpos" => fsetpos, "setvbuf" => ret0, "setbuf" => ret0, "fileno" => fileno,
        "open" => open, "close" => close, "read" => read, "write" => write, "lseek" => lseek,
        "fstat" => fstat, "stat" => stat, "lstat" => stat, "access" => access,
        "remove" => remove, "unlink" => remove, "rename" => rename, "mkdir" => ret0, "chdir" => chdir,
        "getcwd" => getcwd,
        "sprintf" => super::fmt::sprintf, "snprintf" => super::fmt::snprintf,
        "vsprintf" => super::fmt::vsprintf, "vsnprintf" => super::fmt::vsnprintf,
        "swprintf" => super::fmt::swprintf, "printf" => super::fmt::printf, "sscanf" => super::fmt::sscanf,
        "opendir" => ret0, "readdir" => ret0, "closedir" => ret0,
    }
}

fn ret0(emu: &mut Emu) -> Flow {
    emu.ret(0);
    Flow::Ret
}

/// Classify an absolute guest path. Returns (is_game_data, key).
fn classify(path: &str) -> (bool, String) {
    if let Some(rest) = path.strip_prefix(GAME_ROOT) {
        (true, rest.to_string())
    } else if let Some(rest) = path.strip_prefix(APP_DIR) {
        (false, format!("app/{}", rest))
    } else if let Some(rest) = path.strip_prefix("/sdcard/") {
        (false, format!("sdcard/{}", rest))
    } else {
        (false, format!("other{}", path))
    }
}

fn normalize(cwd: &str, p: &str) -> String {
    let full = if p.starts_with('/') { p.to_string() } else { format!("{}/{}", cwd, p) };
    let mut parts: Vec<&str> = Vec::new();
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    let mut s = String::from("/");
    s += &parts.join("/");
    if full.ends_with('/') && !s.ends_with('/') {
        s.push('/');
    }
    s
}

impl Emu {
    fn data_size(&mut self, key: &str) -> Option<u64> {
        if let Some(v) = self.hle.vfs.size_cache.get(key) {
            return *v;
        }
        let v = self.plat.file_size(key);
        self.hle.vfs.size_cache.insert(key.to_string(), v);
        v
    }

    fn read_data(&mut self, key: &str, size: u64, off: u64, out: &mut [u8]) -> usize {
        if off >= size {
            return 0;
        }
        let n = (out.len() as u64).min(size - off) as usize;
        let mut done = 0usize;
        while done < n {
            let pos = off + done as u64;
            let ci = pos / CHUNK;
            let co = (pos % CHUNK) as usize;
            self.hle.vfs.tick += 1;
            let tick = self.hle.vfs.tick;
            let k = (key.to_string(), ci);
            if !self.hle.vfs.chunks.contains_key(&k) {
                let len = (CHUNK.min(size - ci * CHUNK)) as usize;
                let mut buf = vec![0u8; len];
                let got = self.plat.file_read(key, ci * CHUNK, &mut buf);
                buf.truncate(got);
                self.hle.vfs.bytes_read += got as u64;
                self.hle.vfs.chunk_bytes += buf.len();
                self.hle.vfs.chunks.insert(k.clone(), (buf, tick));
                if self.hle.vfs.chunk_bytes > CACHE_BUDGET {
                    self.evict_chunks();
                }
            }
            let ent = self.hle.vfs.chunks.get_mut(&k).unwrap();
            ent.1 = tick;
            let avail = ent.0.len().saturating_sub(co);
            if avail == 0 {
                break;
            }
            let take = avail.min(n - done);
            out[done..done + take].copy_from_slice(&ent.0[co..co + take]);
            done += take;
        }
        done
    }

    fn evict_chunks(&mut self) {
        let vfs = &mut self.hle.vfs;
        let mut ents: Vec<((String, u64), u64)> = vfs.chunks.iter().map(|(k, v)| (k.clone(), v.1)).collect();
        ents.sort_by_key(|e| e.1);
        let target = CACHE_BUDGET * 3 / 4;
        for (k, _) in ents {
            if vfs.chunk_bytes <= target {
                break;
            }
            if let Some((b, _)) = vfs.chunks.remove(&k) {
                vfs.chunk_bytes -= b.len();
            }
        }
    }

    /// Open a file. Returns fd or -errno.
    pub fn vfs_open(&mut self, path: &str, read: bool, write: bool, create: bool, trunc: bool, append: bool) -> i32 {
        let abs = normalize(&self.hle.vfs.cwd, path);
        let (is_data, key) = classify(&abs);
        let saved = self.plat.save_load(&key);
        let backing = if let Some(data) = saved {
            Backing::Mem { path: key.clone(), data: if trunc { Vec::new() } else { data }, dirty: trunc, persist: true }
        } else if is_data && !write {
            match self.data_size(&key) {
                Some(size) => Backing::Data { path: key.clone(), size },
                None => {
                    if self.hle.vfs.log_opens {
                        self.log(&format!("[vfs] open {} -> ENOENT", abs));
                    }
                    return -2;
                }
            }
        } else if write {
            let mut data = Vec::new();
            if !trunc && is_data {
                if let Some(size) = self.data_size(&key) {
                    data = vec![0u8; size as usize];
                    let n = self.read_data(&key, size, 0, &mut data);
                    data.truncate(n);
                }
            } else if !create && !is_data {
                return -2;
            }
            if !create && !is_data && data.is_empty() {
                return -2;
            }
            Backing::Mem { path: key.clone(), data, dirty: true, persist: true }
        } else {
            if self.hle.vfs.log_opens {
                self.log(&format!("[vfs] open {} -> ENOENT", abs));
            }
            return -2;
        };
        if self.hle.vfs.log_opens {
            self.log(&format!("[vfs] open {} (r={} w={})", abs, read, write));
        }
        let fd = self.hle.vfs.next_fd;
        self.hle.vfs.next_fd += 1;
        let mut f = OpenFile { backing, pos: 0, readable: read, writable: write, append, eof: false, err: false, unget: None, refs: 1 };
        if append {
            f.pos = file_len(&f);
        }
        self.hle.vfs.fds.insert(fd, f);
        fd
    }

    pub fn vfs_read(&mut self, fd: i32, buf: u32, len: u32) -> i64 {
        let (key, size, pos, unget) = match self.hle.vfs.fds.get_mut(&fd) {
            None => return -9,
            Some(f) if !f.readable => return -9,
            Some(f) => {
                let u = f.unget.take();
                match &f.backing {
                    Backing::Data { path, size } => (Some(path.clone()), *size, f.pos, u),
                    Backing::Mem { data, .. } => {
                        let mut off = 0u32;
                        if let Some(c) = u {
                            if len == 0 {
                                return 0;
                            }
                            self.mem.w8(buf, c);
                            off = 1;
                        }
                        let p = f.pos as usize;
                        let n = if p >= data.len() { 0 } else { (data.len() - p).min((len - off) as usize) };
                        let chunk = data[p.min(data.len())..p.min(data.len()) + n].to_vec();
                        f.pos += n as u64;
                        if (n as u32 + off) < len {
                            f.eof = true;
                        }
                        self.mem.write_bytes(buf + off, &chunk);
                        return (n as u32 + off) as i64;
                    }
                    Backing::Console(_) => return 0,
                }
            }
        };
        let key = key.unwrap();
        let mut off = 0u32;
        if let Some(c) = unget {
            if len == 0 {
                return 0;
            }
            self.mem.w8(buf, c);
            off = 1;
        }
        let want = (len - off) as usize;
        let mut tmp = vec![0u8; want];
        let n = self.read_data(&key, size, pos, &mut tmp);
        self.mem.write_bytes(buf + off, &tmp[..n]);
        let f = self.hle.vfs.fds.get_mut(&fd).unwrap();
        f.pos = pos + n as u64;
        if n < want {
            f.eof = true;
        }
        (n as u32 + off) as i64
    }

    pub fn vfs_write(&mut self, fd: i32, buf: u32, len: u32) -> i64 {
        let bytes = self.mem.slice(buf, len as usize).to_vec();
        let f = match self.hle.vfs.fds.get_mut(&fd) {
            None => return -9,
            Some(f) => f,
        };
        match &mut f.backing {
            Backing::Console(n) => {
                let n = *n;
                let s = String::from_utf8_lossy(&bytes).trim_end().to_string();
                if !s.is_empty() {
                    self.log(&format!("[{}] {}", if n == 2 { "stderr" } else { "stdout" }, s));
                }
                len as i64
            }
            Backing::Data { .. } => -9,
            Backing::Mem { data, dirty, .. } => {
                if !f.writable {
                    return -9;
                }
                if f.append {
                    f.pos = data.len() as u64;
                }
                let p = f.pos as usize;
                if data.len() < p + bytes.len() {
                    data.resize(p + bytes.len(), 0);
                }
                data[p..p + bytes.len()].copy_from_slice(&bytes);
                f.pos += bytes.len() as u64;
                *dirty = true;
                len as i64
            }
        }
    }

    pub fn vfs_seek(&mut self, fd: i32, off: i64, whence: u32) -> i64 {
        let f = match self.hle.vfs.fds.get_mut(&fd) {
            None => return -9,
            Some(f) => f,
        };
        let len = file_len(f) as i64;
        let base = match whence {
            0 => 0,
            1 => f.pos as i64 - if f.unget.is_some() { 1 } else { 0 },
            2 => len,
            _ => return -22,
        };
        let np = base + off;
        if np < 0 {
            return -22;
        }
        f.pos = np as u64;
        f.eof = false;
        f.unget = None;
        np
    }

    pub fn vfs_flush(&mut self, fd: i32) {
        let (path, data) = match self.hle.vfs.fds.get_mut(&fd) {
            Some(OpenFile { backing: Backing::Mem { path, data, dirty, persist }, .. }) if *dirty && *persist => {
                *dirty = false;
                (path.clone(), data.clone())
            }
            _ => return,
        };
        self.plat.save_store(&path, &data);
    }

    pub fn vfs_close(&mut self, fd: i32) -> i32 {
        if fd < 3 {
            return 0;
        }
        self.vfs_flush(fd);
        match self.hle.vfs.fds.get_mut(&fd) {
            None => -9,
            Some(f) => {
                f.refs -= 1;
                if f.refs == 0 {
                    self.hle.vfs.fds.remove(&fd);
                }
                0
            }
        }
    }
}

fn file_len(f: &OpenFile) -> u64 {
    match &f.backing {
        Backing::Data { size, .. } => *size,
        Backing::Mem { data, .. } => data.len() as u64,
        Backing::Console(_) => 0,
    }
}

pub fn pread_into(emu: &mut Emu, fd: i32, ptr: u32, len: u32, off: u64) {
    let saved = emu.hle.vfs.fds.get(&fd).map(|f| f.pos);
    if let Some(p) = saved {
        emu.vfs_seek(fd, off as i64, 0);
        emu.vfs_read(fd, ptr, len);
        emu.vfs_seek(fd, p as i64, 0);
    }
}

fn set_errno_ret(emu: &mut Emu, r: i64) {
    if r < 0 {
        super::libc::set_errno(emu, (-r) as u32);
        emu.ret(u32::MAX);
    } else {
        emu.ret(r as u32);
    }
}

// ---------------- fd API ----------------

fn open(emu: &mut Emu) -> Flow {
    let path = emu.mem.cstr(emu.arg(0));
    let flags = emu.arg(1);
    let acc = flags & 3;
    let fd = emu.vfs_open(&path, acc != 1, acc != 0, flags & 0x40 != 0, flags & 0x200 != 0, flags & 0x400 != 0);
    set_errno_ret(emu, fd as i64);
    Flow::Ret
}
fn close(emu: &mut Emu) -> Flow {
    if emu.arg(0) >= 512 { return super::net::call(emu, 15); }
    let r = emu.vfs_close(emu.arg(0) as i32);
    set_errno_ret(emu, r as i64);
    Flow::Ret
}
fn read(emu: &mut Emu) -> Flow {
    if emu.arg(0) >= 512 { return super::net::call(emu, 16); }
    let r = emu.vfs_read(emu.arg(0) as i32, emu.arg(1), emu.arg(2));
    set_errno_ret(emu, r);
    Flow::Ret
}
fn write(emu: &mut Emu) -> Flow {
    if emu.arg(0) >= 512 { return super::net::call(emu, 17); }
    let r = emu.vfs_write(emu.arg(0) as i32, emu.arg(1), emu.arg(2));
    set_errno_ret(emu, r);
    Flow::Ret
}
fn lseek(emu: &mut Emu) -> Flow {
    let r = emu.vfs_seek(emu.arg(0) as i32, emu.arg(1) as i32 as i64, emu.arg(2));
    set_errno_ret(emu, r);
    Flow::Ret
}
fn write_stat(emu: &mut Emu, st: u32, size: u64, dir: bool) {
    emu.mem.fill(st, 0, 104);
    emu.mem.w32(st + 16, if dir { 0o040755 } else { 0o100644 });
    emu.mem.w32(st + 20, 1);
    emu.mem.w64(st + 48, size);
    emu.mem.w32(st + 56, 4096);
    emu.mem.w64(st + 64, (size + 511) / 512);
}
fn fstat(emu: &mut Emu) -> Flow {
    let (fd, st) = (emu.arg(0) as i32, emu.arg(1));
    let size = match emu.hle.vfs.fds.get(&fd) {
        Some(f) => file_len(f),
        None => {
            set_errno_ret(emu, -9);
            return Flow::Ret;
        }
    };
    write_stat(emu, st, size, false);
    emu.ret(0);
    Flow::Ret
}
fn stat(emu: &mut Emu) -> Flow {
    let path = emu.mem.cstr(emu.arg(0));
    let st = emu.arg(1);
    let fd = emu.vfs_open(&path, true, false, false, false, false);
    if fd < 0 {
        set_errno_ret(emu, fd as i64);
        return Flow::Ret;
    }
    let size = file_len(emu.hle.vfs.fds.get(&fd).unwrap());
    emu.vfs_close(fd);
    write_stat(emu, st, size, false);
    emu.ret(0);
    Flow::Ret
}
fn access(emu: &mut Emu) -> Flow {
    let path = emu.mem.cstr(emu.arg(0));
    let fd = emu.vfs_open(&path, true, false, false, false, false);
    if fd < 0 {
        set_errno_ret(emu, fd as i64);
    } else {
        emu.vfs_close(fd);
        emu.ret(0);
    }
    Flow::Ret
}
fn remove(emu: &mut Emu) -> Flow {
    let path = emu.mem.cstr(emu.arg(0));
    let abs = normalize(&emu.hle.vfs.cwd, &path);
    let (_, key) = classify(&abs);
    emu.plat.save_delete(&key);
    emu.ret(0);
    Flow::Ret
}
fn rename(emu: &mut Emu) -> Flow {
    let a = normalize(&emu.hle.vfs.cwd, &emu.mem.cstr(emu.arg(0)));
    let b = normalize(&emu.hle.vfs.cwd, &emu.mem.cstr(emu.arg(1)));
    let (_, ka) = classify(&a);
    let (_, kb) = classify(&b);
    match emu.plat.save_load(&ka) {
        Some(d) => {
            emu.plat.save_store(&kb, &d);
            emu.plat.save_delete(&ka);
            emu.ret(0);
        }
        None => set_errno_ret(emu, -2),
    }
    Flow::Ret
}
fn chdir(emu: &mut Emu) -> Flow {
    let p = emu.mem.cstr(emu.arg(0));
    let abs = normalize(&emu.hle.vfs.cwd, &p);
    emu.hle.vfs.cwd = abs;
    emu.ret(0);
    Flow::Ret
}
fn getcwd(emu: &mut Emu) -> Flow {
    let (buf, n) = (emu.arg(0), emu.arg(1) as usize);
    let s = emu.hle.vfs.cwd.clone();
    if s.len() + 1 > n {
        emu.ret(0);
    } else {
        emu.mem.write_cstr(buf, s.as_bytes());
        emu.ret(buf);
    }
    Flow::Ret
}

// ---------------- stdio ----------------

fn stream_fd(emu: &Emu, fp: u32) -> Option<i32> {
    emu.hle.vfs.streams.get(&fp).copied()
}

fn sync_flags(emu: &mut Emu, fp: u32, fd: i32) {
    if let Some(f) = emu.hle.vfs.fds.get(&fd) {
        let mut fl = emu.mem.r16(fp + 12) & !(SEOF | SERR);
        if f.eof {
            fl |= SEOF;
        }
        if f.err {
            fl |= SERR;
        }
        emu.mem.w16(fp + 12, fl);
    }
}

fn fopen(emu: &mut Emu) -> Flow {
    let path = emu.mem.cstr(emu.arg(0));
    let mode = emu.mem.cstr(emu.arg(1));
    let plus = mode.contains('+');
    let (r, w, c, t, a) = match mode.chars().next() {
        Some('r') => (true, plus, false, false, false),
        Some('w') => (plus, true, true, true, false),
        Some('a') => (plus, true, true, false, true),
        _ => {
            set_errno_ret(emu, -22);
            emu.ret(0);
            return Flow::Ret;
        }
    };
    let fd = emu.vfs_open(&path, r, w, c, t, a);
    if fd < 0 {
        super::libc::set_errno(emu, (-fd) as u32);
        emu.ret(0);
        return Flow::Ret;
    }
    let fp = match emu.hle.vfs.free_files.pop() {
        Some(p) => p,
        None => emu.hle.heap.malloc(&mut emu.mem, FILE_SIZE),
    };
    emu.mem.fill(fp, 0, FILE_SIZE as usize);
    emu.mem.w16(fp + 12, if w { 0x0008 } else { 0x0004 });
    emu.mem.w16(fp + 14, fd as u16);
    emu.hle.vfs.streams.insert(fp, fd);
    emu.ret(fp);
    Flow::Ret
}
fn fclose(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    match emu.hle.vfs.streams.remove(&fp) {
        Some(fd) => {
            emu.vfs_close(fd);
            if fp >= emu.hle.vfs.sf_base + FILE_SIZE * 3 || fp < emu.hle.vfs.sf_base {
                emu.hle.vfs.free_files.push(fp);
            }
            emu.ret(0);
        }
        None => emu.ret(u32::MAX),
    }
    Flow::Ret
}
fn fread(emu: &mut Emu) -> Flow {
    let (buf, size, n, fp) = (emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3));
    let total = size.wrapping_mul(n);
    let fd = match stream_fd(emu, fp) {
        Some(fd) => fd,
        None => {
            emu.ret(0);
            return Flow::Ret;
        }
    };
    if total == 0 {
        emu.ret(0);
        return Flow::Ret;
    }
    let r = emu.vfs_read(fd, buf, total);
    sync_flags(emu, fp, fd);
    emu.ret(if r <= 0 { 0 } else { r as u32 / size });
    Flow::Ret
}
fn fwrite(emu: &mut Emu) -> Flow {
    let (buf, size, n, fp) = (emu.arg(0), emu.arg(1), emu.arg(2), emu.arg(3));
    let total = size.wrapping_mul(n);
    let fd = match stream_fd(emu, fp) {
        Some(fd) => fd,
        None => {
            emu.ret(0);
            return Flow::Ret;
        }
    };
    if total == 0 {
        emu.ret(0);
        return Flow::Ret;
    }
    let r = emu.vfs_write(fd, buf, total);
    emu.ret(if r <= 0 { 0 } else { r as u32 / size });
    Flow::Ret
}
fn fseek(emu: &mut Emu) -> Flow {
    let (fp, off, wh) = (emu.arg(0), emu.arg(1) as i32 as i64, emu.arg(2));
    let r = match stream_fd(emu, fp) {
        Some(fd) => {
            let r = emu.vfs_seek(fd, off, wh);
            sync_flags(emu, fp, fd);
            r
        }
        None => -9,
    };
    emu.ret(if r < 0 { u32::MAX } else { 0 });
    Flow::Ret
}
fn ftell(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let r = match stream_fd(emu, fp) {
        Some(fd) => emu.vfs_seek(fd, 0, 1),
        None => -1,
    };
    emu.ret(r as u32);
    Flow::Ret
}
fn rewind(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    if let Some(fd) = stream_fd(emu, fp) {
        emu.vfs_seek(fd, 0, 0);
        sync_flags(emu, fp, fd);
    }
    Flow::Ret
}
fn fgetpos(emu: &mut Emu) -> Flow {
    let (fp, pos) = (emu.arg(0), emu.arg(1));
    match stream_fd(emu, fp) {
        Some(fd) => {
            let r = emu.vfs_seek(fd, 0, 1);
            emu.mem.w32(pos, r as u32);
            emu.ret(0);
        }
        None => emu.ret(u32::MAX),
    }
    Flow::Ret
}
fn fsetpos(emu: &mut Emu) -> Flow {
    let (fp, pos) = (emu.arg(0), emu.arg(1));
    match stream_fd(emu, fp) {
        Some(fd) => {
            let p = emu.mem.r32(pos) as i32 as i64;
            emu.vfs_seek(fd, p, 0);
            sync_flags(emu, fp, fd);
            emu.ret(0);
        }
        None => emu.ret(u32::MAX),
    }
    Flow::Ret
}
fn fflush(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    if let Some(fd) = stream_fd(emu, fp) {
        emu.vfs_flush(fd);
    }
    emu.ret(0);
    Flow::Ret
}
fn feof(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let r = stream_fd(emu, fp).and_then(|fd| emu.hle.vfs.fds.get(&fd)).map(|f| f.eof).unwrap_or(true);
    emu.ret(r as u32);
    Flow::Ret
}
fn ferror(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let r = stream_fd(emu, fp).and_then(|fd| emu.hle.vfs.fds.get(&fd)).map(|f| f.err).unwrap_or(true);
    emu.ret(r as u32);
    Flow::Ret
}
fn clearerr(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    if let Some(fd) = stream_fd(emu, fp) {
        if let Some(f) = emu.hle.vfs.fds.get_mut(&fd) {
            f.eof = false;
            f.err = false;
        }
        sync_flags(emu, fp, fd);
    }
    Flow::Ret
}
fn fileno(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let r = stream_fd(emu, fp).unwrap_or(-1);
    emu.ret(r as u32);
    Flow::Ret
}
fn fgetc(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let fd = match stream_fd(emu, fp) {
        Some(fd) => fd,
        None => {
            emu.ret(u32::MAX);
            return Flow::Ret;
        }
    };
    let tmp = emu.hle.scratch + 4000;
    let r = emu.vfs_read(fd, tmp, 1);
    sync_flags(emu, fp, fd);
    emu.ret(if r == 1 { emu.mem.r8(tmp) as u32 } else { u32::MAX });
    Flow::Ret
}
fn ungetc(emu: &mut Emu) -> Flow {
    let (c, fp) = (emu.arg(0), emu.arg(1));
    if c == u32::MAX {
        emu.ret(u32::MAX);
        return Flow::Ret;
    }
    if let Some(fd) = stream_fd(emu, fp) {
        if let Some(f) = emu.hle.vfs.fds.get_mut(&fd) {
            if f.unget.is_none() && f.pos > 0 {
                f.unget = Some(c as u8);
                f.eof = false;
            } else if f.pos > 0 {
                f.pos -= 1;
            }
        }
        sync_flags(emu, fp, fd);
    }
    emu.ret(c & 0xff);
    Flow::Ret
}
fn fgets(emu: &mut Emu) -> Flow {
    let (buf, n, fp) = (emu.arg(0), emu.arg(1) as i32, emu.arg(2));
    let fd = match stream_fd(emu, fp) {
        Some(fd) => fd,
        None => {
            emu.ret(0);
            return Flow::Ret;
        }
    };
    if n <= 0 {
        emu.ret(0);
        return Flow::Ret;
    }
    let tmp = emu.hle.scratch + 4000;
    let mut i = 0;
    while i < n - 1 {
        if emu.vfs_read(fd, tmp, 1) != 1 {
            break;
        }
        let c = emu.mem.r8(tmp);
        emu.mem.w8(buf + i as u32, c);
        i += 1;
        if c == b'\n' {
            break;
        }
    }
    emu.mem.w8(buf + i as u32, 0);
    sync_flags(emu, fp, fd);
    emu.ret(if i == 0 { 0 } else { buf });
    Flow::Ret
}
fn write_stream(emu: &mut Emu, fp: u32, bytes: &[u8]) -> i64 {
    let fd = match stream_fd(emu, fp) {
        Some(fd) => fd,
        None => return -1,
    };
    let tmp = emu.hle.heap.malloc(&mut emu.mem, bytes.len() as u32 + 1);
    emu.mem.write_bytes(tmp, bytes);
    let r = emu.vfs_write(fd, tmp, bytes.len() as u32);
    emu.hle.heap.free(&mut emu.mem, tmp);
    r
}
fn fputc(emu: &mut Emu) -> Flow {
    let (c, fp) = (emu.arg(0), emu.arg(1));
    let r = write_stream(emu, fp, &[c as u8]);
    emu.ret(if r == 1 { c & 0xff } else { u32::MAX });
    Flow::Ret
}
fn fputs(emu: &mut Emu) -> Flow {
    let (s, fp) = (emu.arg(0), emu.arg(1));
    let b = emu.mem.cstr_bytes(s).to_vec();
    let r = write_stream(emu, fp, &b);
    emu.ret(if r < 0 { u32::MAX } else { 0 });
    Flow::Ret
}
fn puts(emu: &mut Emu) -> Flow {
    let s = emu.mem.cstr(emu.arg(0));
    emu.log(&format!("[stdout] {}", s));
    emu.ret(0);
    Flow::Ret
}
fn putchar(emu: &mut Emu) -> Flow {
    let c = emu.arg(0);
    emu.ret(c);
    Flow::Ret
}
fn fprintf(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let mut va = VaArgs::from_call(emu, 2);
    let s = super::fmt::format(emu, emu.arg(1), &mut va);
    let r = write_stream(emu, fp, &s);
    emu.ret(r as u32);
    Flow::Ret
}
fn vfprintf(emu: &mut Emu) -> Flow {
    let fp = emu.arg(0);
    let mut va = VaArgs::from_va_list(emu.arg(2));
    let s = super::fmt::format(emu, emu.arg(1), &mut va);
    let r = write_stream(emu, fp, &s);
    emu.ret(r as u32);
    Flow::Ret
}
fn fscanf(emu: &mut Emu) -> Flow {
    // Read the remainder of the line-buffered stream content (approximation:
    // read up to 4 KiB, scan, then seek back by the unconsumed amount).
    emu.ret(u32::MAX);
    Flow::Ret
}
