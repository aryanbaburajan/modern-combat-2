//! Guest heap allocator (malloc/free) living in the guest address space.
//!
//! Each block has an 8-byte header: [capacity: u32][magic: u32]. Small
//! blocks use segregated free lists; large blocks use best-fit with
//! coalescing.

use crate::mem::Memory;
use std::collections::{BTreeMap, BTreeSet};

const HDR: u32 = 8;
const MAGIC_USED: u32 = 0xA110_C8ED;
const MAGIC_FREE: u32 = 0xF4EE_B10C;
const SMALL_MAX: u32 = 2048;
const SMALL_STEP: u32 = 16;

pub struct Heap {
    start: u32,
    end: u32,
    brk: u32,
    small: Vec<Vec<u32>>, // free user pointers per class
    large_by_addr: BTreeMap<u32, u32>, // block start (header) -> total size
    large_by_size: BTreeSet<(u32, u32)>, // (total size, block start)
    pub live_bytes: u64,
    pub peak_bytes: u64,
}

fn class_of(size: u32) -> usize {
    ((size + SMALL_STEP - 1) / SMALL_STEP).max(1) as usize
}

impl Heap {
    pub fn new(start: u32, end: u32) -> Heap {
        Heap {
            start,
            end,
            brk: start,
            small: vec![Vec::new(); (SMALL_MAX / SMALL_STEP) as usize + 1],
            large_by_addr: BTreeMap::new(),
            large_by_size: BTreeSet::new(),
            live_bytes: 0,
            peak_bytes: 0,
        }
    }

    pub fn contains(&self, p: u32) -> bool {
        p >= self.start + HDR && p < self.brk
    }

    fn bump(&mut self, total: u32, align: u32) -> Option<u32> {
        // header start such that user pointer (start+HDR) is aligned
        let mut hs = self.brk;
        let user = (hs + HDR + align - 1) & !(align - 1);
        let pad = user - HDR - hs;
        if pad != 0 {
            // donate padding as a free large region if big enough, else waste it
            if pad >= 64 {
                self.add_free_large(hs, pad);
            }
            hs = user - HDR;
        }
        if hs as u64 + total as u64 > self.end as u64 {
            return None;
        }
        self.brk = hs + total;
        Some(hs)
    }

    fn add_free_large(&mut self, start: u32, size: u32) {
        self.large_by_addr.insert(start, size);
        self.large_by_size.insert((size, start));
    }

    fn remove_free_large(&mut self, start: u32, size: u32) {
        self.large_by_addr.remove(&start);
        self.large_by_size.remove(&(size, start));
    }

    pub fn malloc(&mut self, mem: &mut Memory, size: u32) -> u32 {
        self.memalign(mem, 8, size)
    }

    pub fn memalign(&mut self, mem: &mut Memory, align: u32, size: u32) -> u32 {
        let align = align.max(8).next_power_of_two();
        if size <= SMALL_MAX && align == 8 {
            let c = class_of(size);
            let cap = c as u32 * SMALL_STEP;
            let p = if let Some(p) = self.small[c].pop() {
                p
            } else {
                match self.bump(cap + HDR, 8) {
                    Some(hs) => hs + HDR,
                    None => return 0,
                }
            };
            mem.w32(p - 8, cap);
            mem.w32(p - 4, MAGIC_USED);
            self.account(cap as i64);
            return p;
        }
        let need = ((size.max(SMALL_MAX + 1) + 15) & !15) + HDR;
        // best fit search
        let mut found = None;
        for &(sz, start) in self.large_by_size.range((need, 0)..) {
            let user = (start + HDR + align - 1) & !(align - 1);
            if user - HDR - start + need <= sz {
                found = Some((sz, start, user));
                break;
            }
            if sz > need + align * 4 + 4096 * 16 {
                break;
            }
        }
        let (hs, total) = if let Some((sz, start, user)) = found {
            self.remove_free_large(start, sz);
            let hs = user - HDR;
            if hs > start {
                let pre = hs - start;
                if pre >= 64 {
                    self.add_free_large(start, pre);
                }
            }
            let avail = start + sz - hs;
            if avail - need >= 256 {
                self.add_free_large(hs + need, avail - need);
                (hs, need)
            } else {
                (hs, avail)
            }
        } else {
            match self.bump(need, align) {
                Some(hs) => (hs, need),
                None => return 0,
            }
        };
        let cap = total - HDR;
        mem.w32(hs, cap);
        mem.w32(hs + 4, MAGIC_USED);
        self.account(cap as i64);
        hs + HDR
    }

    fn account(&mut self, d: i64) {
        self.live_bytes = (self.live_bytes as i64 + d) as u64;
        self.peak_bytes = self.peak_bytes.max(self.live_bytes);
    }

    pub fn capacity(&self, mem: &Memory, p: u32) -> Option<u32> {
        if !self.contains(p) || mem.r32(p - 4) != MAGIC_USED {
            return None;
        }
        Some(mem.r32(p - 8))
    }

    pub fn free(&mut self, mem: &mut Memory, p: u32) -> bool {
        if p == 0 {
            return true;
        }
        let cap = match self.capacity(mem, p) {
            Some(c) => c,
            None => return false,
        };
        mem.w32(p - 4, MAGIC_FREE);
        self.account(-(cap as i64));
        if cap <= SMALL_MAX {
            self.small[(cap / SMALL_STEP) as usize].push(p);
            return true;
        }
        let mut start = p - HDR;
        let mut size = cap + HDR;
        // coalesce with following free block
        if let Some(&nsz) = self.large_by_addr.get(&(start + size)) {
            self.remove_free_large(start + size, nsz);
            size += nsz;
        }
        // coalesce with preceding free block
        if let Some((&pstart, &psz)) = self.large_by_addr.range(..start).next_back() {
            if pstart + psz == start {
                self.remove_free_large(pstart, psz);
                start = pstart;
                size += psz;
            }
        }
        if start + size == self.brk {
            self.brk = start;
        } else {
            self.add_free_large(start, size);
        }
        true
    }

    pub fn realloc(&mut self, mem: &mut Memory, p: u32, size: u32) -> u32 {
        if p == 0 {
            return self.malloc(mem, size);
        }
        if size == 0 {
            self.free(mem, p);
            return 0;
        }
        let cap = match self.capacity(mem, p) {
            Some(c) => c,
            None => return 0,
        };
        if size <= cap && (cap <= SMALL_MAX || size > cap / 2) {
            return p;
        }
        let np = self.malloc(mem, size);
        if np == 0 {
            return 0;
        }
        mem.copy_within(np, p, cap.min(size) as usize);
        self.free(mem, p);
        np
    }
}
