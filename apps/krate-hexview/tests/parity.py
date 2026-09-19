#!/usr/bin/env python3
"""Correctness only: compare a pinned native hexyl with the Krate adaptation.

Generated fixtures and reports live outside the source tree. No timings here.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument('--krate', type=Path, required=True)
p.add_argument('--hexyl', type=Path, required=True)
p.add_argument('--bundle', type=Path, required=True)
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
a.krate, a.hexyl, a.bundle = (x.resolve() for x in (a.krate, a.hexyl, a.bundle))
results = []
env = dict(os.environ)
for k in list(env):
    if k.startswith('HEXYL_') or k == 'NO_COLOR':
        env.pop(k)

def run(cmd, root, data=None):
    return subprocess.run(list(map(str, cmd)), cwd=root, input=data or b'',
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=30, env=env)

with tempfile.TemporaryDirectory(prefix='hexview-parity-') as temp:
    root = Path(temp)
    (root / 'input').mkdir()
    rng = random.Random(20260919)
    fixtures = {
        'empty.bin': b'', 'hello.bin': b'Hello, Krate!\n',
        'all-bytes.bin': bytes(range(256)),
        'squeeze.bin': bytes(1024) + b'A' * 256 + bytes(range(64)) + bytes(160),
        'random.bin': rng.randbytes(4099),
        'boundary.bin': b'x' * 32766 + b'needle' + b'y' * 100,
    }
    for name, data in fixtures.items():
        (root / 'input' / name).write_bytes(data)
    base = [a.krate, 'run', '--headless', '--profile', 'hexview-parity',
            '--sandbox-root', root, '--grant', 'io.args', '--grant', 'io.stdout',
            '--grant', 'io.stderr']

    def compare(name, flags, file='random.bin', stdin=False):
        opts = ([] if any(f == '--color' or f.startswith('--color=') for f in flags) else ['--color', 'never'])
        opts += ([] if any(f.startswith(('--panels', '--terminal-width')) for f in flags) else ['--panels', '2'])
        opts += flags
        path = '-' if stdin else 'input/' + file
        data = fixtures[file] if stdin else None
        native = run([a.hexyl] + opts + [path], root, data)
        guest = run(base + ['--grant', 'fs.read:input/**', '--grant', 'io.stdin',
                           a.bundle, '--', '--dump'] + opts + [path], root, data)
        okay = native.returncode == guest.returncode == 0 and native.stdout == guest.stdout
        result = {'case': name, 'pass': okay, 'native_exit': native.returncode,
                  'krate_exit': guest.returncode, 'native_bytes': len(native.stdout),
                  'krate_bytes': len(guest.stdout),
                  'native_sha256': hashlib.sha256(native.stdout).hexdigest(),
                  'krate_sha256': hashlib.sha256(guest.stdout).hexdigest()}
        if not okay:
            result.update(native_stderr=native.stderr.decode(errors='replace')[:1000],
                          krate_stderr=guest.stderr.decode(errors='replace')[:1000],
                          native_excerpt=repr(native.stdout[:300]),
                          krate_excerpt=repr(guest.stdout[:300]))
        results.append(result)
        print(('PASS ' if okay else 'FAIL ') + name, flush=True)

    for name in fixtures:
        compare('file/' + name, [], name)
    cases = [
        ('partial', ['-n', '19']), ('zero-length', ['-n', '0']),
        ('skip', ['-s', '17', '-n', '83']), ('end-relative', ['--skip=-37']),
        ('display-offset', ['-o', '0x1000', '-n', '32']),
        ('block-range', ['--block-size', '32', '-s', '2block', '-n', '3block']),
        ('units', ['-n', '1KiB']), ('past-eof', ['-s', '9000']),
        ('plain', ['--plain']), ('no-position', ['-P']),
        ('no-chars', ['--no-characters']), ('include', ['-i', '-n', '47']),
        ('squeeze-off', ['-v']),
        ('color', ['--color', 'always', '-n', '256']),
        ('gradient', ['--color', 'always', '--color-scheme', 'gradient', '-n', '256']),
        ('attached-short', ['-n37', '-g2']),
        ('short-cluster', ['-Pv', '-n=37']),
        ('plain-explicit-before', ['--color=always', '--border=ascii', '--plain', '-n32']),
        ('plain-explicit-after', ['--plain', '--border=ascii', '--color=always', '-n32']),
        ('plain-characters', ['--plain', '-C', '-n32']),
        ('characters-override', ['--no-characters', '-C', '-n32']),
        ('skip-positive', ['--skip=+17', '-n32']),
        ('terabytes', ['--skip=1TB']),
        ('tebibytes', ['--skip=1TiB']),
        ('width-80', ['--terminal-width=80', '-n64']),
        ('width-binary', ['--terminal-width=80', '-b2', '-n64']),
        ('width-narrow', ['--terminal-width=20', '-n64']),
        ('width-no-chars', ['--terminal-width=120', '--no-characters', '-n64']),
        ('width-groups', ['--terminal-width=120', '-g4', '-n64']),
        ('include-partial', ['--include', '-n1']),
        ('force-color', ['--color=force', '-n64']),
    ]
    cases += [('border/' + s, ['--border', s, '-n', '51']) for s in ('unicode', 'ascii', 'none')]
    cases += [('base/' + s, ['--base', s, '-n', '51']) for s in ('2', '8', '10', '16')]
    cases += [('panels/' + str(n), ['--panels', str(n), '-n', '101']) for n in (1, 3, 4)]
    cases += [('group/' + str(n) + '/' + e, ['-g', str(n), '--endianness', e, '-n', '101'])
              for n in (1, 2, 4, 8) for e in ('big', 'little')]
    cases += [('characters/' + s, ['--character-table', s, '-n', '256'])
              for s in ('default', 'ascii', 'braille')]
    for name, flags in cases:
        compare(name, flags)
    compare('squeeze/disabled', ['-v'], 'squeeze.bin')
    compare('squeeze/colored', ['--color', 'always'], 'squeeze.bin')
    compare('stdin/basic', [], 'hello.bin', True)
    compare('stdin/skip-length', ['-s', '7', '-n', '19'], 'all-bytes.bin', True)
    compare('stdin/include', ['-i', '-n', '19'], 'all-bytes.bin', True)
    for size in (1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 8191, 8192, 8193):
        fixtures['edge.bin'] = rng.randbytes(size)
        (root / 'input' / 'edge.bin').write_bytes(fixtures['edge.bin'])
        compare('boundary/' + str(size), [], 'edge.bin')
        compare('boundary-grouped/' + str(size), ['-g8', '-e'], 'edge.bin')
    compare('empty/include', ['-i'], 'empty.bin')
    compare('empty/plain', ['-p'], 'empty.bin')
    compare('stdin/skip-past-end', ['-s9000'], 'hello.bin', True)

    for name, args, grant, needle in [
        ('no-file-grant', ['input/hello.bin'], False, 'PermissionDenied'),
        ('outside-grant', ['outside.bin'], True, 'PermissionDenied'),
        ('missing-file', ['input/missing.bin'], True, 'NotFound'),
        ('unsupported-table', ['--character-table', 'cp437', 'input/hello.bin'], True, 'supports'),
        ('invalid-group', ['-g', '3', 'input/hello.bin'], True, 'Group size'),
        ('negative-stdin', ['-s', '-1', '-'], True, 'Cannot seek backward'),
    ]:
        (root / 'outside.bin').write_bytes(b'NOT GRANTED')
        cmd = base + (['--grant', 'fs.read:input/**', '--grant', 'io.stdin'] if grant else [])
        result = run(cmd + [a.bundle, '--', '--dump'] + args, root)
        okay = result.returncode != 0 and needle.lower() in result.stderr.decode().lower() and not result.stdout
        results.append({'case': 'error/' + name, 'pass': okay, 'exit': result.returncode,
                        'stderr': result.stderr.decode(errors='replace')})
        print(('PASS ' if okay else 'FAIL ') + 'error/' + name, flush=True)

report = {'purpose': 'correctness, not a performance benchmark',
          'bundle_sha256': hashlib.sha256(a.bundle.read_bytes()).hexdigest(),
          'runtime_sha256': hashlib.sha256(a.krate.read_bytes()).hexdigest(),
          'native_sha256': hashlib.sha256(a.hexyl.read_bytes()).hexdigest(),
          'passed': sum(r['pass'] for r in results), 'total': len(results), 'results': results}
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(report, indent=2) + '\n')
print(f"{report['passed']}/{report['total']} passed. Report: {a.report}")
raise SystemExit(0 if report['passed'] == report['total'] else 1)
