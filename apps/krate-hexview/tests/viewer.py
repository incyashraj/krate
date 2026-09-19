#!/usr/bin/env python3
"""Read real fixtures through Krate's file API and inspect the viewer's state.
This is not a native file-picker or OS-input test, and not a benchmark.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument('--krate', type=Path, required=True)
p.add_argument('--bundle', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
a.krate, a.bundle, a.output = (x.resolve() for x in (a.krate, a.bundle, a.output))
a.output.mkdir(parents=True, exist_ok=True)
results = []
with tempfile.TemporaryDirectory(prefix='hexview-files-') as d:
    root = Path(d)
    (root / 'input').mkdir()
    data = bytearray(bytes(range(256)) * 256)
    data[32766:32772] = b'needle'
    (root / 'input' / 'boundary.bin').write_bytes(data)
    (root / 'input' / 'empty.bin').write_bytes(b'')
    # Sparse fixture: exercises a large file offset without a giant allocation.
    size = 1024 * 1024 * 1024 + 7
    with (root / 'input' / 'large.bin').open('wb') as f:
        f.truncate(size)
        f.seek(size - 7)
        f.write(b'ENDMARK')
    cases = [
        ('first-page', 'boundary.bin', [], len(data), 0, 0, bytes(data[:320])),
        ('search-crosses-buffer', 'boundary.bin', ['--find', 'needle'], len(data), 32752, 32766, bytes(data[32752:33072])),
        ('hex-search', 'boundary.bin', ['--find', 'hex: 6e 65 65 64 6c 65'], len(data), 32752, 32766, bytes(data[32752:33072])),
        ('large-file-end', 'large.bin', ['--offset', str(size-7)], size, size-7, size-7, b'ENDMARK'),
        ('empty-file', 'empty.bin', [], 0, 0, 0, b''),
        ('compact-window', 'boundary.bin', ['--size', '880x608'], len(data), 0, 0, bytes(data[:224])),
        ('large-window', 'boundary.bin', ['--size', '1280x900'], len(data), 0, 0, bytes(data[:320])),
    ]
    for name, file, flags, size, offset, selected, page in cases:
        cmd = [str(a.krate), 'run', '--headless', '--profile', 'hexview-files',
               '--sandbox-root', str(root), '--grant', 'io.args', '--grant', 'io.stdout',
               '--grant', 'io.stderr', '--grant', 'ui.window:create',
               '--grant', 'fs.read:input/**', '--shoot', str(a.output / (name+'.png')),
               str(a.bundle), '--', '--file', 'input/'+file, '--snapshot', '--probe'] + flags
        r = subprocess.run(cmd, capture_output=True, timeout=30)
        values = dict(x.split('=', 1) for x in r.stdout.decode().strip().split()) if r.returncode == 0 else {}
        okay = (r.returncode == 0 and int(values['size']) == size and int(values['offset']) == offset
                and int(values['selected']) == selected and values['page'] == page.hex())
        results.append({'case': name, 'pass': okay, 'exit': r.returncode, 'state': values, 'stderr': r.stderr.decode()})
        print(('PASS ' if okay else 'FAIL ') + name)
    cmd = [str(a.krate), 'run', '--headless', '--profile', 'hexview-selftest',
           '--grant', 'io.args', '--grant', 'io.stdout', '--grant', 'io.stderr',
           '--grant', 'ui.window:create', str(a.bundle), '--', '--self-test']
    r = subprocess.run(cmd, capture_output=True, timeout=30)
    okay = r.returncode == 0 and b'hexview-ui:self-test:pass' in r.stdout
    results.append({'case': 'handler-self-test', 'pass': okay, 'exit': r.returncode, 'stderr': r.stderr.decode()})
    print(('PASS ' if okay else 'FAIL ') + 'handler-self-test')
report = {'passed': sum(r['pass'] for r in results), 'total': len(results), 'results': results,
          'scope': 'guest handlers, headless rendering and real file handles; OS event injection/file picker not verified'}
(a.output / 'viewer.json').write_text(json.dumps(report, indent=2) + '\n')
raise SystemExit(0 if report['passed'] == report['total'] else 1)
