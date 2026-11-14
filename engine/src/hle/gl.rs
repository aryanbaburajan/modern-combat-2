//! GLES forwarding. Every GL import becomes a call into the platform with the
//! raw argument words; the browser side translates to WebGL.

use super::{Flow, HleFn};
use crate::Emu;
use std::collections::HashMap;

pub const GL_FUNCS: &[&str] = &[
    "glActiveTexture",
    "glAlphaFunc",
    "glAttachShader",
    "glBindBuffer",
    "glBindFramebuffer",
    "glBindFramebufferOES",
    "glBindRenderbuffer",
    "glBindRenderbufferOES",
    "glBindTexture",
    "glBlendColor",
    "glBlendEquation",
    "glBlendEquationOES",
    "glBlendFunc",
    "glBufferData",
    "glBufferSubData",
    "glCheckFramebufferStatus",
    "glCheckFramebufferStatusOES",
    "glClear",
    "glClearColor",
    "glClearDepthf",
    "glClearStencil",
    "glClientActiveTexture",
    "glClipPlanef",
    "glColor4f",
    "glColor4ub",
    "glColorMask",
    "glColorPointer",
    "glCompileShader",
    "glCompressedTexImage2D",
    "glCompressedTexImage3DOES",
    "glCompressedTexSubImage2D",
    "glCompressedTexSubImage3DOES",
    "glCopyTexImage2D",
    "glCopyTexSubImage2D",
    "glCreateProgram",
    "glCreateShader",
    "glCullFace",
    "glDeleteBuffers",
    "glDeleteFramebuffers",
    "glDeleteFramebuffersOES",
    "glDeleteProgram",
    "glDeleteRenderbuffers",
    "glDeleteRenderbuffersOES",
    "glDeleteShader",
    "glDeleteTextures",
    "glDepthFunc",
    "glDepthMask",
    "glDepthRangef",
    "glDisable",
    "glDisableClientState",
    "glDisableVertexAttribArray",
    "glDrawArrays",
    "glDrawElements",
    "glEnable",
    "glEnableClientState",
    "glEnableVertexAttribArray",
    "glFogf",
    "glFogfv",
    "glFramebufferRenderbuffer",
    "glFramebufferRenderbufferOES",
    "glFramebufferTexture2D",
    "glFramebufferTexture2DOES",
    "glFrontFace",
    "glFrustumf",
    "glGenBuffers",
    "glGenerateMipmap",
    "glGenerateMipmapOES",
    "glGenFramebuffers",
    "glGenFramebuffersOES",
    "glGenRenderbuffers",
    "glGenRenderbuffersOES",
    "glGenTextures",
    "glGetActiveAttrib",
    "glGetActiveUniform",
    "glGetAttribLocation",
    "glGetError",
    "glGetFloatv",
    "glGetIntegerv",
    "glGetProgramInfoLog",
    "glGetProgramiv",
    "glGetShaderInfoLog",
    "glGetShaderiv",
    "glGetString",
    "glGetTexEnviv",
    "glGetUniformLocation",
    "glHint",
    "glIsEnabled",
    "glLightf",
    "glLightfv",
    "glLightModelf",
    "glLightModelfv",
    "glLineWidth",
    "glLinkProgram",
    "glLoadIdentity",
    "glLoadMatrixf",
    "glMapBufferOES",
    "glMaterialf",
    "glMaterialfv",
    "glMatrixMode",
    "glMultiTexCoord4f",
    "glMultMatrixf",
    "glNormal3f",
    "glNormalPointer",
    "glOrthox",
    "glPixelStorei",
    "glPointParameterf",
    "glPointParameterfv",
    "glPointSize",
    "glPolygonOffset",
    "glPopMatrix",
    "glPushMatrix",
    "glReadPixels",
    "glRenderbufferStorage",
    "glRenderbufferStorageOES",
    "glRotatef",
    "glSampleCoverage",
    "glScalef",
    "glScissor",
    "glShadeModel",
    "glShaderSource",
    "glStencilFunc",
    "glStencilMask",
    "glStencilOp",
    "glTexCoordPointer",
    "glTexEnvf",
    "glTexEnvfv",
    "glTexEnvi",
    "glTexImage2D",
    "glTexImage3DOES",
    "glTexParameterf",
    "glTexParameteri",
    "glTexParameterx",
    "glTexSubImage2D",
    "glTexSubImage3DOES",
    "glTranslatef",
    "glUniform1f",
    "glUniform1fv",
    "glUniform1i",
    "glUniform1iv",
    "glUniform2fv",
    "glUniform2iv",
    "glUniform3fv",
    "glUniform3iv",
    "glUniform4fv",
    "glUniform4iv",
    "glUniformMatrix4fv",
    "glUnmapBufferOES",
    "glUseProgram",
    "glVertexAttrib4f",
    "glVertexAttribPointer",
    "glVertexPointer",
    "glViewport",
];

