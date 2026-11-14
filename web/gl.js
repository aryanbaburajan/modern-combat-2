// GLES 2.0 -> WebGL2 translation layer.
//
// The emulated game calls GL through `call(funcIndex, argWordsPtr)`. Every
// argument is a raw 32-bit word (floats as bits); pointers are guest
// addresses that we translate with `mem.base`.

const GL_ARRAY_BUFFER = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER = 0x8893;
const GL_UNSIGNED_BYTE = 0x1401;
const GL_UNSIGNED_SHORT = 0x1403;
const GL_UNSIGNED_INT = 0x1405;
const GL_FLOAT = 0x1406;
const GL_HALF_FLOAT_OES = 0x8d61;
const GL_BGRA_EXT = 0x80e1;

const typeSize = { 0x1400: 1, 0x1401: 1, 0x1402: 2, 0x1403: 2, 0x1404: 4, 0x1405: 4, 0x1406: 4, 0x140c: 4, 0x8d61: 2, 0x140b: 2 };

export class GLES {
  constructor(gl, mem, log) {
    this.gl = gl;
    this.mem = mem; // { u8(), u16(), u32(), i32(), f32(), base, str(ptr) }
    this.log = log;
    this.names = new Map(); // GL name -> WebGL object (all object kinds share our counter)
    this.nextName = 1;
    this.programs = new Map(); // program name -> info
    this.curProgram = null;
    this.boundArray = 0;
    this.boundElement = 0;
    this.elementShadow = new Map(); // element buffer name -> Uint8Array copy
    this.attribs = [];
    for (let i = 0; i < 16; i++) this.attribs.push({ enabled: false, client: false, size: 4, type: GL_FLOAT, norm: false, stride: 0, ptr: 0 });
    this.streamBufs = [];
    this.indexStream = gl.createBuffer();
    this.unpackAlign = 4;
    this.warned = new Set();
    this.boundTex = new Map(); // unit -> {2d}
    this.activeUnit = 0;
    this.texFormats = new Map(); // texture name -> {w,h}
    this.drawCalls = 0;
    this.curFbo = 0;
    this.funcs = {};
    this.installFuncs();
  }

  setNames(list) {
    this.table = list.map((n) => this.funcs[n] || this.stub(n));
  }

  stub(name) {
    return () => {
      if (!this.warned.has(name)) {
        this.warned.add(name);
        this.log(`[gl] unimplemented ${name}`);
      }
      return 0;
    };
  }

  call(id, argsPtr) {
    if (id === 0xffff0001) {
      const a = this.mem.u32()[argsPtr >> 2];
      return this.gl.getBufferParameter(a, this.gl.BUFFER_SIZE) | 0;
    }
    const f = this.table[id];
    if (!f) return 0;
    const i = argsPtr >> 2;
    const u = this.mem.u32();
    const r = f(u[i], u[i + 1], u[i + 2], u[i + 3], u[i + 4], u[i + 5], u[i + 6], u[i + 7], u[i + 8], u[i + 9], u[i + 10], u[i + 11]);
    return r === undefined ? 0 : r >>> 0;
  }

