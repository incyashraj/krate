#!/usr/bin/env python3
"""Record edge cases and known incompatibilities, not just passing cases."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser()
for key in ('krate', 'hexyl', 'bundle', 'report'):
    p.add_argument('--' + key, type=Path, required=True)
a = p.parse_args()
a.krate, a.hexyl, a.bundle = (x.resolve() for x in (a.krate, a.hexyl, a.bundle))
env = {k: v for k, v in os.environ.items() if not k.startswith('HEXYL_') and k != 'NO_COLOR'}
cases = [
    ('default', [], {}), ('binary-default-width', ['-b2'], {}),
    ('auto-panels', ['--panels=auto'], {}), ('auto-color-pipe', ['--color=auto'], {}),
    ('NO_COLOR-empty', [], {'NO_COLOR': ''}),
    ('NO_COLOR-force', ['--color=force'], {'NO_COLOR': '1'}),
    ('cp437', ['--character-table=codepage-437'], {}),
    ('cp1047', ['--character-table=codepage-1047'], {}),
    ('color-legend', ['--print-color-table'], {}),
    ('hex-block-size', ['--block-size=0x20', '-n16'], {}),
    ('negative-display-offset', ['--display-offset=-1'], {}),
    ('duplicate-length', ['--length=32', '--bytes=10'], {}),
    ('duplicate-color', ['--color=always', '--color=never'], {}),
    ('panels-17', ['--panels=17'], {}),
    ('width-panels-conflict', ['--terminal-width=80', '--panels=2'], {}),
    ('include-endian-conflict', ['-i', '-e'], {}),
    ('block-unit-invalid', ['--block-size=2block'], {}),
    ('help', ['--help'], {}), ('version', ['--version'], {}),
]
cases += [('completion-' + s, ['--completion', s], {})
          for s in ('bash', 'elvish', 'fish', 'powershell', 'zsh')]
cases += [('env-' + k, [], {'HEXYL_COLOR_' + k: '#ff7f99'})
          for k in ('NULL', 'OFFSET', 'ASCII_PRINTABLE', 'ASCII_WHITESPACE', 'ASCII_OTHER', 'NONASCII')]
results = []
with tempfile.TemporaryDirectory(prefix='hexview-audit-') as d:
    root = Path(d)
    (root / 'input').mkdir()
    data = bytes(range(256))
    for name in ('all.bin', 'spaced name.bin', '日本語.bin', '-leading.bin'):
        (root / 'input' / name).write_bytes(data)
    base = [str(a.krate), 'run', '--headless', '--profile', 'hexview-audit',
            '--sandbox-root', str(root), '--grant', 'io.args', '--grant', 'io.stdout',
            '--grant', 'io.stderr', '--grant', 'io.stdin', '--grant', 'fs.read:input/**',
            str(a.bundle), '--', '--dump']
    def compare(name, flags, extra_env, path='input/all.bin'):
        row = {'case': name, 'flags': flags, 'environment': extra_env, 'path': path}
        for binary, command in [('native', [str(a.hexyl)]), ('krate', base)]:
            cmd = command + flags + ([] if path is None else [path])
            r = subprocess.run(cmd, cwd=root, env=dict(env, **extra_env), input=data,
                               capture_output=True, timeout=30)
            row[binary] = {'exit': r.returncode, 'bytes': len(r.stdout),
                           'sha256': hashlib.sha256(r.stdout).hexdigest(),
                           'stderr': r.stderr.decode(errors='replace')[:1200]}
        row['identical_stdout_and_exit'] = (row['native']['exit'] == row['krate']['exit'] and
                                           row['native']['sha256'] == row['krate']['sha256'])
        results.append(row)
        print(name, 'MATCH' if row['identical_stdout_and_exit'] else 'DIFFERENT', flush=True)
    for name, flags, extra in cases:
        compare(name, flags, extra)
    for name in ('spaced name.bin', '日本語.bin', '-leading.bin', 'missing.bin'):
        compare('path/' + name, ['--color=never'], {}, 'input/' + name)
    compare('directory', [], {}, 'input/')
    compare('stdin-implicit', ['--color=never'], {}, None)
    compare('stdin-implicit-include', ['-i'], {}, None)
    # The reader exits after one line, equivalent to piping into head -n1.
    (root / 'input' / 'pipe.bin').write_bytes(bytes(range(256)) * 16384)
    row = {'case': 'broken-pipe', 'commands': {}, 'exits': {}}
    for binary, command in [('native', [str(a.hexyl)]), ('krate', base)]:
        cmd = command + ['--color=never', 'input/pipe.bin']
        proc = subprocess.Popen(cmd, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        proc.stdout.readline()
        proc.stdout.close()
        error = proc.stderr.read()
        row['commands'][binary] = cmd
        row['exits'][binary] = proc.wait(timeout=30)
        row[binary + '_stderr'] = error.decode(errors='replace')[:1200]
    results.append(row)
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps({'scope': 'non-TTY audit; differences are deliberately retained',
                               'bundle_sha256': hashlib.sha256(a.bundle.read_bytes()).hexdigest(),
                               'cases': results}, indent=2) + '\n')
