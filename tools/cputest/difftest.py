#!/usr/bin/env python3
"""Differential tester: mc2engine Thumb-2/VFP interpreter vs Unicorn.

Enumerates every Thumb instruction in libsandstorm2.so's .text (using $t/$a/$d
mapping symbols), dedupes by encoding, runs each unique encoding from N random
initial states on Unicorn (single step), replays the same states through the
Rust `cputest` binary and reports mismatches grouped by mnemonic.

usage (run with the venv python and -I):
  /tmp/mc2/venv/bin/python -I tools/cputest/difftest.py [--cases 3] [--jobs 10]
      [--filter REGEX] [--limit N] [--seed S] [--out DIR]
"""
import argparse, json, multiprocessing as mp, os, random, re, struct, subprocess, sys, time
from collections import defaultdict

from elftools.elf.elffile import ELFFile
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB
from unicorn import Uc, UcError, UC_ARCH_ARM, UC_MODE_THUMB, UC_PROT_READ, UC_PROT_WRITE, UC_PROT_EXEC
from unicorn import UC_ERR_FETCH_UNMAPPED, UC_ERR_FETCH_PROT
from unicorn.arm_const import *

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..'))
ENGINE = os.path.join(ROOT, 'engine')
DEFAULT_LIB = '/tmp/mc2/apk/lib/armeabi-v7a/libsandstorm2.so'

LIB_BASE = 0x100000
WIN_BASE = 0x0FFFC000
WIN_SIZE = 0x8000
PTR_LO, PTR_HI = 0x0FFFF000, 0x10001000
NWIN = 32

SKIP_MN = re.compile(r'^(it[te]*|svc|bkpt|udf|wfi|wfe|sev|yield|cps.*|srs.*|rfe.*|smc|hvc|eret|mrc2?|mcr2?|mrrc2?|mcrr2?|cdp2?|ldc2?l?|stc2?l?)(\.w)?$')

CASE_FMT = struct.Struct('<3I16I2I64II')          # id addr insn r[16] cpsr fpscr s[64] win
EXP_FMT = struct.Struct('<16III64II')              # r[16] cpsr fpscr s[64] pc


# ---------------------------------------------------------------- enumeration
def load_lib(path):
    f = open(path, 'rb')
    elf = ELFFile(f)
    segs = []
    for s in elf.iter_segments():
        if s['p_type'] == 'PT_LOAD':
            segs.append((s['p_vaddr'] + LIB_BASE, s.data()[:s['p_filesz']]))
    text = elf.get_section_by_name('.text')
    tidx = elf.get_section_index('.text')
    tstart, tsize = text['sh_addr'], text['sh_size']
    tdata = text.data()
    maps = []
    for sym in elf.get_section_by_name('.symtab').iter_symbols():
        n = sym.name
        if sym['st_shndx'] == tidx and len(n) >= 2 and n[0] == '$' and n[1] in 'tad' and (len(n) == 2 or n[2] == '.'):
            maps.append((sym['st_value'] & ~1, n[1]))
    maps.sort()
    return segs, tstart, tsize, tdata, maps


def enumerate_thumb(tstart, tsize, tdata, maps):
    uniq = {}  # key bytes -> [vaddr, count]
    total = 0
    tend = tstart + tsize
    bounds = [a for a, _ in maps] + [tend]
    for i, (a, k) in enumerate(maps):
        if k != 't':
            continue
        end = min(bounds[i + 1], tend)
        pc = a
        while pc + 2 <= end:
            off = pc - tstart
            hw1 = tdata[off] | tdata[off + 1] << 8
            if (hw1 >> 11) in (0x1d, 0x1e, 0x1f):
                if pc + 4 > end:
                    break
                key = bytes(tdata[off:off + 4]); sz = 4
            else:
                key = bytes(tdata[off:off + 2]); sz = 2
            total += 1
            e = uniq.get(key)
            if e is None:
                uniq[key] = [pc, 1]
            else:
                e[1] += 1
            pc += sz
    return uniq, total


