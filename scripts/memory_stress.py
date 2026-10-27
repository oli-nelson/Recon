#!/usr/bin/env python3
"""Exercise default snapshot/history budgets with 300 changed 1 MiB files."""
import pathlib
import subprocess
import tempfile
root = pathlib.Path(__file__).resolve().parents[1]
subprocess.run(['cargo','build','--release','--example','memory'],cwd=root,check=True)
with tempfile.TemporaryDirectory(prefix='recon-memory-') as folder:
    repo = pathlib.Path(folder)
    def git(*args):
        subprocess.run(['git','-C',folder,*args],check=True,stdout=subprocess.DEVNULL)
    git('init','-q')
    git('config','user.name','Recon Benchmark Fixture')
    git('config','user.email','fixture@example.invalid')
    original = 'a' * (1024 * 1024 - 1) + '\n'
    for i in range(300):
        (repo/f'{i:04}.txt').write_text(original)
    git('add','.')
    git('commit','-qm','memory fixture')
    changed = 'b' * (1024 * 1024 - 1) + '\n'
    for i in range(300):
        (repo/f'{i:04}.txt').write_text(changed)
    subprocess.run([str(root/'target/release/examples/memory'),folder],check=True)
