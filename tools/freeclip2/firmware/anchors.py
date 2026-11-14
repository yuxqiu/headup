#!/usr/bin/env python3
"""Report the firmware facts headup depends on, from tools/freeclip2/firmware/disasm.py output, and check them.

usage: python3 -I tools/freeclip2/firmware/anchors.py OUT_DIR
OUT_DIR holds body.bin, body.asm and ram_*.asm from disasm.py. The load base is body.asm's first address.

Prints a stable text report to diff between firmware versions (skill freeclip2-firmware; baseline
docs/devices/freeclip2/firmware/anchors-<ver>.txt). Addresses move between versions; the values matter. The report
ends with one "check:" line per invariant headup relies on; if any failed, the tool exits non-zero after the report.
"none" in a report line is information (e.g. 43/37 is not sent with an immediate command id), not a failure.
"""
import collections, glob, os, re, struct, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import listing  # noqa: E402

SVC = 43
# command -> (what, has a host handler). 43/89 only goes from the earbuds to the host.
WATCH = {37: ("wear", True), 54: ("device list / audio state", True), 88: ("stream switch", True),
         89: ("sample", False), 90: ("spatial capability", True), 167: ("peer state sync", True),
         180: ("spatial mode", True)}
REPLY_CODES = [0, 1, 2]  # 43/88 TLV 2: 0 applied, 1 earbud-to-earbud link down, 2 refused (FINDINGS "43/88")
INTERNAL = (0x8402, 0x8406, 0x841A, 0x8424)  # handlers that set the stream's start conditions (FINDINGS)
SAMPLE_TASK = 40  # the task that sends 43/89 (FINDINGS "Where the head-tracking computation is")
HOST_TABLE_MIN, INTERNAL_TABLE_MIN = 40, 20  # entries; shorter (id, pointer) runs are not dispatch tables
# Instruction windows, chosen by reading the 526 code: a call's a0/a1 immediates sit within 8 instructions before
# it, a reply code within 3; a descriptor pointer is stored within 5 after its load; an ops call loads its function
# pointer within 3 after loading the ops global; a send to a task is a call within 3 after `li a0,<task>`.
SEND_WINDOW, REPLY_WINDOW, STORE_WINDOW, OPS_WINDOW, TASK_WINDOW = 8, 3, 5, 3, 3
TAIL = 0x40  # bytes at the end of the image holding the pointer table the RAM code reads


def fail(msg):
    sys.exit(f"anchors: {msg}")


def send_sites(rows, fn):
    """{command: [(call address, callee, calling function)]} for calls with a0 = 43 and a1 = an immediate."""
    sites = collections.defaultdict(list)
    for i, r in enumerate(rows):
        callee = r.target() if r.op == "jal" else None
        if callee is None:
            continue
        regs = {}
        for x in rows[max(0, i - SEND_WINDOW):i]:
            for reg in ("a0", "a1"):
                if x.imm(reg) is not None:
                    regs[reg] = x.imm(reg)
        if regs.get("a0") == SVC and "a1" in regs:
            sites[regs["a1"]].append((r.addr, callee, fn[i]))
    return sites


def tables(body, base, lo_id, hi_id, min_len):
    """Runs of (u32 id, u32 code pointer) pairs: command and message dispatch tables."""
    end, found, i = base + len(body), [], 0

    def entry(k):
        if k + 8 > len(body):
            return False
        ident, ptr = struct.unpack_from("<2I", body, k)
        return lo_id <= ident < hi_id and base <= ptr < end

    while i < len(body) - 8:
        if not entry(i):
            i += 4
            continue
        j = i
        while entry(j):
            j += 8
        if (j - i) // 8 >= min_len:
            found.append((base + i, [struct.unpack_from("<2I", body, k) for k in range(i, j, 8)]))
        i = j
    return found