  // ---- helpers ----
  f(bits) {
    this._fb = this._fb || new Float32Array(new Uint32Array([0]).buffer);
    this._ub = this._ub || new Uint32Array(this._fb.buffer);
    this._ub[0] = bits;
    return this._fb[0];
  }
  newName(obj) {
    const n = this.nextName++;
    if (obj) obj._name = n;
    this.names.set(n, obj);
    return n;
  }
  obj(n) {
    return n === 0 ? null : this.names.get(n) || null;
  }
  bytes(ptr, len) {
    return new Uint8Array(this.mem.u8().buffer, this.mem.base + ptr, len);
  }
  copyBytes(ptr, len) {
    return this.bytes(ptr, len).slice();
  }
  writeI32(ptr, v) {
    const a = this.mem.base + ptr;
    if (a & 3) new DataView(this.mem.u8().buffer).setInt32(a, v, true);
    else this.mem.i32()[a >> 2] = v;
  }
  writeF32(ptr, v) {
    new DataView(this.mem.u8().buffer).setFloat32(this.mem.base + ptr, v, true);
  }
  writeStr(ptr, cap, s, lenPtr) {
    const enc = new TextEncoder().encode(s);
    const n = Math.max(0, Math.min(enc.length, cap - 1));
    if (cap > 0) {
      this.bytes(ptr, n + 1).set(enc.subarray(0, n));
      this.mem.u8()[this.mem.base + ptr + n] = 0;
    }
    if (lenPtr) this.writeI32(lenPtr, n);
  }
  gen(n, ptr, create) {
    for (let i = 0; i < n; i++) this.writeI32(ptr + 4 * i, this.newName(create()));
  }
  del(n, ptr, destroy) {
    const u = this.mem.u8();
    const dv = new DataView(u.buffer);
    for (let i = 0; i < n; i++) {
      const name = dv.getUint32(this.mem.base + ptr + 4 * i, true);
      const o = this.names.get(name);
      if (o) {
        destroy(o);
        this.names.delete(name);
      }
    }
  }
  pixelView(type, ptr, byteLen) {
    if (!ptr) return null;
    const off = this.mem.base + ptr;
    const buf = this.mem.u8().buffer;
    if (type === GL_UNSIGNED_BYTE || type === 0x1400) return new Uint8Array(buf, off, byteLen);
    if (type === GL_FLOAT) return off & 3 ? new Float32Array(this.copyBytes(ptr, byteLen).buffer) : new Float32Array(buf, off, byteLen >> 2);
    // 16-bit packed types and half floats
    return off & 1 ? new Uint16Array(this.copyBytes(ptr, byteLen).buffer) : new Uint16Array(buf, off, byteLen >> 1);
  }
  imageSize(w, h, format, type) {
    let bpp;
    if (type === 0x8033 || type === 0x8034 || type === 0x8363) bpp = 2; // 4444, 5551, 565
    else {
      const comps = { 0x1906: 1, 0x1909: 1, 0x190a: 2, 0x1907: 3, 0x1908: 4, 0x80e1: 4 }[format] || 4;
      bpp = comps * (typeSize[type] || 1);
    }
    const row = Math.ceil((w * bpp) / this.unpackAlign) * this.unpackAlign;
    return { size: row * (h - 1) + w * bpp, bpp, row };
  }

  // Upload client-side vertex arrays before a draw.
  prepareClientArrays(maxVertex) {
    const gl = this.gl;
    let touched = false;
    for (let i = 0; i < this.attribs.length; i++) {
      const a = this.attribs[i];
      if (!a.enabled || !a.client) continue;
      const elem = a.size * (typeSize[a.type] || 4);
      const stride = a.stride || elem;
      const len = maxVertex > 0 ? (maxVertex - 1) * stride + elem : 0;
      if (!this.streamBufs[i]) this.streamBufs[i] = gl.createBuffer();
      gl.bindBuffer(GL_ARRAY_BUFFER, this.streamBufs[i]);
      gl.bufferData(GL_ARRAY_BUFFER, this.bytes(a.ptr, len), gl.STREAM_DRAW);
      gl.vertexAttribPointer(i, a.size, a.type, a.norm, a.stride, 0);
      touched = true;
    }
    if (touched) gl.bindBuffer(GL_ARRAY_BUFFER, this.obj(this.boundArray));
  }
  anyClient() {
    for (const a of this.attribs) if (a.enabled && a.client) return true;
    return false;
  }

