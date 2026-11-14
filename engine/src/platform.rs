//! Host services the emulated game needs. Implemented by the browser (wasm)
//! front-end and by the native test harness.

use crate::mem::Memory;

#[derive(Clone, Debug)]
pub enum JValue {
    Void,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(Option<String>),
    /// Byte array (returned to the guest as a jbyteArray).
    Bytes(Vec<u8>),
    Object(u32),
}

impl JValue {
    pub fn as_i32(&self) -> i32 {
        match *self {
            JValue::Int(v) => v,
            JValue::Long(v) => v as i32,
            JValue::Float(v) => v as i32,
            JValue::Double(v) => v as i32,
            JValue::Object(v) => v as i32,
            _ => 0,
        }
    }
}

pub trait Platform {
    /// Virtual LAN socket bridge. Negative results are Linux errno values.
    fn net(&mut self, _op: u32, _args: &[u32; 6], _memory: &mut Memory) -> i32 { -38 }
    fn log(&mut self, msg: &str);
    /// Monotonic milliseconds.
    fn now_ms(&mut self) -> f64;
    /// Wall-clock seconds since the Unix epoch.
    fn wall_time(&mut self) -> f64;

    /// Size of a read-only game data file (path relative to the data root,
    /// e.g. "data/3d.pak"), or None if missing.
    fn file_size(&mut self, path: &str) -> Option<u64>;
    /// Read from a read-only game data file.
    fn file_read(&mut self, path: &str, offset: u64, buf: &mut [u8]) -> usize;

    /// Persistent storage for files the game writes (saves, settings).
    fn save_load(&mut self, path: &str) -> Option<Vec<u8>>;
    fn save_store(&mut self, path: &str, data: &[u8]);
    fn save_delete(&mut self, path: &str);

    /// Forward a GL call. `args` are the raw 32-bit argument words (floats as
    /// bits); pointers are guest addresses into `mem`.
    fn gl(&mut self, mem: &mut Memory, func: u32, args: &[u32]) -> u32;

    /// A static Java method the native code invoked through JNI that the
    /// runtime does not handle itself (audio, platform hooks).
    fn java_call(&mut self, class: &str, name: &str, sig: &str, args: &[JValue]) -> JValue;
}