def registered_length(rows, at):
    """The entry count the firmware registers the table at `at` with: a0 = its address, then an immediate a1
    before the call. A dispatch table can run straight into another, so the run of pairs alone does not say."""
    for i, r in enumerate(rows):
        if r.ref == at and r.args.startswith("a0,"):
            for x in rows[i + 1:i + 1 + SEND_WINDOW]:
                if x.imm("a1") is not None:
                    return x.imm("a1")
    return None


def reply_codes(rows, fn, handler, reporters):
    """Immediates loaded into a0 right before each call from `handler` to a 43/88 report function."""
    codes = set()
    for i, r in enumerate(rows):
        if fn[i] != handler or r.op != "jal" or r.target() not in reporters:
            continue
        code = next((x.imm("a0") for x in reversed(rows[max(0, i - REPLY_WINDOW):i]) if x.imm("a0") is not None), None)
        if code is not None:
            codes.add(code)
    return sorted(codes)


def reaches_samples(rows, fn, g):
    """Whether a function that uses global `g` reaches the sample task within two calls or tail jumps."""
    fn_of = {r.addr: f for r, f in zip(rows, fn)}
    calls = collections.defaultdict(set)
    for r, f in zip(rows, fn):
        if r.target() is not None and f is not None:
            calls[f].add(r.target())
    senders = {fn[k] for k, r in enumerate(rows)
               if r.imm("a0") == SAMPLE_TASK and any(x.op == "jal" for x in rows[k + 1:k + 1 + TASK_WINDOW])}

    def reach(f, depth):
        return f in senders or (depth > 0 and any(reach(fn_of.get(c, c), depth - 1) for c in calls.get(f, ())))

    return any(reach(f, 2) for r, f in zip(rows, fn) if r.ref == g and f is not None)


def head_tracking(out_dir, body, base):
    """[(ops global, op calls, module name or None)] for each algorithm module that produces the 43/89 samples: RAM
    code loads its descriptor pointer from the image's tail table and stores it to a global, the sample path calls
    ops +32/+36/+40 through that global, and the descriptor's name sits right before it in initialised data
    (FINDINGS "Where the head-tracking computation is")."""
    regions = sorted((int(os.path.basename(f)[4:12], 16), f) for f in glob.glob(f"{out_dir}/ram_*.asm"))
    if not regions:
        fail(f"no ram_*.asm in {out_dir}")
    rows = [r for _, f in regions for r in listing.parse(open(f).read())]
    fn = listing.function_starts(rows)
    blobs = {a: open(f[:-4] + ".bin", "rb").read() for a, f in regions}
    tail_lo, tail_hi = base + len(body) - TAIL, base + len(body)
    found = []
    for i, r in enumerate(rows):
        if r.op != "lw" or r.ref is None or not tail_lo <= r.ref < tail_hi:
            continue
        reg = r.args.split(",")[0]
        store = next((x for x in rows[i + 1:i + 1 + STORE_WINDOW] if x.op == "sw" and x.args.startswith(reg + ",")),
                     None)
        if store is None or store.ref is None:
            continue
        g = store.ref
        calls = sum(1 for k, x in enumerate(rows) if x.ref == g and x.op == "lw" and any(
            y.op == "lw" and re.fullmatch(r"\w+,(32|36|40)\(\w+\)", y.args) for y in rows[k + 1:k + 1 + OPS_WINDOW]))
        if calls < 2 or not reaches_samples(rows, fn, g):
            continue
        ptr = struct.unpack_from("<I", body, r.ref - base)[0]
        start, data = next(((a, b) for a, b in blobs.items() if a <= ptr < a + len(b)), (ptr, b""))
        name = re.findall(rb"([A-Za-z_][A-Za-z0-9_]{2,31})\x00*$", data[max(0, ptr - start - 32):ptr - start])
        found.append((g, calls, name[0].decode() if name else None))
    return found


