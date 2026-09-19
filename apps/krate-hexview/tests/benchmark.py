#!/usr/bin/env python3
"""macOS process-level benchmark. Verify identical output before measuring.

No GUI or terminal painting is timed. Both complete outputs go to /dev/null.
Includes process launch, Krate bundle validation, compilation and host setup.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import re
import statistics
import subprocess
import time

p = argparse.ArgumentParser()
for name in ('krate', 'hexyl', 'bundle', 'output'):
    p.add_argument('--' + name, type=Path, required=True)
p.add_argument('--runs', type=int, default=9)
a = p.parse_args()
a.krate, a.hexyl, a.bundle, a.output = (
    x.resolve() for x in (a.krate, a.hexyl, a.bundle, a.output))
assert platform.system() == 'Darwin', 'RSS units and /usr/bin/time format are macOS-specific'
a.output.mkdir(parents=True, exist_ok=True)
root = a.output / 'fixtures'
(root / 'input').mkdir(parents=True, exist_ok=True)
rng = random.Random(20260919)
fixtures = {'tiny': bytes(range(64)), 'random256k': rng.randbytes(256 * 1024),
            'random4m': rng.randbytes(4 * 1024 * 1024), 'zeros16m': bytes(16 * 1024 * 1024),
            'random16m': rng.randbytes(16 * 1024 * 1024),
            'native-executable': a.hexyl.read_bytes()}
for name, data in fixtures.items():
    (root / 'input' / name).write_bytes(data)
env = {k: v for k, v in os.environ.items() if not k.startswith('HEXYL_') and k != 'NO_COLOR'}
base = [str(a.krate), 'run', '--headless', '--profile', 'hexview-benchmark',
        '--sandbox-root', str(root), '--grant', 'io.args', '--grant', 'io.stdout',
        '--grant', 'io.stderr', '--grant', 'fs.read:input/**', str(a.bundle), '--', '--dump']
cases = [('startup-64B', 'tiny', []), ('hex-256KiB', 'random256k', []),
         ('hex-4MiB', 'random4m', []),
         ('ansi-256KiB', 'random256k', ['--color', 'always']),
         ('squeeze-16MiB', 'zeros16m', []),
         ('tail-256B-of-4MiB', 'random4m', ['--skip=-256']),
         ('hex-16MiB', 'random16m', []), ('native-executable', 'native-executable', [])]
def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()
def command(binary, file, flags):
    opts = ['--panels', '2'] + ([] if '--color' in flags else ['--color', 'never']) + flags
    return ([str(a.hexyl)] if binary == 'native' else base) + opts + ['input/' + file]
report = {'scope': 'fresh process, warmed filesystem cache, identical output discarded, no GUI',
          'platform': platform.platform(), 'machine': platform.machine(),
          'cpu': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip(),
          'runs': a.runs, 'warmups_per_case_per_binary': 2, 'cases': [],
          'artifacts': {k: {'path': str(v), 'bytes': v.stat().st_size, 'sha256': digest(v)}
                        for k, v in [('runtime', a.krate), ('hexyl', a.hexyl), ('bundle', a.bundle)]},
          'fixture_hashes': {k: hashlib.sha256(v).hexdigest() for k, v in fixtures.items()},
          'build_profiles': {'hexyl': 'opt-level=3, lto=true, codegen-units=1',
                             'krate_guest': 'opt-level=s, lto=true, codegen-units=1'},
          'runtime_limits': 'default 256 MiB guest linear memory, no fuel limit; host RSS is separate'}
def save():
    (a.output / 'benchmark.json').write_text(json.dumps(report, indent=2) + '\n')
for name, file, flags in cases:
    result = {'case': name, 'input_bytes': len(fixtures[file]), 'commands': {}, 'samples': []}
    hashes = []
    for binary in ('native', 'krate'):
        cmd = command(binary, file, flags)
        result['commands'][binary] = cmd
        out = a.output / (name + '-' + binary + '.stdout')
        with out.open('wb') as f:
            r = subprocess.run(cmd, cwd=root, env=env, stdin=subprocess.DEVNULL,
                               stdout=f, stderr=subprocess.PIPE, timeout=180)
        if r.returncode:
            raise RuntimeError(f'{binary}/{name}: {r.returncode}: {r.stderr.decode(errors="replace")}')
        hashes.append(digest(out))
        result[binary + '_output_bytes'] = out.stat().st_size
        out.unlink()  # generated preflight output only; digest remains in report
    result['output_sha256'] = hashes
    result['output_equal'] = hashes[0] == hashes[1]
    assert result['output_equal'], f'Not equivalent: {name}'
    for _ in range(2):
        for binary in ('native', 'krate'):
            subprocess.run(result['commands'][binary], cwd=root, env=env,
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.PIPE, timeout=180, check=True)
    for iteration in range(a.runs):
        order = ['native', 'krate']
        rng.shuffle(order)
        for binary in order:
            start = time.perf_counter_ns()
            r = subprocess.run(['/usr/bin/time', '-l'] + result['commands'][binary],
                               cwd=root, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=180)
            wall_ms = (time.perf_counter_ns() - start) / 1e6
            metrics = r.stderr.decode(errors='replace')
            assert r.returncode == 0, metrics
            rss = re.search(r'(\d+)\s+maximum resident set size', metrics)
            cpu = re.search(r'([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys', metrics)
            assert rss and cpu, metrics
            result['samples'].append({'binary': binary, 'iteration': iteration, 'wall_ms': wall_ms,
                                      'peak_rss_bytes': int(rss[1]), 'user_seconds': float(cpu[2]),
                                      'system_seconds': float(cpu[3])})
    result['summary'] = {}
    for binary in ('native', 'krate'):
        samples = [x for x in result['samples'] if x['binary'] == binary]
        times = [x['wall_ms'] for x in samples]
        result['summary'][binary] = {'median_ms': statistics.median(times), 'min_ms': min(times),
                                    'max_ms': max(times), 'median_peak_rss_MiB':
                                    statistics.median(x['peak_rss_bytes'] for x in samples) / 1048576}
    report['cases'].append(result)
    save()
    print(name, json.dumps(result['summary']), flush=True)
save()
