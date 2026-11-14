//! Minimal ELF32/ARM shared-object loader.

use crate::mem::Memory;
use std::collections::HashMap;

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn cstr_at(d: &[u8], o: usize) -> String {
    let end = d[o..].iter().position(|&b| b == 0).map(|n| o + n).unwrap_or(d.len());
    String::from_utf8_lossy(&d[o..end]).into_owned()
}

pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
}

pub struct LoadedLib {
    pub base: u32,
    pub size: u32,
    pub exports: HashMap<String, u32>,
    pub init_array: Vec<u32>,
    pub text_start: u32,
    pub text_end: u32,
    pub plt_start: u32,
    pub plt_end: u32,
    /// Sorted (addr, size, name) of all known functions for symbolization.
    pub funcs: Vec<Symbol>,
    /// Thumb/ARM/data mapping symbols ($t/$a/$d), sorted by address.
    pub mapping: Vec<(u32, u8)>,
}

impl LoadedLib {
    pub fn find_func(&self, name: &str) -> Option<u32> {
        self.funcs.iter().find(|s| s.name == name).map(|s| s.value)
    }

    pub fn symbolize(&self, addr: u32) -> Option<(String, u32)> {
        let a = addr & !1;
        let idx = match self.funcs.binary_search_by(|s| s.value.cmp(&a)) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let s = &self.funcs[idx];
        if a - s.value < s.size.max(1).max(0x10000) {
            Some((s.name.clone(), a - s.value))
        } else {
            None
        }
    }
}

