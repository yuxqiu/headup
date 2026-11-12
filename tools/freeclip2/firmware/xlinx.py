#!/usr/bin/env python3
"""Annotate objdump output of a FreeClip 2 RISC-V image with HiSilicon "xlinx" custom instructions.

usage: python3 -I tools/freeclip2/firmware/xlinx.py < LISTING > ANNOTATED   (disasm.py imports annotate() instead)

Input must be an RV32 disassembly. objdump decodes a raw binary as RV64 (c.jal shows as addiw), so wrap the
image in an ELF assembled for rv32imfc first (disasm.py does). Decoded lines end in "; X". Encodings:
hisi-riscv-qemu docs/reference/xlinx-isa.md (WS63) plus the BS25 differences in FINDINGS "Firmware head-tracking
logic". Lines with "?" are guesses.
"""
import re
import sys

REGS = ["zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5",
        "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6"]
LINE = re.compile(r"^(\s*)([0-9a-f]+):(\t)([0-9a-f ]+?)(\s*\t)(.*)$")


def creg(x):
    """A 3-bit compressed register field (x8..x15)."""
    return REGS[8 + x]


def sx(v, bits):
    """Sign-extend the low `bits` of v."""
    return v - (1 << bits) if v >> (bits - 1) & 1 else v


def bits(v, hi, lo):
    return (v >> lo) & ((1 << (hi - lo + 1)) - 1)


def d16(h):
    """Decode a 16-bit custom instruction, or None.

    The cases overlap and are tried in order: "uxt?" is the push/pop encoding (quadrant 0, funct3 4, bits 12-11 = 3)
    with both the push bit (6) and the ret bit (5) set, so it must be tested before push/pop.
    """
    quadrant, f3 = h & 3, h >> 13
    if quadrant == 0 and f3 == 4 and bits(h, 12, 11) == 3 and bits(h, 6, 5) == 3:
        return f"uxt? {creg(bits(h, 9, 7))}  [sz={bits(h, 4, 2)}]"
    if quadrant == 2 and f3 == 0 and bits(h, 12, 12):  # BS25 zero store in the RV32-invalid c.slli space (shift >= 32)
        rd = bits(h, 11, 7)
        sh = 1 << 5 | bits(h, 6, 2)
        base = creg(rd & 7)
        if (sh >> 4) & 3 == 3:
            return f"sw zero,{(rd >> 3) * 16 + (sh & 7) * 2}({base})"
        return f"sb zero,{(sh & 7) * 2 + ((sh >> 3) & 1)}({base})"
    if quadrant == 1 and f3 == 4 and bits(h, 12, 12) and bits(h, 11, 10) in (0, 1):
        width = "sh" if bits(h, 11, 10) == 0 else "s?"
        return (f"xzero? {width} zero,{bits(h, 7, 7) << 3 | bits(h, 4, 2)}(sp)?  "
                f"[raw b9-8={bits(h, 9, 8)} b6-5={bits(h, 6, 5)}]")
    if quadrant == 0 and f3 == 4 and bits(h, 12, 11) == 3:
        n, push, ret, extra = bits(h, 10, 7), bits(h, 6, 6), bits(h, 5, 5), bits(h, 4, 2)
        regs = "{ra" + (",s0" if n == 1 else ",s0-s%d" % (n - 1) if n > 1 else "") + "}"
        adj = ((n + 1) * 4 + 15) // 16 * 16 + extra * 16
        return ("push" if push else "popret" if ret else "pop") + f" {regs},{'-' if push else ''}{adj}"
    if quadrant in (0, 2) and f3 in (1, 5) and not h & 1:
        half = bits(h, 1, 1)
        imm = bits(h, 11, 11) << 4 | bits(h, 10, 10) << 3 | bits(h, 6, 6) << 2 | bits(h, 5, 5) << 1 | bits(h, 12, 12)
        op = ("lhu" if half else "lbu") if f3 == 1 else ("sh" if half else "sb")
        return f"{op} {creg(bits(h, 4, 2))},{imm}({creg(bits(h, 9, 7))})"
    if quadrant == 0 and f3 == 4 and bits(h, 12, 11) == 2:
        return f"xlinx16? rd'={creg(bits(h, 9, 7))} f={bits(h, 6, 2):05b} b10={bits(h, 10, 10)}"
    return None


def d32(w):
    """Decode a 32-bit custom instruction: (text, branch offset or None), or None."""
    opcode, f3 = w & 0x7F, bits(w, 14, 12)
    rd, rs1, rs2 = REGS[bits(w, 11, 7)], REGS[bits(w, 19, 15)], REGS[bits(w, 24, 20)]
    if opcode == 0x0B and f3 == 6:  # BS25: load/store multiple; bitmap bits 20..31 = x7..x18, bit 8 = store
        regs = [REGS[k - 13] for k in range(20, 32) if (w >> k) & 1]
        return ("stm" if bits(w, 8, 8) else "ldm") + " {" + ",".join(regs) + "},(" + rs1 + ")", None
    if opcode == 0x0B:  # BS25: branch on immediate (WS63 puts this layout at 0x3b)
        name = {0: "beqi", 1: "bnei", 4: "blti", 5: "bgei", 6: "bltui", 7: "bgeui"}.get(f3, "b?i")
        off = bits(w, 7, 7) << 1 | bits(w, 11, 8) << 2 | bits(w, 23, 20) << 6
        return f"{name} {rs1},{sx(w >> 24, 8)},", sx(off, 10)
    if opcode == 0x7B:  # BS25: long gp-relative load/store; f3 bit 2 = store, f3 & 3 = width (0 b, 1 h, 2 w, 3 ?)
        size = "bhw?"[f3 & 3]
        if f3 & 4:
            off = sx((w >> 25) << 10 | bits(w, 11, 7) << 5 | bits(w, 19, 15), 17)
            return f"gs{size} {rs2},gp[{off:+#x}]", None
        return f"gl{size} {rd},gp[{sx(w >> 15, 17):+#x}]", None
    if opcode == 0x2B and f3 == 7:  # BS25: rd = rs1 + rs2 * (2 * imm), imm = bits[31:25]
        return f"muliadd {rd},{rs1},{rs2},{2 * (w >> 25)}", None
    if opcode == 0x2B and f3 <= 4:  # BS25: WS63 "shf" (rd = rs1 OP (rs2 shifted)), at 0x2b instead of 0x1b
        op = ["add", "sub", "or", "xor", "and"][f3]
        shift = ["sll", "srl", "sra", "ror"][w >> 30]
        return f"{op}shf {rd},{rs1},{rs2},{shift},{bits(w, 29, 25)}", None
    return None


def annotate_line(line):
    m = LINE.match(line)
    if not m:
        return line
    addr, raw = int(m.group(2), 16), m.group(4).strip()
    out = None
    if len(raw) == 4:
        out = d16(int(raw, 16))
    elif len(raw) == 8:
        r = d32(int(raw, 16))
        if r:
            text, off = r
            out = text if off is None else f"{text} 0x{addr + off:x}"
    return f"{m.group(1)}{m.group(2)}:\t{raw}\t\t{out}    ; X" if out else line


def annotate(listing):
    """The listing with every decodable custom instruction replaced by its decoding."""
    return "\n".join(annotate_line(line) for line in listing.split("\n"))


if __name__ == "__main__":
    sys.stdout.write(annotate(sys.stdin.read()))
