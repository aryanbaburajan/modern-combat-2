//! Guest (ARM) address space.
//!
//! The guest sees a flat 32-bit address space of which the low `GUEST_SIZE`
//! bytes are backed by a single host allocation. Every access is masked so a
//! wild guest pointer can never touch host memory outside the buffer.

pub const GUEST_SIZE: usize = 0x2000_0000; // 512 MiB
pub const GUEST_MASK: u32 = (GUEST_SIZE as u32) - 1;

// Address-space layout.
pub const THUNK_BASE: u32 = 0x0001_0000; // one 4-byte slot per imported symbol
pub const THUNK_END: u32 = 0x0001_F000;
pub const RETURN_MAGIC: u32 = 0x0001_FFF0; // host-call return address
pub const THREAD_EXIT_MAGIC: u32 = 0x0001_FFE0; // thread start routine return address
pub const LIB_BASE: u32 = 0x0010_0000;
pub const STATIC_BASE: u32 = 0x0080_0000; // HLE-owned data (ctype tables, FILEs, JNI)
pub const STATIC_END: u32 = 0x0100_0000;
pub const HEAP_BASE: u32 = 0x0100_0000;
pub const HEAP_END: u32 = 0x1C00_0000;
pub const STACK_BASE: u32 = 0x1C00_0000;
pub const STACK_END: u32 = 0x2000_0000;

pub struct Memory {
    buf: Vec<u8>,
}

impl Memory {
    pub fn new() -> Memory {
        Memory { buf: vec![0u8; GUEST_SIZE] }
    }

    #[inline(always)]
    pub fn host_ptr(&self) -> *const u8 {
        self.buf.as_ptr()
    }

    #[inline(always)]
    pub fn r8(&self, a: u32) -> u8 {
        unsafe { *self.buf.get_unchecked((a & GUEST_MASK) as usize) }
    }
    #[inline(always)]
    pub fn r16(&self, a: u32) -> u16 {
        let a = a & GUEST_MASK;
        if a <= GUEST_MASK - 1 {
            unsafe { (self.buf.as_ptr().add(a as usize) as *const u16).read_unaligned() }
        } else {
            self.r8(a) as u16 | (self.r8(a.wrapping_add(1)) as u16) << 8
        }
    }
    #[inline(always)]
    pub fn r32(&self, a: u32) -> u32 {
        let a = a & GUEST_MASK;
        if a <= GUEST_MASK - 3 {
            unsafe { (self.buf.as_ptr().add(a as usize) as *const u32).read_unaligned() }
        } else {
            self.r16(a) as u32 | (self.r16(a.wrapping_add(2)) as u32) << 16
        }
    }
    #[inline(always)]
    pub fn r64(&self, a: u32) -> u64 {
        self.r32(a) as u64 | (self.r32(a.wrapping_add(4)) as u64) << 32
    }
    #[inline(always)]
    pub fn w8(&mut self, a: u32, v: u8) {
        unsafe { *self.buf.get_unchecked_mut((a & GUEST_MASK) as usize) = v }
    }
    #[inline(always)]
    pub fn w16(&mut self, a: u32, v: u16) {
        let a = a & GUEST_MASK;
        if a <= GUEST_MASK - 1 {
            unsafe { (self.buf.as_mut_ptr().add(a as usize) as *mut u16).write_unaligned(v) }
        } else {
            self.w8(a, v as u8);
            self.w8(a.wrapping_add(1), (v >> 8) as u8);
        }
    }
    #[inline(always)]
    pub fn w32(&mut self, a: u32, v: u32) {
        let a = a & GUEST_MASK;
        if a <= GUEST_MASK - 3 {
            unsafe { (self.buf.as_mut_ptr().add(a as usize) as *mut u32).write_unaligned(v) }
        } else {
            self.w16(a, v as u16);
            self.w16(a.wrapping_add(2), (v >> 16) as u16);
        }
    }
    #[inline(always)]
    pub fn w64(&mut self, a: u32, v: u64) {
        self.w32(a, v as u32);
        self.w32(a.wrapping_add(4), (v >> 32) as u32);
    }

    /// Borrow a guest range as a host slice (clamped to the buffer).
    pub fn slice(&self, a: u32, len: usize) -> &[u8] {
        let a = (a & GUEST_MASK) as usize;
        let end = (a + len).min(GUEST_SIZE);
        &self.buf[a..end]
    }
    pub fn slice_mut(&mut self, a: u32, len: usize) -> &mut [u8] {
        let a = (a & GUEST_MASK) as usize;
        let end = (a + len).min(GUEST_SIZE);
        &mut self.buf[a..end]
    }

    pub fn write_bytes(&mut self, a: u32, data: &[u8]) {
        self.slice_mut(a, data.len()).copy_from_slice(data);
    }

    pub fn fill(&mut self, a: u32, v: u8, len: usize) {
        for b in self.slice_mut(a, len) {
            *b = v;
        }
    }

    pub fn copy_within(&mut self, dst: u32, src: u32, len: usize) {
        let s = (src & GUEST_MASK) as usize;
        let d = (dst & GUEST_MASK) as usize;
        if s + len > GUEST_SIZE || d + len > GUEST_SIZE {
            return;
        }
        self.buf.copy_within(s..s + len, d);
    }

    pub fn strlen(&self, a: u32) -> usize {
        let start = (a & GUEST_MASK) as usize;
        match self.buf[start..].iter().position(|&b| b == 0) {
            Some(n) => n,
            None => GUEST_SIZE - start,
        }
    }

    pub fn cstr_bytes(&self, a: u32) -> &[u8] {
        let n = self.strlen(a);
        self.slice(a, n)
    }

    pub fn cstr(&self, a: u32) -> String {
        String::from_utf8_lossy(self.cstr_bytes(a)).into_owned()
    }

    pub fn write_cstr(&mut self, a: u32, s: &[u8]) {
        self.write_bytes(a, s);
        self.w8(a + s.len() as u32, 0);
    }

    pub fn wstrlen(&self, a: u32) -> usize {
        let mut n = 0;
        while self.r32(a + 4 * n as u32) != 0 {
            n += 1;
        }
        n
    }
}