# ---------------------------------------------------------------- random state
def f2u(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


def d2u(x):
    return struct.unpack('<Q', struct.pack('<d', x))[0]


def nice_float(rng):
    r = rng.random()
    if r < 0.5:
        return rng.uniform(-1000, 1000)
    if r < 0.8:
        return rng.choice([-1, 1]) * rng.random() * 2.0 ** rng.randint(-20, 20)
    return float(rng.randint(-100, 100)) / rng.choice([1, 2, 4, 8])


def rand_dreg(rng):
    k = rng.random()
    if k < 0.30:
        return f2u(nice_float(rng)) | f2u(nice_float(rng)) << 32
    if k < 0.60:
        return d2u(nice_float(rng))
    if k < 0.72:
        return (rng.randint(-1000, 1000) & 0xffffffff) | (rng.randint(-1000, 1000) & 0xffffffff) << 32
    if k < 0.90:
        return rng.getrandbits(64)
    sp32 = [0, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00000, 0x7f800001, 0x00000001, 0x3f800000, 0x007fffff]
    sp64 = [0, 1 << 63, 0x7ff0000000000000, 0x7ff8000000000000, 0x7ff0000000000001, 1, 0x3ff0000000000000, 0x41e0000000000000]
    if rng.random() < 0.5:
        return rng.choice(sp32) | rng.choice(sp32) << 32
    return rng.choice(sp64)


def rand_reg(rng, ptr_bias):
    if ptr_bias < 0:  # "index" mode: pointers or small word-aligned offsets
        return rng.randrange(PTR_LO, PTR_HI, 4) if rng.random() < 0.5 else rng.randrange(0, 256, 4)
    r = rng.random()
    if r < ptr_bias:
        return rng.randrange(PTR_LO, PTR_HI, 4)
    if r < ptr_bias + 0.05:
        return rng.randrange(PTR_LO, PTR_HI)
    if r < ptr_bias + (1 - ptr_bias) * 0.55:
        return rng.randint(-16, 64) & 0xffffffff
    return rng.getrandbits(32)


def make_windows(seed):
    rng = random.Random(seed ^ 0x5eed)
    wins = []
    for i in range(NWIN):
        if i % 2 == 0:
            wins.append(rng.randbytes(WIN_SIZE))
        else:
            w = bytearray()
            for _ in range(WIN_SIZE // 4):
                k = rng.random()
                if k < 0.35:
                    v = f2u(nice_float(rng))
                elif k < 0.6:
                    v = rng.randrange(PTR_LO, PTR_HI, 4)
                elif k < 0.8:
                    v = rng.randint(-16, 300) & 0xffffffff
                else:
                    v = rng.getrandbits(32)
                w += struct.pack('<I', v)
            wins.append(bytes(w))
    return wins


# ---------------------------------------------------------------- unicorn worker
G = {}

R_CORE = [UC_ARM_REG_R0 + i for i in range(13)] + [UC_ARM_REG_SP, UC_ARM_REG_LR]
R_D = [UC_ARM_REG_D0 + i for i in range(32)]


def worker_init(lib, seed):
    segs, *_ = load_lib(lib)
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB)
    uc.ctl_set_cpu_model(UC_CPU_ARM_CORTEX_A8)
    lo = min(a for a, _ in segs) & ~0xfff
    hi = (max(a + len(d) for a, d in segs) + 0xfff) & ~0xfff
    uc.mem_map(lo, hi - lo, UC_PROT_READ | UC_PROT_EXEC)
    for a, d in segs:
        uc.mem_write(a, d)
    uc.mem_map(WIN_BASE, WIN_SIZE, UC_PROT_READ | UC_PROT_WRITE)
    uc.reg_write(UC_ARM_REG_C1_C0_2, uc.reg_read(UC_ARM_REG_C1_C0_2) | (0xf << 20))
    uc.reg_write(UC_ARM_REG_FPEXC, 0x40000000)
    uc.reg_write(UC_ARM_REG_CPSR, 0x10 | 0x20)  # user mode, thumb
    G['uc'] = uc
    G['wins'] = make_windows(seed)
    G['seed'] = seed


def diff_bytes(a, b):
    out = []
    n = len(a)
    C = 256
    for o in range(0, n, C):
        if a[o:o + C] != b[o:o + C]:
            for i in range(o, min(o + C, n)):
                if a[i] != b[i]:
                    out.append((i, b[i]))
    return out


def run_unicorn(uc, addr, regs, cpsr, fpscr, dregs, win):
    uc.reg_write_batch([(UC_ARM_REG_CPSR, cpsr | 0x10 | 0x20)] + list(zip(R_CORE, regs[:15])) +
                       [(UC_ARM_REG_FPSCR, fpscr), (UC_ARM_REG_FPEXC, 0x40000000)] + list(zip(R_D, dregs)))
    uc.mem_write(WIN_BASE, win)
    try:
        uc.emu_start(addr | 1, 0xffffffff, count=1)
    except UcError as e:
        if e.errno not in (UC_ERR_FETCH_UNMAPPED, UC_ERR_FETCH_PROT):
            return None, str(e)
    vals = uc.reg_read_batch(R_CORE + [UC_ARM_REG_PC, UC_ARM_REG_CPSR, UC_ARM_REG_FPSCR] + R_D)
    out_regs = list(vals[:16])
    ocpsr, ofpscr = vals[16], vals[17]
    od = vals[18:]
    nwin = uc.mem_read(WIN_BASE, WIN_SIZE)
    return (out_regs, ocpsr, ofpscr, od, bytes(nwin)), None


def work(chunk):
    """chunk: list of (eid, addr, insn, ncases, maxatt). Returns list of per-encoding results."""
    uc = G['uc']; wins = G['wins']
    res = []
    for eid, addr, insn, ncases, maxatt in chunk:
        rng = random.Random((G['seed'] * 1000003) ^ (eid * 7919) ^ addr)
        cases = []
        errs = defaultdict(int)
        att = 0
        while len(cases) < ncases and att < maxatt:
            bias = 0.6 if att < 4 else (-1 if att % 2 else 0.95)
            att += 1
            regs = [rand_reg(rng, bias) for _ in range(13)] + [rng.randrange(PTR_LO, PTR_HI, 8), rand_reg(rng, bias), 0]
            cpsr = rng.getrandbits(5) << 27
            fpscr = rng.getrandbits(4) << 28
            dregs = [rand_dreg(rng) for _ in range(32)]
            wi = rng.randrange(NWIN)
            out, err = run_unicorn(uc, addr, regs, cpsr, fpscr, dregs, wins[wi])
            if out is None:
                errs[err] += 1
                continue
            oregs, ocpsr, ofpscr, od, nwin = out
            s = []
            for d in dregs:
                s += [d & 0xffffffff, d >> 32]
            os_ = []
            for d in od:
                os_ += [d & 0xffffffff, d >> 32]
            inp = CASE_FMT.pack(0, addr, insn, *regs, cpsr, fpscr, *s, wi)
            exp = EXP_FMT.pack(*oregs, ocpsr, ofpscr, *os_, oregs[15])
            chg = diff_bytes(wins[wi], nwin)
            cases.append((inp, exp, chg))
        res.append((eid, cases, dict(errs)))
    return res


# ---------------------------------------------------------------- compare
def isnan32(u):
    return (u & 0x7f800000) == 0x7f800000 and (u & 0x7fffff) != 0


def isnan64(u):
    return (u & 0x7ff0000000000000) == 0x7ff0000000000000 and (u & 0xfffffffffffff) != 0


def compare(inp, exp, chg, act):
    """Return list of (field, expected, actual) differences."""
    e = EXP_FMT.unpack(exp)
    st, a_r, a_cpsr, a_fpscr, a_s, a_pc, ninst, a_chg = act
    diffs = []
    if st == 1:
        diffs.append(('fault', '', 'rust fault'))
    elif st == 2:
        diffs.append(('panic', '', 'rust panic'))
        return diffs
    if ninst != 1:
        diffs.append(('ninsts', 1, ninst))
    e_r = e[0:16]
    for i in range(15):
        if e_r[i] != a_r[i]:
            diffs.append(('r%d' % i if i < 13 else ('sp', 'lr')[i - 13], '%08x' % e_r[i], '%08x' % a_r[i]))
    e_cpsr, e_fpscr = e[16], e[17]
    # ARM-state targets with bit 1 set are UNPREDICTABLE; compare word-aligned.
    pcmask = ~1 if (e_cpsr >> 5) & 1 else ~3
    if (e_r[15] & pcmask) != (a_pc & pcmask):
        diffs.append(('pc', '%08x' % e_r[15], '%08x' % a_pc))
    if (e_cpsr >> 5) & 1 != (a_cpsr >> 5) & 1:
        diffs.append(('T', (e_cpsr >> 5) & 1, (a_cpsr >> 5) & 1))
    if (e_cpsr ^ a_cpsr) & 0xf0000000:
        diffs.append(('nzcv', '%x' % (e_cpsr >> 28), '%x' % (a_cpsr >> 28 & 0xf)))
    if (e_cpsr ^ a_cpsr) & 0x08000000:
        diffs.append(('q', (e_cpsr >> 27) & 1, (a_cpsr >> 27) & 1))
    if (e_fpscr ^ a_fpscr) & 0xf0000000:
        diffs.append(('fpscr.nzcv', '%x' % (e_fpscr >> 28), '%x' % (a_fpscr >> 28)))
    e_s = e[18:82]
    for d in range(32):
        e0, e1, a0, a1 = e_s[2 * d], e_s[2 * d + 1], a_s[2 * d], a_s[2 * d + 1]
        if e0 == a0 and e1 == a1:
            continue
        if isnan64(e0 | e1 << 32) and isnan64(a0 | a1 << 32):
            continue
        for k, (x, y) in enumerate(((e0, a0), (e1, a1))):
            if x != y and not (isnan32(x) and isnan32(y)):
                diffs.append(('s%d' % (2 * d + k), '%08x' % x, '%08x' % y))
    if chg != a_chg:
        ed = dict(chg); ad = dict(a_chg)
        offs = sorted(set(ed) | set(ad))
        bad = [o for o in offs if ed.get(o) != ad.get(o)]
        if bad:
            def fmt(d):
                return ' '.join('%08x:%s' % (WIN_BASE + o, '%02x' % d[o] if o in d else '--') for o in bad[:16])
            diffs.append(('mem', fmt(ed), fmt(ad)))
    return diffs


def read_results(path):
    data = open(path, 'rb').read()
    assert data[:4] == b'MC2R'
    n = struct.unpack_from('<I', data, 4)[0]
    p = 8
    hdr = struct.Struct('<2I16III64III')
    res = {}
    for _ in range(n):
        v = hdr.unpack_from(data, p); p += hdr.size
        nch = struct.unpack_from('<I', data, p)[0]; p += 4
        ch = struct.unpack_from('<%dI' % (2 * nch), data, p); p += 8 * nch
        cid, st = v[0], v[1]
        res[cid] = (st, v[2:18], v[18], v[19], v[20:84], v[84], v[85], [(ch[2 * i], ch[2 * i + 1]) for i in range(nch)])
    return res


# ---------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--lib', default=DEFAULT_LIB)
    ap.add_argument('--cases', type=int, default=3)
    ap.add_argument('--max-attempts', type=int, default=24)
    ap.add_argument('--jobs', type=int, default=max(1, (os.cpu_count() or 2) - 2))
    ap.add_argument('--filter', help='only encodings whose capstone mnemonic matches this regex')
    ap.add_argument('--limit', type=int, default=0, help='only the first N unique encodings')
    ap.add_argument('--seed', type=int, default=1)
    ap.add_argument('--out', default=os.path.join(HERE, 'out'))
    ap.add_argument('--examples', type=int, default=4)
    ap.add_argument('--no-build', action='store_true')
    ap.add_argument('--mutate', action='store_true', help='self-test: flip one operand bit of the encoding given to Rust (most cases must fail)')
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    t0 = time.time()

    if not args.no_build:
        r = subprocess.run(['cargo', 'build', '--profile', 'cputest', '--bin', 'cputest'], cwd=ENGINE)
        if r.returncode:
            sys.exit('cargo build failed')
    rust_bin = os.path.join(ENGINE, 'target', 'cputest', 'cputest')

    segs, tstart, tsize, tdata, maps = load_lib(args.lib)
    uniq, total = enumerate_thumb(tstart, tsize, tdata, maps)
    print('thumb instructions in .text: %d, unique encodings: %d' % (total, len(uniq)))

    md = Cs(CS_ARCH_ARM, CS_MODE_THUMB)
    encs = []  # (addr, bytes4, mnemonic, opstr, count, size)
    skipped = defaultdict(int)
    flt = re.compile(args.filter) if args.filter else None
    for key, (va, cnt) in sorted(uniq.items(), key=lambda kv: kv[1][0]):
        off = va - tstart
        b4 = bytes(tdata[off:off + 4]).ljust(4, b'\0')
        ins = next(md.disasm(b4[:len(key)], va + LIB_BASE, 1), None)
        if ins is None or ins.size != len(key):
            skipped['capstone-undecodable'] += 1
            continue
        mn = ins.mnemonic
        if SKIP_MN.match(mn):
            skipped[mn.split('.')[0]] += 1
            continue
        if flt and not flt.search(mn):
            continue
        encs.append((va + LIB_BASE, b4, mn, ins.op_str, cnt, len(key)))
    if args.limit:
        encs = encs[:args.limit]
    print('encodings to test: %d (skipped: %s)' % (len(encs), dict(skipped)))

    # ---- unicorn phase
    tasks = []
    for eid, (addr, b4, mn, ops, cnt, sz) in enumerate(encs):
        insn = struct.unpack('<I', b4)[0]
        tasks.append((eid, addr, insn, args.cases, args.max_attempts))
    chunks = [tasks[i:i + 200] for i in range(0, len(tasks), 200)]
    results = {}
    with mp.Pool(args.jobs, initializer=worker_init, initargs=(args.lib, args.seed)) as pool:
        done = 0
        for part in pool.imap_unordered(work, chunks):
            for eid, cases, errs in part:
                results[eid] = (cases, errs)
            done += len(part)
            if done % 5000 < 200:
                print('  unicorn: %d/%d encodings (%.0fs)' % (done, len(tasks), time.time() - t0), flush=True)
    ncases = sum(len(c) for c, _ in results.values())
    no_case = [eid for eid, (c, _) in results.items() if not c]
    print('unicorn cases: %d; encodings with no valid case: %d (%.0fs)' % (ncases, len(no_case), time.time() - t0))

    # ---- rust phase
    batch = os.path.join(args.out, 'batch.bin')
    rout = os.path.join(args.out, 'results.bin')
    wins = make_windows(args.seed)
    index = []  # case id -> (eid, inp, exp, chg)
    with open(batch, 'wb') as f:
        f.write(b'MC2T' + struct.pack('<I', len(segs)))
        for a, d in segs:
            f.write(struct.pack('<II', a, len(d))); f.write(d)
        f.write(struct.pack('<III', WIN_BASE, WIN_SIZE, NWIN))
        for w in wins:
            f.write(w)
        f.write(struct.pack('<I', ncases))
        for eid in sorted(results):
            for inp, exp, chg in results[eid][0]:
                cid = len(index)
                if args.mutate:
                    sz = encs[eid][5]
                    ins = struct.unpack_from('<I', inp, 8)[0] ^ (1 << (random.Random(cid).randrange(0, 3) if sz == 2 else 16 + random.Random(cid).randrange(8, 11)))
                    inp = inp[:8] + struct.pack('<I', ins) + inp[12:]
                f.write(struct.pack('<I', cid) + inp[4:])
                index.append((eid, inp, exp, chg))
    r = subprocess.run([rust_bin, batch, rout])
    if r.returncode:
        sys.exit('cputest exited with %d' % r.returncode)
    act = read_results(rout)
    print('rust done (%.0fs)' % (time.time() - t0))

    # ---- compare
    enc_fail = {}   # eid -> list of (case, diffs)
    ncase_fail = 0
    for cid, (eid, inp, exp, chg) in enumerate(index):
        d = compare(inp, exp, chg, act[cid])
        if d:
            ncase_fail += 1
            enc_fail.setdefault(eid, []).append((inp, d))
    groups = defaultdict(list)
    for eid, fails in enc_fail.items():
        groups[encs[eid][2]].append(eid)
    tested_by_mn = defaultdict(int)
    for eid, (c, _) in results.items():
        if c:
            tested_by_mn[encs[eid][2]] += 1

    lines = []
    ntested = sum(1 for c, _ in results.values() if c)
    lines.append('encodings tested: %d (of %d selected; %d had no valid unicorn case)' % (ntested, len(encs), len(no_case)))
    lines.append('cases: %d, failing cases: %d, case pass rate: %.3f%%' % (ncases, ncase_fail, 100.0 * (ncases - ncase_fail) / max(1, ncases)))
    lines.append('failing encodings: %d, encoding pass rate: %.3f%%' % (len(enc_fail), 100.0 * (ntested - len(enc_fail)) / max(1, ntested)))
    lines.append('')
    report = []
    for mn, eids in sorted(groups.items(), key=lambda kv: -len(kv[1])):
        fields = defaultdict(int)
        for eid in eids:
            for _, d in enc_fail[eid]:
                for fld in set(x[0] for x in d):
                    fields[fld] += 1
        lines.append('== %s: %d/%d encodings fail; fields: %s' % (mn, len(eids), tested_by_mn[mn], dict(fields)))
        grp = {'mnemonic': mn, 'failing': len(eids), 'tested': tested_by_mn[mn], 'encodings': []}
        for eid in eids[:args.examples]:
            addr, b4, _, ops, cnt, sz = encs[eid]
            inp, d = enc_fail[eid][0]
            lines.append('  %08x  %-8s %s %s   (x%d in binary, %d/%d cases fail)' % (addr, b4[:sz].hex(), mn, ops, cnt, len(enc_fail[eid]), len(results[eid][0])))
            v = CASE_FMT.unpack(inp)
            lines.append('     in: ' + ' '.join('r%d=%08x' % (i, v[3 + i]) for i in range(15)) + ' cpsr=%08x fpscr=%08x' % (v[19], v[20]))
            for fld, e, a in d[:8]:
                lines.append('     %-10s exp=%s  act=%s' % (fld, e, a))
        for eid in eids:
            addr, b4, _, ops, cnt, sz = encs[eid]
            grp['encodings'].append({'addr': '%08x' % addr, 'bytes': b4[:sz].hex(), 'asm': mn + ' ' + ops, 'count': cnt,
                                     'cases': [{'in': list(CASE_FMT.unpack(inp)[3:21]), 'diffs': d} for inp, d in enc_fail[eid]]})
        report.append(grp)
    # unicorn-rejected encodings (never produced a valid case)
    errsum = defaultdict(list)
    for eid in no_case:
        e = results[eid][1]
        k = max(e, key=e.get) if e else '?'
        errsum[k].append(eid)
    lines.append('')
    lines.append('encodings with no valid unicorn case (by dominant error):')
    for k, eids in sorted(errsum.items(), key=lambda kv: -len(kv[1])):
        lines.append('  %s: %d  e.g. %s' % (k, len(eids), ', '.join('%08x %s %s' % (encs[i][0], encs[i][2], encs[i][3]) for i in eids[:6])))
    txt = '\n'.join(lines)
    print(txt)
    open(os.path.join(args.out, 'report.txt'), 'w').write(txt + '\n')
    json.dump(report, open(os.path.join(args.out, 'report.json'), 'w'), indent=1)
    print('\nwrote %s/report.txt and report.json (%.0fs)' % (args.out, time.time() - t0))


if __name__ == '__main__':
    main()
