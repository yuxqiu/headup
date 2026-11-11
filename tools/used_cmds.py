import re, sys, pathlib
root = pathlib.Path(sys.argv[1])
const = {}
for m in re.finditer(r'byte (\w+) = (-?\d+);', (root/'com/huawei/audiodevicekit/audiobluetooth/constant/Command.java').read_text()):
    const['Command.'+m.group(1)] = int(m.group(2))
for m in re.finditer(r'byte (\w+) = (-?\d+);', (root/'com/huawei/audiodevicekit/smartrecord/constant/RecordConstant.java').read_text()):
    const['RecordConstant.'+m.group(1)] = int(m.group(2))
used = {}
for p in root.rglob('*.java'):
    s = p.read_text(errors='ignore')
    for m in re.finditer(r'\{(-?\d+|[\w.]+), (-?\d+|[\w.]+)[,}]', s):
        a, b = m.group(1), m.group(2)
        a = int(a) if re.fullmatch(r'-?\d+', a) else const.get(a)
        b = int(b) if re.fullmatch(r'-?\d+', b) else const.get(b)
        if a == 43 and b is not None:
            used.setdefault(b & 0xff, set()).add(str(p.relative_to(root)))
for k in sorted(used): print(k, sorted(used[k])[:3])