pub fn register(r: &mut HashMap<&'static str, HleFn>) {
    for n in GL_FUNCS {
        r.insert(n, gl_call);
    }
    r.insert("glGetString", gl_get_string);
    r.insert("glCompressedTexImage2D", gl_compressed_tex_image);
    r.insert("glCompressedTexSubImage2D", gl_compressed_tex_sub_image);
    r.insert("glMapBufferOES", gl_map_buffer);
    r.insert("glUnmapBufferOES", gl_unmap_buffer);
}

pub fn gl_id(name: &str) -> Option<u32> {
    GL_FUNCS.iter().position(|&n| n == name).map(|i| i as u32)
}

pub struct GlState {
    ids: HashMap<u32, u32>,
    strings: HashMap<u32, u32>,
    pub extensions: String,
    pub renderer: String,
    pub calls: u64,
    mapped: Option<(u32, u32, u32)>, // (target, ptr, size)
}

impl GlState {
    pub fn new() -> GlState {
        GlState {
            ids: HashMap::new(),
            strings: HashMap::new(),
            extensions: "GL_OES_rgb8_rgba8 GL_OES_depth24 GL_OES_packed_depth_stencil GL_OES_texture_npot GL_OES_element_index_uint GL_OES_mapbuffer GL_OES_standard_derivatives GL_OES_vertex_half_float GL_OES_texture_half_float GL_EXT_texture_format_BGRA8888 GL_OES_compressed_ETC1_RGB8_texture GL_IMG_texture_compression_pvrtc GL_EXT_texture_compression_dxt1 GL_EXT_texture_compression_s3tc GL_AMD_compressed_ATC_texture".into(),
            renderer: "PowerVR SGX 540".into(),
            calls: 0,
            mapped: None,
        }
    }
}

fn func_id(emu: &mut Emu) -> u32 {
    let idx = (emu.cpu.pc - crate::mem::THUNK_BASE) / 4;
    if let Some(&id) = emu.hle.gl.ids.get(&idx) {
        return id;
    }
    let id = gl_id(emu.hle.thunk_name(idx)).unwrap_or(u32::MAX);
    emu.hle.gl.ids.insert(idx, id);
    id
}

fn gl_call(emu: &mut Emu) -> Flow {
    let id = func_id(emu);
    let mut args = [0u32; 12];
    for (i, a) in args.iter_mut().enumerate() {
        *a = emu.arg(i);
    }
    emu.hle.gl.calls += 1;
    let r = emu.plat.gl(&mut emu.mem, id, &args);
    emu.ret(r);
    Flow::Ret
}

fn gl_get_string(emu: &mut Emu) -> Flow {
    let name = emu.arg(0);
    if let Some(&p) = emu.hle.gl.strings.get(&name) {
        emu.ret(p);
        return Flow::Ret;
    }
    let s = match name {
        0x1F00 => "Imagination Technologies".to_string(),
        0x1F01 => emu.hle.gl.renderer.clone(),
        0x1F02 => "OpenGL ES 2.0".to_string(),
        0x1F03 => emu.hle.gl.extensions.clone(),
        0x8B8C => "OpenGL ES GLSL ES 1.00".to_string(),
        _ => String::new(),
    };
    let p = emu.hle.static_alloc(s.len() as u32 + 1, 4);
    emu.mem.write_cstr(p, s.as_bytes());
    emu.hle.gl.strings.insert(name, p);
    emu.ret(p);
    Flow::Ret
}