/// Load `data` at `base`. `resolve` maps an undefined symbol name to an address.
pub fn load_elf(
    mem: &mut Memory,
    data: &[u8],
    base: u32,
    resolve: &mut dyn FnMut(&str, bool) -> u32,
) -> Result<LoadedLib, String> {
    if &data[0..4] != b"\x7fELF" || data[4] != 1 {
        return Err("not an ELF32 file".into());
    }
    let phoff = u32le(data, 28) as usize;
    let shoff = u32le(data, 32) as usize;
    let phentsize = u16le(data, 42) as usize;
    let phnum = u16le(data, 44) as usize;
    let shentsize = u16le(data, 46) as usize;
    let shnum = u16le(data, 48) as usize;
    let shstrndx = u16le(data, 50) as usize;

    let mut max_addr = 0u32;
    let mut dyn_off = None;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let ptype = u32le(data, o);
        let off = u32le(data, o + 4) as usize;
        let vaddr = u32le(data, o + 8);
        let filesz = u32le(data, o + 16) as usize;
        let memsz = u32le(data, o + 20);
        match ptype {
            1 => {
                mem.write_bytes(base + vaddr, &data[off..off + filesz]);
                max_addr = max_addr.max(vaddr + memsz);
            }
            2 => dyn_off = Some(vaddr),
            _ => {}
        }
    }
    let dyn_off = dyn_off.ok_or("no PT_DYNAMIC")?;

    // Parse dynamic section (from loaded memory image).
    let mut tags: HashMap<u32, u32> = HashMap::new();
    let mut a = base + dyn_off;
    loop {
        let tag = mem.r32(a);
        let val = mem.r32(a + 4);
        if tag == 0 {
            break;
        }
        tags.entry(tag).or_insert(val);
        a += 8;
    }
    let symtab = tags.get(&6).copied().ok_or("no DT_SYMTAB")?;
    let strtab = tags.get(&5).copied().ok_or("no DT_STRTAB")?;
    let hash = tags.get(&4).copied();
    let nsyms = match hash {
        Some(h) => mem.r32(base + h + 4),
        None => (strtab - symtab) / 16,
    };

    let mut exports = HashMap::new();
    let mut dynsyms: Vec<(String, u32, bool)> = Vec::new();
    for i in 0..nsyms {
        let s = base + symtab + i * 16;
        let name = mem.cstr(base + strtab + mem.r32(s));
        let value = mem.r32(s + 4);
        let info = mem.r8(s + 12);
        let shndx = mem.r16(s + 14);
        let is_func = info & 0xf == 2;
        if shndx != 0 && !name.is_empty() {
            exports.insert(name.clone(), base + value);
        }
        dynsyms.push((name, if shndx != 0 { base + value } else { 0 }, is_func));
    }

    let mut resolved: HashMap<u32, u32> = HashMap::new();
    let mut sym_addr = |idx: u32, mem: &Memory| -> u32 {
        if let Some(&v) = resolved.get(&idx) {
            return v;
        }
        let _ = mem;
        let (ref name, value, is_func) = dynsyms[idx as usize];
        let v = if value != 0 { value } else { resolve(name, is_func) };
        resolved.insert(idx, v);
        v
    };

    let mut apply = |mem: &mut Memory, rel: u32, size: u32| -> Result<(), String> {
        let mut o = 0;
        while o < size {
            let r_off = mem.r32(base + rel + o);
            let r_info = mem.r32(base + rel + o + 4);
            let ty = r_info & 0xff;
            let sym = r_info >> 8;
            let where_ = base + r_off;
            match ty {
                0 => {}
                2 => {
                    let s = sym_addr(sym, mem);
                    let a = mem.r32(where_);
                    mem.w32(where_, s.wrapping_add(a));
                }
                3 => {
                    let s = sym_addr(sym, mem);
                    let a = mem.r32(where_);
                    mem.w32(where_, s.wrapping_add(a).wrapping_sub(where_));
                }
                21 | 22 => {
                    let s = sym_addr(sym, mem);
                    mem.w32(where_, s);
                }
                23 => {
                    let a = mem.r32(where_);
                    mem.w32(where_, base.wrapping_add(a));
                }
                _ => return Err(format!("unsupported relocation type {}", ty)),
            }
            o += 8;
        }
        Ok(())
    };
    if let (Some(&rel), Some(&sz)) = (tags.get(&17), tags.get(&18)) {
        apply(mem, rel, sz)?;
    }
    if let (Some(&rel), Some(&sz)) = (tags.get(&23), tags.get(&2)) {
        apply(mem, rel, sz)?;
    }

    let mut init_array = Vec::new();
    if let Some(&init) = tags.get(&12) {
        init_array.push(base + init);
    }
    if let (Some(&ia), Some(&sz)) = (tags.get(&25), tags.get(&27)) {
        for i in 0..sz / 4 {
            let f = mem.r32(base + ia + i * 4);
            if f != 0 && f != 0xffff_ffff {
                init_array.push(f);
            }
        }
    }

    // Section headers: .text / .plt bounds and the static symbol table.
    let mut text = (0, 0);
    let mut plt = (0, 0);
    let mut funcs = Vec::new();
    let mut mapping = Vec::new();
    if shoff != 0 && shoff + shnum * shentsize <= data.len() {
        let sh = |i: usize| shoff + i * shentsize;
        let shstr_off = u32le(data, sh(shstrndx) + 16) as usize;
        let mut symtab_sec = None;
        for i in 0..shnum {
            let o = sh(i);
            let name = cstr_at(data, shstr_off + u32le(data, o) as usize);
            let addr = u32le(data, o + 12);
            let off = u32le(data, o + 16);
            let size = u32le(data, o + 20);
            let link = u32le(data, o + 24);
            match name.as_str() {
                ".text" => text = (base + addr, base + addr + size),
                ".plt" => plt = (base + addr, base + addr + size),
                ".symtab" => symtab_sec = Some((off as usize, size as usize, link as usize)),
                _ => {}
            }
        }
        if let Some((off, size, link)) = symtab_sec {
            let stroff = u32le(data, sh(link) + 16) as usize;
            for i in 0..size / 16 {
                let s = off + i * 16;
                let name = cstr_at(data, stroff + u32le(data, s) as usize);
                let value = u32le(data, s + 4);
                let sz = u32le(data, s + 8);
                let info = data[s + 12];
                let shndx = u16le(data, s + 14);
                if shndx == 0 {
                    continue;
                }
                if name == "$t" || name == "$a" || name == "$d" {
                    mapping.push((base + value, name.as_bytes()[1]));
                } else if info & 0xf == 2 {
                    funcs.push(Symbol { name, value: base + (value & !1), size: sz });
                }
            }
        }
    }
    if funcs.is_empty() {
        for (name, value, is_func) in &dynsyms {
            if *is_func && *value != 0 {
                funcs.push(Symbol { name: name.clone(), value: value & !1, size: 0 });
            }
        }
    }
    funcs.sort_by_key(|s| s.value);
    mapping.sort();

    Ok(LoadedLib {
        base,
        size: max_addr,
        exports,
        init_array,
        text_start: text.0,
        text_end: text.1,
        plt_start: plt.0,
        plt_end: plt.1,
        funcs,
        mapping,
    })
}
