//! A fake JNIEnv / JavaVM. Static Java methods the game calls are forwarded
//! to the platform (audio etc.) or handled here.

use super::{Flow, State, VaArgs};
use crate::mem::Memory;
use crate::platform::JValue;
use crate::Emu;
use std::collections::HashMap;

const NUM_ENV_FUNCS: u32 = 233;
const NUM_VM_FUNCS: u32 = 8;
const HANDLE_BASE: u32 = 0x5000_0000;
const METHOD_BASE: u32 = 0x6000_0000;
const FIELD_BASE: u32 = 0x6800_0000;

#[derive(Clone, Debug)]
pub enum JObj {
    Class(String),
    Str(String),
    Bytes(Vec<u8>),
    Ints(Vec<i32>),
    Other(String),
}

pub struct Method {
    pub class: String,
    pub name: String,
    pub sig: String,
}

pub struct Jni {
    pub env: u32,
    pub vm: u32,
    env_thunk_base: u32,
    vm_thunk_base: u32,
    objs: Vec<Option<JObj>>,
    free_objs: Vec<u32>,
    pub methods: Vec<Method>,
    method_ids: HashMap<(String, String, String), u32>,
    fields: Vec<(String, String, String)>,
    /// Guest buffers handed out by Get*ArrayElements / GetStringUTFChars.
    pinned: HashMap<u32, u32>,
    pub trace: bool,
}

impl Jni {
    pub fn new() -> Jni {
        Jni {
            env: 0,
            vm: 0,
            env_thunk_base: 0,
            vm_thunk_base: 0,
            objs: Vec::new(),
            free_objs: Vec::new(),
            methods: Vec::new(),
            method_ids: HashMap::new(),
            fields: Vec::new(),
            pinned: HashMap::new(),
            trace: false,
        }
    }

    pub fn new_obj(&mut self, o: JObj) -> u32 {
        if let Some(i) = self.free_objs.pop() {
            self.objs[i as usize] = Some(o);
            return HANDLE_BASE + i * 4;
        }
        self.objs.push(Some(o));
        HANDLE_BASE + (self.objs.len() as u32 - 1) * 4
    }

    pub fn get(&self, h: u32) -> Option<&JObj> {
        if h < HANDLE_BASE {
            return None;
        }
        self.objs.get(((h - HANDLE_BASE) / 4) as usize).and_then(|o| o.as_ref())
    }

    pub fn delete(&mut self, h: u32) {
        if h < HANDLE_BASE {
            return;
        }
        let i = (h - HANDLE_BASE) / 4;
        if let Some(slot) = self.objs.get_mut(i as usize) {
            // Keep classes alive (they are cached by the game as global refs).
            if let Some(JObj::Class(_)) = slot {
                return;
            }
            if slot.is_some() {
                *slot = None;
                self.free_objs.push(i);
            }
        }
    }

    pub fn new_string(&mut self, s: &str) -> u32 {
        self.new_obj(JObj::Str(s.to_string()))
    }