fn gl_map_buffer(emu: &mut Emu) -> Flow {
    // Ask the platform for the bound buffer size (pseudo-call id 0xffff0001).
    let target = emu.arg(0);
    let size = emu.plat.gl(&mut emu.mem, 0xffff_0001, &[target]);
    let p = emu.hle.heap.malloc(&mut emu.mem, size.max(4));
    if p == 0 {
        emu.ret(0);
        return Flow::Ret;
    }
    // Preserve untouched bytes when the guest only updates part of a mapping.
    // The browser implements this pseudo-call with WebGL2 getBufferSubData.
    if size != 0 {
        emu.plat.gl(&mut emu.mem, 0xffff_0002, &[target, p, size]);
    }
    emu.hle.gl.mapped = Some((target, p, size));
    emu.ret(p);
    Flow::Ret
}

fn gl_unmap_buffer(emu: &mut Emu) -> Flow {
    if let Some((target, p, size)) = emu.hle.gl.mapped.take() {
        // glBufferSubData(target, 0, size, p)
        if let Some(id) = gl_id("glBufferSubData") {
            emu.plat.gl(&mut emu.mem, id, &[target, 0, size, p]);
        }
        emu.hle.heap.free(&mut emu.mem, p);
    }
    emu.ret(1);
    Flow::Ret
}

/// Decode a compressed texture we can't pass through. Returns RGBA8 pixels.
fn decode_compressed(emu: &Emu, fmt: u32, w: u32, h: u32, data: u32, size: u32) -> Option<Vec<u8>> {
    let src = emu.mem.slice(data, size as usize);
    match fmt {
        0x8C00 | 0x8C02 => Some(super::texdecode::decode_pvrtc(src, w, h, false)),
        0x8C01 | 0x8C03 => Some(super::texdecode::decode_pvrtc(src, w, h, true)),
        0x8D64 => Some(super::texdecode::decode_etc1(src, w, h)),
        _ => None,
    }
}

fn upload_rgba(emu: &mut Emu, sub: bool, args: [u32; 9], pixels: &[u8]) {
    let p = emu.hle.heap.malloc(&mut emu.mem, pixels.len() as u32);
    emu.mem.write_bytes(p, pixels);
    if sub {
        // glTexSubImage2D(target, level, x, y, w, h, format, type, pixels)
        let id = gl_id("glTexSubImage2D").unwrap();
        emu.plat.gl(&mut emu.mem, id, &[args[0], args[1], args[2], args[3], args[4], args[5], 0x1908, 0x1401, p]);
    } else {
        // glTexImage2D(target, level, internalformat, w, h, border, format, type, pixels)
        let id = gl_id("glTexImage2D").unwrap();
        emu.plat.gl(&mut emu.mem, id, &[args[0], args[1], 0x1908, args[3], args[4], 0, 0x1908, 0x1401, p]);
    }
    emu.hle.heap.free(&mut emu.mem, p);
}

fn gl_compressed_tex_image(emu: &mut Emu) -> Flow {
    // (target, level, internalformat, width, height, border, imageSize, data)
    let a: Vec<u32> = (0..8).map(|i| emu.arg(i)).collect();
    emu.hle.gl.calls += 1;
    if let Some(px) = decode_compressed(emu, a[2], a[3], a[4], a[7], a[6]) {
        upload_rgba(emu, false, [a[0], a[1], a[2], a[3], a[4], a[5], 0, 0, 0], &px);
        emu.ret(0);
        return Flow::Ret;
    }
    gl_call(emu)
}

fn gl_compressed_tex_sub_image(emu: &mut Emu) -> Flow {
    // (target, level, xoffset, yoffset, width, height, format, imageSize, data)
    let a: Vec<u32> = (0..9).map(|i| emu.arg(i)).collect();
    emu.hle.gl.calls += 1;
    if let Some(px) = decode_compressed(emu, a[6], a[4], a[5], a[8], a[7]) {
        upload_rgba(emu, true, [a[0], a[1], a[2], a[3], a[4], a[5], 0, 0, 0], &px);
        emu.ret(0);
        return Flow::Ret;
    }
    gl_call(emu)
}
