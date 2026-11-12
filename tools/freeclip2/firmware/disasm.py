#!/usr/bin/env python3
"""Disassemble a FreeClip 2 RISC-V firmware image (from tools/freeclip2/firmware/unpack.py) as RV32, at its load address, with the
HiSilicon custom instructions annotated (tools/freeclip2/firmware/xlinx.py).

usage (needs riscv64-none-elf binutils, e.g. `nix shell nixpkgs#pkgsCross.riscv64-embedded.buildPackages.binutils`):
    python3 -I tools/freeclip2/firmware/disasm.py IMAGE.img OUT_DIR [--no-stub]

--no-stub skips the startup-stub analysis (gp, stub copies) for images whose stub has another shape (image 3).

Writes OUT_DIR/body.bin (the code body), OUT_DIR/body.asm (annotated listing), and for each region the startup code
copies to RAM, OUT_DIR/ram_<addr>.asm disassembled at its run address. Prints the load base and the regions.
Checks: the code-info block's SHA-256 matches the body, and the load base maps many string references.
"""
import collections, hashlib, os, re, struct, subprocess, sys, tempfile
from typing import NamedTuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import listing, xlinx  # noqa: E402

KEY_AREA, CODE_INFO, BODY = 0x200, 0x300, 0x500  # offsets in a section image with a 0x200 section header
COPY_PAIR = ("804626db", "c0d5a25b")  # custom post-increment load, store: the word copy loop of the startup code
COPY_SCAN = 0x1000  # bytes from the image start searched for copy loops (526: all three within 0x290)
STUB_ROWS = 200  # rows from the image start holding the startup stub (526: last copy loop at row 72; margin)
BRANCHES = "jal|j|beqz|bnez|beq|bne|blt|bge|bltu|bgeu|bgtz|blez|bltz|bgez|bgt|ble|bgtu|bleu"


class Copy(NamedTuple):
    """A region the startup code copies from flash to RAM."""
    dst: int  # run address
    src: int  # flash address
    size: int


def body_of(img):
    magic = struct.unpack_from("<I", img, CODE_INFO)[0]
    assert magic >> 24 == 0x4B, f"no code-info block at {CODE_INFO:#x} (magic {magic:#x})"
    size = struct.unpack_from("<I", img, CODE_INFO + 0x24)[0]
    digest = img[CODE_INFO + 0x28:CODE_INFO + 0x48]
    body = img[BODY:BODY + size]
    assert hashlib.sha256(body).digest() == digest, "code-info SHA-256 does not match the body"
    return body


def objdump(binary, base):
    """RV32 listing of `binary` loaded at `base`, with the custom instructions decoded (xlinx.py). A raw
    `-b binary` dump would decode RV64, so the image is wrapped in an rv32imfc ELF first."""
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, "wrap.s"), "w") as f:
            f.write(f'.section .text,"ax"\n.incbin "{os.path.abspath(binary)}"\n')
        run = lambda *a: subprocess.run(a, check=True, capture_output=True, text=True, cwd=tmp).stdout
        run("riscv64-none-elf-as", "-march=rv32imfc", "-mabi=ilp32f", "wrap.s", "-o", "wrap.o")
        run("riscv64-none-elf-ld", "-m", "elf32lriscv", f"-Ttext={base:#x}", "-e", f"{base:#x}", "wrap.o",
            "-o", "wrap.elf")
        out = run("riscv64-none-elf-objdump", "-D", "wrap.elf")
    out = re.sub(r" <[^>]*>", "", out)
    out = re.sub(rf"(\t({BRANCHES})\t([a-z0-9]+,)*)([0-9a-f]{{8}})$", r"\g<1>0x\4", out, flags=re.M)
    return xlinx.annotate(out)


def absolute_targets(rows):
    """Addresses built with lui + addi (independent of the load base). Each lui counts once, at its first addi:
    unlike listing.constants, this runs over the whole image, where a register's later uses are unrelated."""
    last, out = {}, []
    for r in rows:
        args = [a.strip() for a in r.args.split(",")]
        if r.op == "lui" and len(args) == 2:
            last[args[0]] = int(args[1], 16) << 12
        elif r.op == "addi" and len(args) == 3 and args[1] in last:
            out.append((last.pop(args[1]) + int(args[2])) & 0xFFFFFFFF)
    return out


def load_base(body, rows):
    """The load address that makes the most lui + addi addresses land on string starts. Sampling every fifth
    string, 16-byte alignment and the top 20 candidates keep it fast; arbitrary choices, 526 wins 1165 to 472."""
    strings = [m.start() for m in re.finditer(rb"[\x20-\x7e]{5,}\x00", body)]
    targets = absolute_targets(rows)
    region = collections.Counter(t >> 24 for t in targets if t >> 24 not in (0, 0xFF)).most_common(1)[0][0]
    near = [t for t in targets if t >> 24 == region]
    votes = collections.Counter(t - s for t in near for s in strings[::5] if (t - s) & 0xF == 0)
    sset = set(strings)
    scored = sorted(((sum(1 for t in near if t - b in sset), b) for b, _ in votes.most_common(20)), reverse=True)
    (best, base), (second, _) = scored[0], scored[1]
    assert best >= 2 * second, f"no clear load base: {scored[:3]}"
    return base, best, second


