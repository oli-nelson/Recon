#!/usr/bin/env python3
"""Exercise the real terminal backend over a pseudo-terminal, including resize."""
import argparse
import fcntl
import os
import pathlib
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

parser = argparse.ArgumentParser()
parser.add_argument('--large', action='store_true', help='Measure help-key latency during a 10,000-file startup refresh')
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='recon-ui-') as folder:
    subprocess.run(['git', '-C', folder, 'init', '-q'], check=True)
    pathlib.Path(folder, 'hello.txt').write_text('hello\n')
    if args.large:
        for i in range(10000):
            pathlib.Path(folder,f'file{i:05}.txt').write_text('fixture content\n' * 6)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
    process = subprocess.Popen([str(root / 'target/release/recon'), folder, '--no-animations'], stdin=slave, stdout=slave, stderr=slave, env={**os.environ, 'TERM':'xterm-256color'})
    os.close(slave)
    output = bytearray()
    def collect(seconds=0.15):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], max(0,deadline-time.monotonic()))[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                output.extend(data)
    collect(0.05 if args.large else 0.5)
    try:
        startup_deadline = time.monotonic() + 10
        while b'RECON' not in output and time.monotonic() < startup_deadline:
            collect(0.005)
        assert b'RECON' in output, output[-3000:]
        assert b'Opening Git snapshot' not in output, 'startup progress leaked into settled footer'
        if args.large:
            assert b'REFRESHING' in output, 'expected an active startup refresh'
            output.clear()
            start = time.monotonic()
            os.write(master,b'?')
            while b'keyboard' not in output and time.monotonic()-start < 2:
                collect(0.005)
            assert b'keyboard' in output, output[-3000:]
            latency_ms = (time.monotonic()-start)*1000
            print(f'input_during_refresh_ms={latency_ms:.2f}')
            assert latency_ms < 100, 'help key exceeded 100 ms input target'
            os.write(master,b'?')
            collect(0.15)
        else:
            assert b'hello.txt' in output, output[-3000:]
        for key in [b'j', b'\r', b'\x04', b'\x15', b'\t', b'[', b']', b'\x1b[<0;33;6M', b'\x1b[<32;50;6M', b'\x1b[<0;50;6m', b'/hello', b'\x7f', b'o', b'\r', b'n', b'N', b'\x1b', b'd', b'j', b'd', b'\t', b'\x1b[5~', b'\x04', b'\x15', b'f', b'f', b'\r', b'?', b'?', b'\x1b']:
            os.write(master,key)
            collect()
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 12, 36, 0, 0))
        collect()
        os.write(master,b'\t')
        collect()
        os.write(master,b'c')
        collect(0.3)
        pathlib.Path(folder,'hello.txt').write_text('after clear\n')
        collect(0.4)
        os.write(master,b'g')
        collect(0.3)
        os.write(master,b'q')
        collect(0.3)
        assert process.wait(timeout=5) == 0, output[-3000:]
        assert b'\x1b[?2026h' in output and b'\x1b[?2026l' in output, 'synchronized frames missing'
        assert b'panic' not in output.lower(), output[-3000:]
        assert b'\x1b[?1049l' in output, 'alternate screen was not restored'
        print('PASS real terminal: search, match navigation, mouse/keyboard divider resize, sidebar, focus, scroll, help, terminal resize, clear, reset, quit, synchronized frames, terminal restoration')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        os.close(master)