    pub fn string(&self, h: u32) -> Option<String> {
        match self.get(h) {
            Some(JObj::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

pub fn init(st: &mut State, mem: &mut Memory) {
    let table = st.static_alloc(NUM_ENV_FUNCS * 4, 4);
    let env = st.static_alloc(4, 4);
    mem.w32(env, table);
    let mut base = 0;
    for i in 0..NUM_ENV_FUNCS {
        let a = st.thunk_fn(&format!("JNIEnv::{}#{}", ENV_NAMES.get(i as usize).copied().unwrap_or("fn"), i), env_dispatch);
        if i == 0 {
            base = a;
        }
        mem.w32(table + i * 4, a);
    }
    let vtable = st.static_alloc(NUM_VM_FUNCS * 4, 4);
    let vm = st.static_alloc(4, 4);
    mem.w32(vm, vtable);
    let mut vbase = 0;
    for i in 0..NUM_VM_FUNCS {
        let a = st.thunk_fn(&format!("JavaVM::{}", i), vm_dispatch);
        if i == 0 {
            vbase = a;
        }
        mem.w32(vtable + i * 4, a);
    }
    st.jni.env = env;
    st.jni.vm = vm;
    st.jni.env_thunk_base = base;
    st.jni.vm_thunk_base = vbase;
}

fn vm_dispatch(emu: &mut Emu) -> Flow {
    let idx = (emu.cpu.pc - emu.hle.jni.vm_thunk_base) / 4;
    match idx {
        4 | 6 | 7 => {
            // AttachCurrentThread(vm, JNIEnv**, args) / GetEnv(vm, void**, version)
            let p = emu.arg(1);
            if p != 0 {
                let env = emu.hle.jni.env;
                emu.mem.w32(p, env);
            }
            emu.ret(0);
        }
        _ => emu.ret(0),
    }
    Flow::Ret
}

/// Parse a JNI method signature into (arg type chars, return type char).
fn parse_sig(sig: &str) -> (Vec<char>, char) {
    let mut args = Vec::new();
    let b: Vec<char> = sig.chars().collect();
    let mut i = 1;
    while i < b.len() && b[i] != ')' {
        let c = b[i];
        match c {
            'L' => {
                while i < b.len() && b[i] != ';' {
                    i += 1;
                }
                args.push('L');
            }
            '[' => {
                while i < b.len() && b[i] == '[' {
                    i += 1;
                }
                if i < b.len() && b[i] == 'L' {
                    while i < b.len() && b[i] != ';' {
                        i += 1;
                    }
                }
                args.push('[');
            }
            _ => args.push(c),
        }
        i += 1;
    }
    let ret = b.get(i + 1).copied().unwrap_or('V');
    (args, ret)
}

enum ArgSrc {
    Va(VaArgs),
    Arr(u32),
}

fn read_args(emu: &Emu, types: &[char], mut src: ArgSrc) -> Vec<JValue> {
    let mut out = Vec::new();
    let mut idx = 0u32;
    for &t in types {
        let v = match &mut src {
            ArgSrc::Va(va) => match t {
                'J' => JValue::Long(va.u64(&emu.mem) as i64),
                'D' => JValue::Double(va.f64(&emu.mem)),
                'F' => JValue::Float(va.f64(&emu.mem) as f32),
                'L' | '[' => {
                    let h = va.u32(&emu.mem);
                    match emu.hle.jni.get(h) {
                        Some(JObj::Str(s)) => JValue::Str(Some(s.clone())),
                        _ if h == 0 => JValue::Str(None),
                        _ => JValue::Object(h),
                    }
                }
                _ => JValue::Int(va.u32(&emu.mem) as i32),
            },
            ArgSrc::Arr(p) => {
                let a = *p + idx * 8;
                match t {
                    'J' => JValue::Long(emu.mem.r64(a) as i64),
                    'D' => JValue::Double(f64::from_bits(emu.mem.r64(a))),
                    'F' => JValue::Float(f32::from_bits(emu.mem.r32(a))),
                    'Z' | 'B' => JValue::Int(emu.mem.r8(a) as i32),
                    'C' => JValue::Int(emu.mem.r16(a) as i32),
                    'S' => JValue::Int(emu.mem.r16(a) as i16 as i32),
                    'L' | '[' => {
                        let h = emu.mem.r32(a);
                        match emu.hle.jni.get(h) {
                            Some(JObj::Str(s)) => JValue::Str(Some(s.clone())),
                            _ if h == 0 => JValue::Str(None),
                            _ => JValue::Object(h),
                        }
                    }
                    _ => JValue::Int(emu.mem.r32(a) as i32),
                }
            }
        };
        idx += 1;
        out.push(v);
    }
    out
}

fn set_ret(emu: &mut Emu, ret: char, v: JValue) {
    match ret {
        'V' => {}
        'J' => {
            let x = match v {
                JValue::Long(x) => x,
                JValue::Int(x) => x as i64,
                JValue::Double(x) => x as i64,
                _ => 0,
            };
            emu.ret64(x as u64);
        }
        'D' => {
            let x = match v {
                JValue::Double(x) => x,
                JValue::Float(x) => x as f64,
                JValue::Int(x) => x as f64,
                _ => 0.0,
            };
            emu.ret_f64(x);
        }
        'F' => {
            let x = match v {
                JValue::Float(x) => x,
                JValue::Double(x) => x as f32,
                JValue::Int(x) => x as f32,
                _ => 0.0,
            };
            emu.ret_f32(x);
        }
        'L' | '[' => {
            let h = match v {
                JValue::Str(Some(s)) => emu.hle.jni.new_string(&s),
                JValue::Bytes(b) => emu.hle.jni.new_obj(JObj::Bytes(b)),
                JValue::Object(h) => h,
                _ => 0,
            };
            emu.ret(h);
        }
        _ => {
            let x = v.as_i32();
            emu.ret(x as u32);
        }
    }
}

fn invoke(emu: &mut Emu, mid: u32, src: ArgSrc) {
    let idx = mid.wrapping_sub(METHOD_BASE) as usize;
    let (class, name, sig) = match emu.hle.jni.methods.get(idx) {
        Some(m) => (m.class.clone(), m.name.clone(), m.sig.clone()),
        None => {
            emu.log(&format!("[jni] call with bad method id {:08x}", mid));
            emu.ret(0);
            return;
        }
    };
    let (types, ret) = parse_sig(&sig);
    let args = read_args(emu, &types, src);
    if emu.hle.jni.trace {
        emu.log(&format!("[jni] {}.{}{} {:?}", class, name, sig, args));
    }
    let short = class.rsplit('/').next().unwrap_or(&class).to_string();
    let v = match (short.as_str(), name.as_str()) {
        ("Game", "GetCurrentTime") => JValue::Long(emu.plat.wall_time().mul_add(1000.0, 0.0) as i64),
        _ => emu.plat.java_call(&short, &name, &sig, &args),
    };
    set_ret(emu, ret, v);
}

fn env_dispatch(emu: &mut Emu) -> Flow {
    let idx = (emu.cpu.pc - emu.hle.jni.env_thunk_base) / 4;
    let a1 = emu.arg(1);
    let a2 = emu.arg(2);
    if emu.hle.jni.trace {
        let lr = emu.cpu.r[14];
        let msg = format!("[jni] env[{}] {} ({:08x}, {:08x}, {:08x}) from {}", idx, ENV_NAMES.get(idx as usize).copied().unwrap_or("?"), a1, a2, emu.arg(3), emu.symbolize(lr));
        emu.log(&msg);
    }
    match idx {
        4 => emu.ret(0x0001_0006), // GetVersion
        6 => {
            let name = emu.mem.cstr(a1);
            let h = emu.hle.jni.new_obj(JObj::Class(name));
            emu.ret(h);
        }
        10 => emu.ret(0),
        11 => emu.ret(1),
        13 | 14 => emu.ret(0),
        15 => emu.ret(0), // ExceptionOccurred
        16 | 17 => {}
        18 => {
            let msg = emu.mem.cstr(a1);
            return Flow::Fault(format!("JNI FatalError: {}", msg));
        }
        19 => emu.ret(0),
        20 => emu.ret(a1),
        21 | 25 => emu.ret(a1), // NewGlobalRef / NewLocalRef: same handle
        22 | 23 => {
            // Keep objects alive for global refs; free local refs of strings/arrays lazily.
        }
        24 => emu.ret((a1 == a2) as u32),
        26 => emu.ret(0),
        27 | 28 | 29 | 30 => {
            let h = emu.hle.jni.new_obj(JObj::Other("object".into()));
            emu.ret(h);
        }
        31 => {
            let h = emu.hle.jni.new_obj(JObj::Class("java/lang/Object".into()));
            emu.ret(h);
        }
        32 => emu.ret(1),
        33 | 113 => {
            // Get(Static)MethodID(env, class, name, sig)
            let class = match emu.hle.jni.get(a1) {
                Some(JObj::Class(c)) => c.clone(),
                _ => "?".into(),
            };
            let name = emu.mem.cstr(a2);
            let sig = emu.mem.cstr(emu.arg(3));
            let key = (class.clone(), name.clone(), sig.clone());
            let id = match emu.hle.jni.method_ids.get(&key) {
                Some(&id) => id,
                None => {
                    let id = METHOD_BASE + emu.hle.jni.methods.len() as u32;
                    emu.hle.jni.methods.push(Method { class, name, sig });
                    emu.hle.jni.method_ids.insert(key, id);
                    id
                }
            };
            emu.ret(id);
        }
        // Call<Type>Method (instance): env, obj, mid, ...
        34..=63 => {
            let k = (idx - 34) % 3;
            let src = match k {
                0 => ArgSrc::Va(VaArgs::from_call(emu, 3)),
                1 => ArgSrc::Va(VaArgs::from_va_list(emu.arg(3))),
                _ => ArgSrc::Arr(emu.arg(3)),
            };
            invoke(emu, a2, src);
        }
        // CallNonvirtual<Type>Method: env, obj, class, mid, ...
        64..=93 => {
            let k = (idx - 64) % 3;
            let src = match k {
                0 => ArgSrc::Va(VaArgs::from_call(emu, 4)),
                1 => ArgSrc::Va(VaArgs::from_va_list(emu.arg(4))),
                _ => ArgSrc::Arr(emu.arg(4)),
            };
            let mid = emu.arg(3);
            invoke(emu, mid, src);
        }
        94 | 144 => {
            let name = emu.mem.cstr(a2);
            let sig = emu.mem.cstr(emu.arg(3));
            let class = match emu.hle.jni.get(a1) {
                Some(JObj::Class(c)) => c.clone(),
                _ => "?".into(),
            };
            let id = FIELD_BASE + emu.hle.jni.fields.len() as u32;
            emu.hle.jni.fields.push((class, name, sig));
            emu.ret(id);
        }
        95..=103 | 145..=153 => {
            let fid = a2.wrapping_sub(FIELD_BASE) as usize;
            let f = emu.hle.jni.fields.get(fid).cloned();
            if let Some((c, n, _)) = f {
                emu.log(&format!("[jni] get field {}.{} -> 0", c, n));
            }
            emu.ret64(0);
        }
        104..=112 | 154..=162 => {}
        // CallStatic<Type>Method: env, class, mid, ...
        114..=143 => {
            let k = (idx - 114) % 3;
            let src = match k {
                0 => ArgSrc::Va(VaArgs::from_call(emu, 3)),
                1 => ArgSrc::Va(VaArgs::from_va_list(emu.arg(3))),
                _ => ArgSrc::Arr(emu.arg(3)),
            };
            invoke(emu, a2, src);
        }
        163 => {
            // NewString(env, jchar*, len)
            let s: Vec<u16> = (0..a2).map(|i| emu.mem.r16(a1 + 2 * i)).collect();
            let h = emu.hle.jni.new_string(&String::from_utf16_lossy(&s));
            emu.ret(h);
        }
        164 => {
            let n = emu.hle.jni.string(a1).map(|s| s.encode_utf16().count()).unwrap_or(0);
            emu.ret(n as u32);
        }
        165 | 224 => {
            let s = emu.hle.jni.string(a1).unwrap_or_default();
            let u: Vec<u16> = s.encode_utf16().collect();
            let p = emu.hle.heap.malloc(&mut emu.mem, (u.len() as u32 + 1) * 2);
            for (i, c) in u.iter().enumerate() {
                emu.mem.w16(p + 2 * i as u32, *c);
            }
            emu.mem.w16(p + 2 * u.len() as u32, 0);
            if emu.arg(2) != 0 {
                emu.mem.w8(emu.arg(2), 1);
            }
            emu.ret(p);
        }
        166 | 170 | 225 => {
            emu.hle.heap.free(&mut emu.mem, a2);
        }
        167 => {
            let s = String::from_utf8_lossy(emu.mem.cstr_bytes(a1)).into_owned();
            let h = emu.hle.jni.new_string(&s);
            emu.ret(h);
        }
        168 => {
            let n = emu.hle.jni.string(a1).map(|s| s.len()).unwrap_or(0);
            emu.ret(n as u32);
        }
        169 => {
            let s = emu.hle.jni.string(a1).unwrap_or_default();
            let p = emu.hle.heap.malloc(&mut emu.mem, s.len() as u32 + 1);
            emu.mem.write_cstr(p, s.as_bytes());
            if a2 != 0 {
                emu.mem.w8(a2, 1);
            }
            emu.ret(p);
        }
        171 => {
            let n = match emu.hle.jni.get(a1) {
                Some(JObj::Bytes(b)) => b.len(),
                Some(JObj::Ints(b)) => b.len(),
                _ => 0,
            };
            emu.ret(n as u32);
        }
        176 | 175 => {
            let h = emu.hle.jni.new_obj(JObj::Bytes(vec![0; a1 as usize]));
            emu.ret(h);
        }
        179 => {
            let h = emu.hle.jni.new_obj(JObj::Ints(vec![0; a1 as usize]));
            emu.ret(h);
        }
        172 | 177 | 178 | 180..=182 => {
            let h = emu.hle.jni.new_obj(JObj::Other("array".into()));
            emu.ret(h);
        }
        173 => emu.ret(0),
        174 => {}
        184 | 183 | 222 => {
            // Get<Byte>ArrayElements / GetPrimitiveArrayCritical
            let data = match emu.hle.jni.get(a1) {
                Some(JObj::Bytes(b)) => b.clone(),
                Some(JObj::Ints(v)) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
                _ => Vec::new(),
            };
            let p = emu.hle.heap.malloc(&mut emu.mem, data.len() as u32 + 1);
            emu.mem.write_bytes(p, &data);
            emu.hle.jni.pinned.insert(p, a1);
            if a2 != 0 && idx != 222 {
                emu.mem.w8(a2, 1);
            }
            emu.ret(p);
        }
        187 => {
            let data = match emu.hle.jni.get(a1) {
                Some(JObj::Ints(v)) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
                _ => Vec::new(),
            };
            let p = emu.hle.heap.malloc(&mut emu.mem, data.len() as u32 + 1);
            emu.mem.write_bytes(p, &data);
            emu.hle.jni.pinned.insert(p, a1);
            emu.ret(p);
        }
        191..=198 | 223 => {
            // Release<Type>ArrayElements(env, array, elems, mode)
            let mode = if idx == 223 { emu.arg(3) } else { emu.arg(3) };
            if mode != 2 {
                let len = match emu.hle.jni.get(a1) {
                    Some(JObj::Bytes(b)) => b.len(),
                    Some(JObj::Ints(v)) => v.len() * 4,
                    _ => 0,
                };
                let bytes = emu.mem.slice(a2, len).to_vec();
                let i = ((a1 - HANDLE_BASE) / 4) as usize;
                match emu.hle.jni.objs.get_mut(i).and_then(|o| o.as_mut()) {
                    Some(JObj::Bytes(b)) => b.copy_from_slice(&bytes),
                    Some(JObj::Ints(v)) => {
                        for (k, x) in v.iter_mut().enumerate() {
                            *x = i32::from_le_bytes([bytes[k * 4], bytes[k * 4 + 1], bytes[k * 4 + 2], bytes[k * 4 + 3]]);
                        }
                    }
                    _ => {}
                }
            }
            if mode != 1 {
                emu.hle.jni.pinned.remove(&a2);
                emu.hle.heap.free(&mut emu.mem, a2);
            }
        }
        199..=206 => {
            // Get<Type>ArrayRegion(env, array, start, len, buf)
            let (start, len, buf) = (a2 as usize, emu.arg(3) as usize, emu.arg(4));
            let data: Vec<u8> = match emu.hle.jni.get(a1) {
                Some(JObj::Bytes(b)) => b.get(start..start + len).map(|s| s.to_vec()).unwrap_or_default(),
                Some(JObj::Ints(v)) => v.get(start..start + len).map(|s| s.iter().flat_map(|x| x.to_le_bytes()).collect()).unwrap_or_default(),
                _ => Vec::new(),
            };
            emu.mem.write_bytes(buf, &data);
        }
        207..=214 => {
            let (start, len, buf) = (a2 as usize, emu.arg(3) as usize, emu.arg(4));
            let i = ((a1.wrapping_sub(HANDLE_BASE)) / 4) as usize;
            let bytes = emu.mem.slice(buf, len * 4).to_vec();
            match emu.hle.jni.objs.get_mut(i).and_then(|o| o.as_mut()) {
                Some(JObj::Bytes(b)) => {
                    if start + len <= b.len() {
                        b[start..start + len].copy_from_slice(&bytes[..len]);
                    }
                }
                Some(JObj::Ints(v)) => {
                    for k in 0..len.min(v.len().saturating_sub(start)) {
                        v[start + k] = i32::from_le_bytes([bytes[k * 4], bytes[k * 4 + 1], bytes[k * 4 + 2], bytes[k * 4 + 3]]);
                    }
                }
                _ => {}
            }
        }
        215 | 216 | 217 | 218 => emu.ret(0),
        219 => {
            let vm = emu.hle.jni.vm;
            emu.mem.w32(a1, vm);
            emu.ret(0);
        }
        220 => {
            // GetStringRegion(env, str, start, len, jchar* buf)
            let s = emu.hle.jni.string(a1).unwrap_or_default();
            let u: Vec<u16> = s.encode_utf16().collect();
            let (start, len, buf) = (a2 as usize, emu.arg(3) as usize, emu.arg(4));
            for k in 0..len {
                emu.mem.w16(buf + 2 * k as u32, *u.get(start + k).unwrap_or(&0));
            }
        }
        221 => {
            let s = emu.hle.jni.string(a1).unwrap_or_default();
            let (start, len, buf) = (a2 as usize, emu.arg(3) as usize, emu.arg(4));
            let chars: String = s.chars().skip(start).take(len).collect();
            emu.mem.write_cstr(buf, chars.as_bytes());
        }
        226 => emu.ret(a1),
        227 => {}
        228 => emu.ret(0),
        229 => emu.ret(0),
        230 => emu.ret(0),
        231 => emu.ret64(u64::MAX),
        232 => emu.ret(1),
        _ => {
            let name = ENV_NAMES.get(idx as usize).copied().unwrap_or("?");
            emu.log(&format!("[jni] unimplemented JNIEnv function {} ({})", idx, name));
            emu.ret(0);
        }
    }
    Flow::Ret
}

const ENV_NAMES: &[&str] = &[
    "reserved0", "reserved1", "reserved2", "reserved3", "GetVersion", "DefineClass", "FindClass", "FromReflectedMethod",
    "FromReflectedField", "ToReflectedMethod", "GetSuperclass", "IsAssignableFrom", "ToReflectedField", "Throw", "ThrowNew",
    "ExceptionOccurred", "ExceptionDescribe", "ExceptionClear", "FatalError", "PushLocalFrame", "PopLocalFrame",
    "NewGlobalRef", "DeleteGlobalRef", "DeleteLocalRef", "IsSameObject", "NewLocalRef", "EnsureLocalCapacity",
    "AllocObject", "NewObject", "NewObjectV", "NewObjectA", "GetObjectClass", "IsInstanceOf", "GetMethodID",
];