def copy_regions(rows, base, body):
    """Each startup copy loop: the 7 instructions before the copy pair set a0 = end, a1 = start, a2 = source with
    lui/addi. The first region holds code run from RAM (0x600d0000 in 7.0.0.526: the OS, libc and the head-tracking
    library), the rest are initialised data."""
    out = []
    for i, r in enumerate(rows):
        if r.addr >= base + COPY_SCAN:
            break
        if (r.raw, rows[i + 1].raw) != COPY_PAIR:
            continue
        *_, (_, regs) = listing.constants(rows[max(0, i - 7):i])
        assert {"a0", "a1", "a2"} <= regs.keys(), f"copy loop at {r.addr:#x}: registers {regs}"
        end, start, src = regs["a0"], regs["a1"], regs["a2"]
        assert start <= end and base <= src < base + len(body), f"copy loop at {r.addr:#x}: {regs}"
        out.append(Copy(start, src, end - start))
    return out


def tcm_data(rows, base, body):
    """The startup stub at the image start copies initialised data (including the constants the code reads
    gp-relative) to a RAM it addresses PC-relatively, and sets gp. It runs at another address (P0) than the flash
    copy: each copy's length is P0 + constant, and each source starts where the previous copy ends, which fixes
    P0. Returns (gp, [Copy]) in run addresses; src is a flash address."""
    loops, gp = [], None
    for r, regs in listing.constants(rows[:STUB_ROWS]):
        rel = {k: (v - base) & 0xFFFFFFFF for k, v in regs.items()}  # relative to the stub's own load address
        if r.op == "addi" and r.ref is not None and r.args.startswith("gp,"):
            gp = rel["gp"]
        if r.op in ("beq", "beqz") and r.args.startswith("t2,") and {"t0", "t1", "t2"} <= rel.keys():
            loops.append((rel["t0"], rel["t1"], rel["t2"]))
    assert gp is not None and len(loops) >= 2, f"startup stub not recognised: gp={gp}, loops={loops}"
    # Sources are absolute flash addresses (P0 + c must land in the image); chain: src[k+1] - src[k] >= len[k].
    p0 = ((loops[1][1] - loops[0][1]) - loops[0][2]) & 0xFFFFFF00
    segs = [Copy((p0 + d) & 0xFFFFFFFF, (p0 + s) & 0xFFFFFFFF, (p0 + n) & 0xFFFFFFFF) for d, s, n in loops]
    for c in segs:
        assert base <= c.src and c.src + c.size <= base + len(body) and c.size < 0x100000, f"bad copy {c}"
    assert 0 <= segs[1].src - (segs[0].src + segs[0].size) < 8, f"first copies do not chain: {segs}"
    return (p0 + gp) & 0xFFFFFFFF, segs


def annotate_gp(text, gp, segs, base, body):
    """Append the initial value of every gp-relative access that lands in the stub-copied data."""
    def val(a, flt):
        for c in segs:
            if c.dst <= a < c.dst + c.size - 3:
                w = struct.unpack_from("<I", body, c.src - base + a - c.dst)[0]
                f = struct.unpack_from("<f", body, c.src - base + a - c.dst)[0]
                return f"{f:.6g}" if flt else f"{w:#x}"
        return None

    def fix(m):
        line = m.group(0)
        g = re.search(r"\t(f?l\w|gl\w)\s+\w+,(?:(-?\d+)\(gp\)|gp\[([+-]0x[0-9a-f]+)\])", line)
        if not g:
            return line
        off = int(g.group(2)) if g.group(2) is not None else int(g.group(3), 16)
        line = re.sub(r" # [0-9a-f]+$", "", line)
        v = val((gp + off) & 0xFFFFFFFF, g.group(1).startswith("fl"))
        return f"{line}  ; gp{off:+d} = {gp + off:#x}" + (f" init {v}" if v else "")
    return re.sub(r"^.*\(gp\).*$|^.*gp\[.*$", fix, text, flags=re.M)


def main(img_path, out_dir, no_stub=False):
    os.makedirs(out_dir, exist_ok=True)
    body = body_of(open(img_path, "rb").read())
    bin_path = os.path.join(out_dir, "body.bin")
    open(bin_path, "wb").write(body)
    base, best, second = load_base(body, listing.parse(objdump(bin_path, 0)))
    print(f"load base {base:#x} ({best} string references land on strings; next best base {second})")
    text = objdump(bin_path, base)
    rows = listing.parse(text)
    if no_stub:
        gp, segs = None, []
        print("startup stub skipped (--no-stub): no gp values, no stub-copied regions")
    else:
        gp, segs = tcm_data(rows, base, body)
        print(f"gp {gp:#x}; startup copies " + ", ".join(f"{c.src:#x}->{c.dst:#x} ({c.size:#x})" for c in segs))
        text = annotate_gp(text, gp, segs, base, body)
    open(os.path.join(out_dir, "body.asm"), "w").write(text)
    print(f"wrote {out_dir}/body.asm")
    loops = copy_regions(rows, base, body)
    assert loops or no_stub, "no startup copy loops found (pass --no-stub for an image without them)"
    for c in loops + segs:
        name = f"ram_{c.dst:08x}"
        part = os.path.join(out_dir, name + ".bin")
        open(part, "wb").write(body[c.src - base:c.src - base + c.size])
        text = objdump(part, c.dst)
        open(os.path.join(out_dir, name + ".asm"), "w").write(annotate_gp(text, gp, segs, base, body) if segs else text)
        pushes = text.count("push {ra")
        print(f"copied at boot: {c.dst:#x}..{c.dst + c.size:#x} from flash {c.src:#x} -> {name}.asm "
              f"({pushes} function prologues)")


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "--no-stub"]
    if len(args) != 2:
        sys.exit(__doc__)
    main(args[0], args[1], no_stub="--no-stub" in sys.argv[1:])
