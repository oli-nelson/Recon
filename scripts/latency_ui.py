#!/usr/bin/env python3
"""Measure actual PTY output after 1 MiB edits in an existing disposable fixture."""
import fcntl
import os
import pathlib
import pty
import select
import struct
import subprocess
import sys
import termios
import time

root = pathlib.Path(__file__).resolve().parents[1]
repo = pathlib.Path(sys.argv[1])
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 48, 160, 0, 0))
process = subprocess.Popen([str(root/'target/release/recon'),str(repo),'--no-animations','--reconcile-seconds','300'],stdin=slave,stdout=slave,stderr=slave,env={**os.environ,'TERM':'xterm-256color'})
os.close(slave)
output = bytearray()
def receive_until(marker, timeout=30):
    deadline = time.monotonic()+timeout
    while marker not in output:
        remaining = deadline-time.monotonic()
        assert remaining > 0, f'no terminal marker {marker!r}; child={process.poll()}; tail={output[-1000:]!r}'
        if select.select([master],[],[],min(remaining,0.1))[0]:
            data = os.read(master,65536)
            assert data, 'terminal closed'
            output.extend(data)
            assert len(output) < 16*1024*1024, 'unexpected terminal output growth'
def cpu():
    data = subprocess.check_output(['ps','-p',str(process.pid),'-o','time='],text=True).strip()
    return sum(float(v)*60**i for i,v in enumerate(reversed(data.split(':'))))
try:
    receive_until(b'WATCHING',120)
    # WATCHING is emitted when the opening snapshot has completed, after its tree render.
    latency=[]
    for i in range(20):
        output.clear()
        marker = bytes([65+i])*15
        contents = marker+b'\n'+b'original text!!\n'*65535
        start=time.monotonic()
        (repo/'normal.txt').write_bytes(contents)
        receive_until(marker)
        latency.append((time.monotonic()-start)*1000)
    latency.sort()
    print(f'visible_pty_1mib_edit_p95_ms={latency[18]:.2f} max_ms={latency[19]:.2f}',flush=True)
    assert latency[18] < 250, 'visible edit latency exceeds target'
    # Allow the last animation-free update to settle before sampling the full app.
    time.sleep(0.5)
    before=cpu()
    time.sleep(10)
    after=cpu()
    percent=(after-before)/10*100
    print(f'full_app_idle_cpu_percent={percent:.2f} sample_seconds=10',flush=True)
    assert percent < 1, 'idle CPU exceeds target'
    os.write(master,b'q')
    assert process.wait(timeout=5)==0
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
    os.close(master)