def main(out_dir):
    body = open(f"{out_dir}/body.bin", "rb").read()
    rows = listing.parse(open(f"{out_dir}/body.asm").read())
    if not rows:
        fail(f"no instructions in {out_dir}/body.asm")
    base = rows[0].addr
    if rows[-1].addr >= base + len(body):
        fail(f"body.asm ends at {rows[-1].addr:#x}, past body.bin loaded at {base:#x}")
    fn = listing.function_starts(rows)
    sites = send_sites(rows, fn)
    if not sites:
        fail("no call with a0 = 43 and an immediate a1: the MBB send pattern changed")
    checks = {}

    print(f"# load base {base:#x}; size {len(body):#x}")
    sender = collections.Counter(t for v in sites.values() for _, t, _ in v).most_common(1)[0][0]
    print(f"MBB send function (most 43/x call sites): {sender:#x}")
    senders = {}
    for cmd, (what, _) in WATCH.items():
        senders[cmd] = sorted({f for _, _, f in sites.get(cmd, []) if f})
        print(f"43/{cmd} ({what}) sent from functions: {', '.join(f'{f:#x}' for f in senders[cmd]) or 'none'}")
    checks["43/89 has exactly one sender"] = len(senders[89]) == 1

    host = [t for t in tables(body, base, 1, 0x200, HOST_TABLE_MIN) if {88, 90} <= {i for i, _ in t[1]}]
    if len(host) != 1:
        fail(f"expected one host service-43 dispatch table (holding 43/88 and 43/90), found {len(host)}")
    at, entries = host[0]
    registered = registered_length(rows, at)
    if registered is None or registered > len(entries):
        fail(f"host table at {at:#x}: no registration with a length up to {len(entries)} found")
    entries, rest = entries[:registered], entries[registered:]
    handlers = collections.defaultdict(list)
    for i, h in entries:
        handlers[i].append(h)
    print(f"host 43/x dispatch table at {at:#x}: {len(entries)} entries (registered)"
          + (f", followed directly by {len(rest)} entries of another table" if rest else ""))
    for cmd, (what, _) in WATCH.items():
        print(f"  43/{cmd} ({what}) handler: {', '.join(f'{h:#x}' for h in handlers.get(cmd, [])) or 'none'}")
    checks["every watched command but 43/89 has a host handler, 43/89 none"] = all(
        bool(handlers.get(cmd)) == expected for cmd, (_, expected) in WATCH.items())

    reporters = {f for _, _, f in sites.get(88, []) if f}
    if len(handlers[88]) == 1:
        codes = reply_codes(rows, fn, handlers[88][0], reporters)
        print(f"43/88 reply codes (TLV 2) in its handler: {codes}")
    else:
        codes = None
        print(f"43/88 reply codes (TLV 2): not read, 43/88 has {len(handlers[88])} host handlers")
    checks[f"43/88 reply codes are {REPLY_CODES}"] = codes == REPLY_CODES

    modules = head_tracking(out_dir, body, base)
    for g, calls, name in modules:
        print(f"head-tracking algorithm module (ops global {g:08x}, {calls} op calls): '{name or '?'}'")
    if not modules:
        print("head-tracking algorithm module: none")
    checks["exactly one named head-tracking module"] = len(modules) == 1 and modules[0][2] is not None

    internal = [t for t in tables(body, base, 0x8000, 0xA000, INTERNAL_TABLE_MIN)
                if any(i in INTERNAL for i, _ in t[1])]
    handled = set()
    for at, entries in internal:
        known = {i: h for i, h in entries}
        handled |= known.keys() & set(INTERNAL)
        print(f"internal message table at {at:#x}: {len(entries)} entries; "
              + ", ".join(f"{m:#x}->{known[m]:#x}" if m in known else f"{m:#x}->none" for m in INTERNAL))
    if not internal:
        print("internal message table: none")
    checks[f"internal messages {', '.join(f'{m:#x}' for m in INTERNAL)} have handlers"] = handled == set(INTERNAL)

    for what, ok in checks.items():
        print(f"check: {what}: {'ok' if ok else 'FAILED'}")
    failed = [what for what, ok in checks.items() if not ok]
    if failed:
        fail(f"{len(failed)} check(s) failed: " + "; ".join(failed))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