  installFuncs() {
    const gl = this.gl;
    const F = this.funcs;
    const self = this;
    const f = (b) => this.f(b);

    // ---- state ----
    F.glEnable = (c) => gl.enable(c);
    F.glDisable = (c) => gl.disable(c);
    F.glIsEnabled = (c) => (gl.isEnabled(c) ? 1 : 0);
    F.glBlendFunc = (s, d) => gl.blendFunc(s, d);
    F.glBlendEquation = (m) => gl.blendEquation(m);
    F.glBlendEquationOES = F.glBlendEquation;
    F.glBlendColor = (r, g, b, a) => gl.blendColor(f(r), f(g), f(b), f(a));
    F.glDepthFunc = (x) => gl.depthFunc(x);
    F.glDepthMask = (x) => gl.depthMask(!!x);
    F.glDepthRangef = (n, fa) => gl.depthRange(f(n), f(fa));
    F.glColorMask = (r, g, b, a) => gl.colorMask(!!r, !!g, !!b, !!a);
    F.glCullFace = (m) => gl.cullFace(m);
    F.glFrontFace = (m) => gl.frontFace(m);
    F.glLineWidth = (w) => gl.lineWidth(f(w));
    F.glPolygonOffset = (a, b) => gl.polygonOffset(f(a), f(b));
    F.glSampleCoverage = (v, inv) => gl.sampleCoverage(f(v), !!inv);
    F.glScissor = (x, y, w, h) => gl.scissor(x | 0, y | 0, w | 0, h | 0);
    F.glViewport = (x, y, w, h) => gl.viewport(x | 0, y | 0, w | 0, h | 0);
    F.glStencilFunc = (fn, ref, mask) => gl.stencilFunc(fn, ref | 0, mask);
    F.glStencilMask = (m) => gl.stencilMask(m);
    F.glStencilOp = (a, b, c) => gl.stencilOp(a, b, c);
    F.glClear = (m) => gl.clear(m);
    F.glClearColor = (r, g, b, a) => gl.clearColor(f(r), f(g), f(b), f(a));
    F.glClearDepthf = (d) => gl.clearDepth(f(d));
    F.glClearStencil = (s) => gl.clearStencil(s | 0);
    F.glHint = (t, m) => {
      if (t === 0x8192 || t === 0x8b8b) gl.hint(t, m);
    };
    F.glPixelStorei = (p, v) => {
      if (p === 0x0cf5) this.unpackAlign = v;
      if (p === 0x0cf5 || p === 0x0d05) gl.pixelStorei(p, v);
    };
    F.glGetError = () => gl.getError();
    F.glFinish = () => gl.finish();
    F.glFlush = () => gl.flush();

    F.glGetIntegerv = (pname, ptr) => this.getv(pname, ptr, false);
    F.glGetFloatv = (pname, ptr) => this.getv(pname, ptr, true);

    // ---- buffers ----
    F.glGenBuffers = (n, p) => this.gen(n, p, () => gl.createBuffer());
    F.glDeleteBuffers = (n, p) =>
      this.del(n, p, (o) => {
        this.elementShadow.delete(o._name);
        if (this.boundArray === o._name) this.boundArray = 0;
        if (this.boundElement === o._name) this.boundElement = 0;
        gl.deleteBuffer(o);
      });
    F.glBindBuffer = (t, n) => {
      if (t === GL_ARRAY_BUFFER) this.boundArray = n;
      else if (t === GL_ELEMENT_ARRAY_BUFFER) this.boundElement = n;
      let o = this.obj(n);
      if (n && !o) {
        o = gl.createBuffer();
        o._name = n;
        this.names.set(n, o);
      }
      gl.bindBuffer(t, o);
    };
    F.glBufferData = (t, size, data, usage) => {
      if (data) {
        const b = this.bytes(data, size);
        gl.bufferData(t, b, usage);
        if (t === GL_ELEMENT_ARRAY_BUFFER) this.elementShadow.set(this.boundElement, b.slice());
      } else {
        gl.bufferData(t, size, usage);
        if (t === GL_ELEMENT_ARRAY_BUFFER) this.elementShadow.set(this.boundElement, new Uint8Array(size));
      }
    };
    F.glBufferSubData = (t, off, size, data) => {
      const b = this.bytes(data, size);
      gl.bufferSubData(t, off, b);
      if (t === GL_ELEMENT_ARRAY_BUFFER) {
        const sh = this.elementShadow.get(this.boundElement);
        if (sh && off + size <= sh.length) sh.set(b, off);
      }
    };

    // ---- vertex attributes ----
    F.glEnableVertexAttribArray = (i) => {
      this.attribs[i].enabled = true;
      gl.enableVertexAttribArray(i);
    };
    F.glDisableVertexAttribArray = (i) => {
      this.attribs[i].enabled = false;
      gl.disableVertexAttribArray(i);
    };
    F.glVertexAttribPointer = (i, size, type, norm, stride, ptr) => {
      const a = this.attribs[i];
      a.size = size;
      a.type = type === GL_HALF_FLOAT_OES ? gl.HALF_FLOAT : type;
      a.norm = !!norm;
      a.stride = stride;
      a.ptr = ptr;
      a.client = this.boundArray === 0;
      if (!a.client) gl.vertexAttribPointer(i, size, a.type, a.norm, stride, ptr);
    };
    F.glVertexAttrib4f = (i, x, y, z, w) => gl.vertexAttrib4f(i, f(x), f(y), f(z), f(w));

    // ---- drawing ----
    F.glDrawArrays = (mode, first, count) => {
      if (this.anyClient()) this.prepareClientArrays(first + count);
      this.drawCalls++;
      gl.drawArrays(mode, first, count);
    };
    F.glDrawElements = (mode, count, type, indices) => {
      const isz = type === GL_UNSIGNED_BYTE ? 1 : type === GL_UNSIGNED_SHORT ? 2 : 4;
      const client = this.boundElement === 0;
      let idxBytes = null;
      if (client) idxBytes = this.copyBytes(indices, count * isz);
      if (this.anyClient()) {
        let src = idxBytes;
        let off = 0;
        if (!src) {
          src = this.elementShadow.get(this.boundElement);
          off = indices;
        }
        let max = 0;
        if (src) {
          const dv = new DataView(src.buffer, src.byteOffset);
          for (let k = 0; k < count; k++) {
            const p = off + k * isz;
            if (p + isz > src.length) break;
            const v = isz === 1 ? src[p] : isz === 2 ? dv.getUint16(p, true) : dv.getUint32(p, true);
            if (v > max) max = v;
          }
        }
        this.prepareClientArrays(max + 1);
      }
      this.drawCalls++;
      if (client) {
        gl.bindBuffer(GL_ELEMENT_ARRAY_BUFFER, this.indexStream);
        gl.bufferData(GL_ELEMENT_ARRAY_BUFFER, idxBytes, gl.STREAM_DRAW);
        gl.drawElements(mode, count, type, 0);
        gl.bindBuffer(GL_ELEMENT_ARRAY_BUFFER, null);
      } else {
        gl.drawElements(mode, count, type, indices);
      }
    };

    // ---- textures ----
    F.glGenTextures = (n, p) => this.gen(n, p, () => gl.createTexture());
    F.glDeleteTextures = (n, p) => this.del(n, p, (o) => gl.deleteTexture(o));
    F.glActiveTexture = (u) => {
      this.activeUnit = u - 0x84c0;
      gl.activeTexture(u);
    };
    F.glBindTexture = (t, n) => {
      let o = this.obj(n);
      if (n && !o) {
        o = gl.createTexture();
        o._name = n;
        this.names.set(n, o);
      }
      gl.bindTexture(t, o);
    };
    F.glTexParameteri = (t, p, v) => {
      if (p === 0x8191) return; // GL_GENERATE_MIPMAP (ES1)
      gl.texParameteri(t, p, v);
    };
    F.glTexParameterx = F.glTexParameteri;
    F.glTexParameterf = (t, p, v) => {
      if (p === 0x84fe) gl.texParameterf(t, p, f(v)); // anisotropy
      else gl.texParameterf(t, p, f(v));
    };
    F.glTexImage2D = (t, level, ifmt, w, h, border, format, type, pixels) => {
      this.texImage(t, level, ifmt, w, h, format, type, pixels, false, 0, 0);
    };
    F.glTexSubImage2D = (t, level, x, y, w, h, format, type, pixels) => {
      this.texImage(t, level, format, w, h, format, type, pixels, true, x, y);
    };
    F.glCompressedTexImage2D = (t, level, ifmt, w, h, border, size, data) => {
      gl.compressedTexImage2D(t, level, ifmt, w, h, 0, this.bytes(data, size));
    };
    F.glCompressedTexSubImage2D = (t, level, x, y, w, h, fmt, size, data) => {
      gl.compressedTexSubImage2D(t, level, x, y, w, h, fmt, this.bytes(data, size));
    };
    F.glCopyTexImage2D = (t, level, ifmt, x, y, w, h, border) => gl.copyTexImage2D(t, level, ifmt, x | 0, y | 0, w, h, 0);
    F.glCopyTexSubImage2D = (t, level, xo, yo, x, y, w, h) => gl.copyTexSubImage2D(t, level, xo, yo, x | 0, y | 0, w, h);
    F.glGenerateMipmap = (t) => gl.generateMipmap(t);
    F.glGenerateMipmapOES = F.glGenerateMipmap;
    F.glReadPixels = (x, y, w, h, format, type, ptr) => {
      const n = w * h * (type === GL_UNSIGNED_BYTE ? (format === 0x1908 ? 4 : 3) : 2);
      const buf = new Uint8Array(n);
      gl.readPixels(x, y, w, h, format, type, type === GL_UNSIGNED_BYTE ? buf : new Uint16Array(buf.buffer));
      this.bytes(ptr, n).set(buf);
    };

    // ---- framebuffers ----
    const genFb = (n, p) => this.gen(n, p, () => gl.createFramebuffer());
    const genRb = (n, p) => this.gen(n, p, () => gl.createRenderbuffer());
    F.glGenFramebuffers = F.glGenFramebuffersOES = genFb;
    F.glGenRenderbuffers = F.glGenRenderbuffersOES = genRb;
    F.glDeleteFramebuffers = F.glDeleteFramebuffersOES = (n, p) => this.del(n, p, (o) => gl.deleteFramebuffer(o));
    F.glDeleteRenderbuffers = F.glDeleteRenderbuffersOES = (n, p) => this.del(n, p, (o) => gl.deleteRenderbuffer(o));
    F.glBindFramebuffer = F.glBindFramebufferOES = (t, n) => {
      this.curFbo = n;
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.obj(n));
    };
    F.glBindRenderbuffer = F.glBindRenderbufferOES = (t, n) => gl.bindRenderbuffer(gl.RENDERBUFFER, this.obj(n));
    F.glRenderbufferStorage = F.glRenderbufferStorageOES = (t, fmt, w, h) => {
      const map = { 0x81a5: gl.DEPTH_COMPONENT16, 0x81a6: gl.DEPTH_COMPONENT24, 0x88f0: gl.DEPTH24_STENCIL8, 0x8d48: gl.STENCIL_INDEX8, 0x8056: gl.RGBA4, 0x8057: gl.RGB5_A1, 0x8d62: gl.RGB565, 0x8051: gl.RGB8, 0x8058: gl.RGBA8 };
      gl.renderbufferStorage(gl.RENDERBUFFER, map[fmt] || fmt, w, h);
    };
    F.glFramebufferRenderbuffer = F.glFramebufferRenderbufferOES = (t, att, rt, n) => {
      if (att === 0x8d00 && n && this.obj(n) && this.obj(n)._ds) att = gl.DEPTH_STENCIL_ATTACHMENT;
      gl.framebufferRenderbuffer(gl.FRAMEBUFFER, att, gl.RENDERBUFFER, this.obj(n));
    };
    F.glFramebufferTexture2D = F.glFramebufferTexture2DOES = (t, att, tt, n, level) => gl.framebufferTexture2D(gl.FRAMEBUFFER, att, tt, this.obj(n), level);
    F.glCheckFramebufferStatus = F.glCheckFramebufferStatusOES = (t) => gl.checkFramebufferStatus(gl.FRAMEBUFFER);

