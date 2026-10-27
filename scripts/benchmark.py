#!/usr/bin/env python3
"""Create a disposable large Git fixture and run the release benchmark.
Usage: CARGO_HOME=/tmp/recon-cargo python3 scripts/benchmark.py [--paths 100000]
"""
import argparse
import pathlib
import platform
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--paths', type=int, default=100000)
parser.add_argument('--changed', type=int, default=10000)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
subprocess.run(['cargo', 'build', '--release', '--example', 'benchmark'], cwd=root, check=True)
print(f'platform={platform.platform()} cpu={platform.processor()} paths={args.paths} changed={args.changed}', flush=True)
if platform.system() == 'Darwin':
    data = subprocess.check_output(['/usr/sbin/system_profiler', 'SPHardwareDataType', '-json'])
    import json
    hardware = json.loads(data).get('SPHardwareDataType', [])
    print([{k: v for k, v in x.items() if k in ('chip_type', 'physical_memory', 'machine_model', 'number_processors')} for x in hardware], flush=True)
with tempfile.TemporaryDirectory(prefix='recon-benchmark-') as temp:
    repo = pathlib.Path(temp)
    def git(*argv):
        subprocess.run(['git', '-C', temp, *argv], check=True, stdout=subprocess.DEVNULL)
    git('init', '-q')
    git('config', 'user.name', 'Recon Benchmark Fixture')
    git('config', 'user.email', 'fixture@example.invalid')
    start = time.monotonic()
    for i in range(args.paths):
        folder = repo / f'd{i // 1000:04}'
        folder.mkdir(exist_ok=True)
        (folder / f'f{i:06}.txt').write_text(f'file {i}\n' + 'original content\n' * 6)
    (repo / 'normal.txt').write_text('original text!!\n' * 65536)  # Exactly 1 MiB
    git('add', '.')
    git('commit', '-qm', 'benchmark fixture')
    for i in range(min(args.changed, args.paths)):
        (repo / f'd{i // 1000:04}' / f'f{i:06}.txt').write_text(f'file {i}\nchanged content\n')
    print(f'fixture_seconds={time.monotonic() - start:.3f}', flush=True)
    command = [str(root / 'target/release/examples/benchmark'), temp, str(min(2000,args.paths))]
    subprocess.run(command, check=True)
    subprocess.run([sys.executable, str(root/"scripts/latency_ui.py"), temp], check=True)
