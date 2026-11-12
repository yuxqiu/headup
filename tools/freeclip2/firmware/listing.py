"""Parse the objdump listings that disasm.py writes (body.asm, ram_<addr>.asm) into rows.

A row line is "ADDR:<TAB>RAW<TAB>OP<TAB>ARGS", optionally followed by objdump's resolved address (" # 702a5800") and
by our annotations after ";" (xlinx.py's "X", disasm.py's "gp-1748 = 0x20007574 init 0xdeadbeef"). xlinx.py writes its
decoded instructions as "OP ARGS" with a space; both forms parse the same.
"""
import re
from typing import NamedTuple

ROW = re.compile(r"^\s*([0-9a-f]+):\t([0-9a-f ]+?)\s*\t+(.*)$")
JUMPS = ("jal", "j")


class Row(NamedTuple):
    addr: int
    raw: str  # the instruction bytes as objdump prints them (4 or 8 hex digits)
    op: str
    args: str  # operands, without comments
    ref: int | None  # objdump's resolved address comment ("# 6012de58"), if any
    note: str  # our annotations after ";", or ""

    def target(self):
        """Destination of a direct jal or j, else None."""
        m = re.fullmatch(r"(?:ra,)?0x([0-9a-f]+)", self.args)
        return int(m.group(1), 16) if self.op in JUMPS and m else None

    def imm(self, reg):
        """The immediate of `li reg,N`, else None."""
        m = re.fullmatch(rf"{reg},(-?\d+)", self.args)
        return int(m.group(1)) if self.op in ("li", "c.li") and m else None


def parse(text):
    """The rows of the .text section, in listing order."""
    rows, in_text = [], False
    for line in text.split("\n"):
        if line.startswith("Disassembly of section "):
            in_text = line == "Disassembly of section .text:"
            continue
        m = ROW.match(line)
        if not in_text or not m:
            continue
        rest, _, note = m.group(3).partition(";")
        instr, _, ref = rest.partition(" # ")
        op, args = (instr.split(None, 1) + ["", ""])[:2]
        rows.append(Row(int(m.group(1), 16), m.group(2), op, args.strip(), int(ref, 16) if ref else None, note.strip()))
    return rows


def function_starts(rows):
    """For each row, the address of the function containing it (functions begin with a custom push of ra), or
    None before the first."""
    start, out = None, []
    for r in rows:
        if r.op == "push" and r.args.startswith("{ra"):
            start = r.addr
        out.append(start)
    return out


def constants(rows):
    """Yield (row, regs) with the register constants known after each row: `lui rd,imm` sets imm << 12, an addi that
    objdump resolved (lui + addi or auipc + addi, "# value") sets that value, and `mv rd,rs` copies. Nothing is
    forgotten, so use it on straight-line code such as the startup stub."""
    regs = {}
    for r in rows:
        a = [x.strip() for x in r.args.split(",")]
        if r.op == "lui" and len(a) == 2:
            regs[a[0]] = int(a[1], 16) << 12
        elif r.op == "addi" and r.ref is not None:
            regs[a[0]] = r.ref
        elif r.op == "mv" and len(a) == 2 and a[1] in regs:
            regs[a[0]] = regs[a[1]]
        yield r, regs