    // ---- shaders ----
    F.glCreateShader = (type) => this.newName(gl.createShader(type));
    F.glDeleteShader = (n) => {
      const o = this.obj(n);
      if (o) gl.deleteShader(o);
    };
    F.glShaderSource = (n, count, strs, lens) => {
      let src = "";
      const u = this.mem.u8();
      const dv = new DataView(u.buffer);
      for (let i = 0; i < count; i++) {
        const p = dv.getUint32(this.mem.base + strs + 4 * i, true);
        const l = lens ? dv.getInt32(this.mem.base + lens + 4 * i, true) : -1;
        src += l >= 0 ? new TextDecoder().decode(this.bytes(p, l)) : this.mem.str(p);
      }
      const o = this.obj(n);
      o._src = src;
      gl.shaderSource(o, this.fixShader(src, gl.getShaderParameter(o, gl.SHADER_TYPE)));
    };
    F.glCompileShader = (n) => {
      const o = this.obj(n);
      gl.compileShader(o);
      if (!gl.getShaderParameter(o, gl.COMPILE_STATUS)) {
        this.log(`[gl] shader compile failed: ${gl.getShaderInfoLog(o)}\n${o._src}`);
      }
    };
    F.glGetShaderiv = (n, pname, ptr) => {
      const o = this.obj(n);
      let v = 0;
      if (pname === 0x8b84) v = (gl.getShaderInfoLog(o) || "").length + 1; // INFO_LOG_LENGTH
      else if (pname === 0x8b88) v = (o._src || "").length + 1; // SHADER_SOURCE_LENGTH
      else {
        const r = gl.getShaderParameter(o, pname);
        v = typeof r === "boolean" ? (r ? 1 : 0) : r | 0;
      }
      this.writeI32(ptr, v);
    };
    F.glGetShaderInfoLog = (n, cap, lenPtr, ptr) => this.writeStr(ptr, cap, gl.getShaderInfoLog(this.obj(n)) || "", lenPtr);
    F.glCreateProgram = () => {
      const p = gl.createProgram();
      const n = this.newName(p);
      this.programs.set(n, { prog: p, locs: [null], byName: new Map() });
      return n;
    };
    F.glDeleteProgram = (n) => {
      const o = this.obj(n);
      if (o) gl.deleteProgram(o);
      this.programs.delete(n);
    };
    F.glAttachShader = (p, s) => gl.attachShader(this.obj(p), this.obj(s));
    F.glLinkProgram = (n) => {
      const o = this.obj(n);
      gl.linkProgram(o);
      const info = this.programs.get(n);
      info.locs = [null];
      info.byName = new Map();
      if (!gl.getProgramParameter(o, gl.LINK_STATUS)) this.log(`[gl] link failed: ${gl.getProgramInfoLog(o)}`);
    };
    F.glUseProgram = (n) => {
      this.curProgram = n ? this.programs.get(n) : null;
      gl.useProgram(this.obj(n));
    };
    F.glGetProgramiv = (n, pname, ptr) => {
      const o = this.obj(n);
      let v = 0;
      if (pname === 0x8b84) v = (gl.getProgramInfoLog(o) || "").length + 1;
      else if (pname === 0x8b87 || pname === 0x8b8a) {
        // ACTIVE_UNIFORM_MAX_LENGTH / ACTIVE_ATTRIBUTE_MAX_LENGTH
        const uni = pname === 0x8b87;
        const cnt = gl.getProgramParameter(o, uni ? gl.ACTIVE_UNIFORMS : gl.ACTIVE_ATTRIBUTES);
        for (let i = 0; i < cnt; i++) {
          const a = uni ? gl.getActiveUniform(o, i) : gl.getActiveAttrib(o, i);
          if (a) v = Math.max(v, a.name.length + 1);
        }
      } else {
        const r = gl.getProgramParameter(o, pname);
        v = typeof r === "boolean" ? (r ? 1 : 0) : r | 0;
      }
      this.writeI32(ptr, v);
    };
    F.glGetProgramInfoLog = (n, cap, lenPtr, ptr) => this.writeStr(ptr, cap, gl.getProgramInfoLog(this.obj(n)) || "", lenPtr);
    const active = (uni) => (n, index, cap, lenPtr, sizePtr, typePtr, namePtr) => {
      const o = this.obj(n);
      const a = uni ? gl.getActiveUniform(o, index) : gl.getActiveAttrib(o, index);
      if (!a) return;
      if (sizePtr) this.writeI32(sizePtr, a.size);
      if (typePtr) this.writeI32(typePtr, a.type);
      this.writeStr(namePtr, cap, a.name, lenPtr);
    };
    F.glGetActiveUniform = active(true);
    F.glGetActiveAttrib = active(false);
    F.glGetAttribLocation = (n, name) => gl.getAttribLocation(this.obj(n), this.mem.str(name));
    F.glGetUniformLocation = (n, namePtr) => {
      const info = this.programs.get(n);
      if (!info) return -1;
      const name = this.mem.str(namePtr);
      if (info.byName.has(name)) return info.byName.get(name);
      const loc = gl.getUniformLocation(info.prog, name);
      if (!loc) {
        info.byName.set(name, -1);
        return -1;
      }
      const id = info.locs.length;
      info.locs.push(loc);
      info.byName.set(name, id);
      return id;
    };
    const loc = (l) => (this.curProgram && (l | 0) > 0 ? this.curProgram.locs[l] : null);
    const fv = (ptr, n) => {
      const off = this.mem.base + ptr;
      return off & 3 ? new Float32Array(this.copyBytes(ptr, n * 4).buffer) : new Float32Array(this.mem.u8().buffer, off, n);
    };
    const iv = (ptr, n) => {
      const off = this.mem.base + ptr;
      return off & 3 ? new Int32Array(this.copyBytes(ptr, n * 4).buffer) : new Int32Array(this.mem.u8().buffer, off, n);
    };
    F.glUniform1f = (l, x) => gl.uniform1f(loc(l), f(x));
    F.glUniform1i = (l, x) => gl.uniform1i(loc(l), x | 0);
    F.glUniform1fv = (l, c, p) => gl.uniform1fv(loc(l), fv(p, c));
    F.glUniform2fv = (l, c, p) => gl.uniform2fv(loc(l), fv(p, c * 2));
    F.glUniform3fv = (l, c, p) => gl.uniform3fv(loc(l), fv(p, c * 3));
    F.glUniform4fv = (l, c, p) => gl.uniform4fv(loc(l), fv(p, c * 4));
    F.glUniform1iv = (l, c, p) => gl.uniform1iv(loc(l), iv(p, c));
    F.glUniform2iv = (l, c, p) => gl.uniform2iv(loc(l), iv(p, c * 2));
    F.glUniform3iv = (l, c, p) => gl.uniform3iv(loc(l), iv(p, c * 3));
    F.glUniform4iv = (l, c, p) => gl.uniform4iv(loc(l), iv(p, c * 4));
    F.glUniformMatrix4fv = (l, c, tr, p) => gl.uniformMatrix4fv(loc(l), false, fv(p, c * 16));

    // GLES1 fixed-function entry points the GLES2 renderer shouldn't use.
    for (const n of ["glAlphaFunc", "glClientActiveTexture", "glClipPlanef", "glColor4f", "glColor4ub", "glColorPointer", "glDisableClientState", "glEnableClientState", "glFogf", "glFogfv", "glFrustumf", "glGetTexEnviv", "glLightf", "glLightfv", "glLightModelf", "glLightModelfv", "glLoadIdentity", "glLoadMatrixf", "glMaterialf", "glMaterialfv", "glMatrixMode", "glMultiTexCoord4f", "glMultMatrixf", "glNormal3f", "glNormalPointer", "glOrthox", "glPointParameterf", "glPointParameterfv", "glPointSize", "glPopMatrix", "glPushMatrix", "glRotatef", "glScalef", "glShadeModel", "glTexCoordPointer", "glTexEnvf", "glTexEnvfv", "glTexEnvi", "glTranslatef", "glVertexPointer"]) {
      F[n] = this.stub(n);
    }
  }

  fixShader(src, type) {
    // Mobile drivers tolerated junk after #endif/#else (e.g. "#endif]").
    src = src.replace(/^([ \t]*#[ \t]*(?:endif|else))\b[^\n]*$/gm, "$1");
    // ESSL 1.00 fragment shaders need a default float precision.
    if (type === this.gl.FRAGMENT_SHADER && !/precision\s+\w+\s+float/.test(src)) {
      const lines = src.split("\n");
      let i = 0;
      while (i < lines.length && /^\s*(#version|#extension|$)/.test(lines[i])) i++;
      lines.splice(i, 0, "precision mediump float;");
      src = lines.join("\n");
    }
    return src;
  }

  texImage(t, level, ifmt, w, h, format, type, pixels, sub, x, y) {
    const gl = this.gl;
    let data = null;
    if (pixels) {
      const sz = this.imageSize(w, h, format, type);
      if (format === GL_BGRA_EXT) {
        // swizzle to RGBA
        const src = this.bytes(pixels, sz.size);
        const out = new Uint8Array(w * h * 4);
        for (let r = 0; r < h; r++) {
          for (let c = 0; c < w; c++) {
            const s = r * sz.row + c * 4;
            const d = (r * w + c) * 4;
            out[d] = src[s + 2];
            out[d + 1] = src[s + 1];
            out[d + 2] = src[s];
            out[d + 3] = src[s + 3];
          }
        }
        data = out;
        format = gl.RGBA;
        if (!sub) ifmt = gl.RGBA;
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
      } else {
        data = this.pixelView(type, pixels, sz.size);
      }
    } else if (format === GL_BGRA_EXT) {
      format = gl.RGBA;
      ifmt = gl.RGBA;
    }
    if (type === GL_HALF_FLOAT_OES) {
      type = gl.HALF_FLOAT;
      if (!sub) ifmt = { 0x1908: gl.RGBA16F, 0x1907: gl.RGB16F, 0x1909: gl.R16F }[ifmt] || gl.RGBA16F;
    } else if (type === GL_FLOAT && !sub) {
      ifmt = { 0x1908: gl.RGBA32F, 0x1907: gl.RGB32F }[ifmt] || ifmt;
    }
    if (sub) gl.texSubImage2D(t, level, x, y, w, h, format, type, data);
    else gl.texImage2D(t, level, ifmt, w, h, 0, format, type, data);
    if (format === gl.RGBA && data && pixels && this.unpackAlign !== 4) gl.pixelStorei(gl.UNPACK_ALIGNMENT, this.unpackAlign);
  }

  getv(pname, ptr, float) {
    const gl = this.gl;
    const w = (i, v) => (float ? this.writeF32(ptr + 4 * i, v) : this.writeI32(ptr + 4 * i, typeof v === "boolean" ? (v ? 1 : 0) : Math.trunc(v)));
    switch (pname) {
      case 0x86a2: // NUM_COMPRESSED_TEXTURE_FORMATS
        w(0, 0);
        return;
      case 0x86a3: // COMPRESSED_TEXTURE_FORMATS
        return;
      case 0x8b9a: // IMPLEMENTATION_COLOR_READ_TYPE
        w(0, gl.UNSIGNED_BYTE);
        return;
      case 0x8b9b:
        w(0, gl.RGBA);
        return;
      case 0x8dfa: // SHADER_COMPILER
        w(0, 1);
        return;
      case 0x8df9: // NUM_SHADER_BINARY_FORMATS
        w(0, 0);
        return;
      case 0x8ca6: // FRAMEBUFFER_BINDING
        w(0, this.curFbo);
        return;
      case 0x8b8d: // CURRENT_PROGRAM
      case 0x8069: // TEXTURE_BINDING_2D
      case 0x8894: // ARRAY_BUFFER_BINDING
      case 0x8895: // ELEMENT_ARRAY_BUFFER_BINDING
      case 0x8ca7: {
        // RENDERBUFFER_BINDING
        const o = gl.getParameter(pname);
        w(0, o ? o._name | 0 : 0);
        return;
      }
    }
    let v;
    try {
      v = gl.getParameter(pname);
    } catch (e) {
      v = null;
    }
    if (v === null || v === undefined) {
      if (!this.warned.has("get" + pname)) {
        this.warned.add("get" + pname);
        this.log(`[gl] glGet unknown pname 0x${pname.toString(16)}`);
      }
      w(0, 0);
      return;
    }
    if (typeof v === "number" || typeof v === "boolean") w(0, v);
    else if (v.length !== undefined) for (let i = 0; i < v.length; i++) w(i, v[i]);
    else w(0, 0);
  }
}
