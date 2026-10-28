#!/usr/bin/env python3
"""Exercise editor handoff and terminal restoration using a fake editor in a PTY."""
import fcntl
import json
import os
import pathlib
import pty
import select
import shlex
import struct
import subprocess
import sys
import tempfile
import termios
import time

root = pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="recon-editor-") as folder:
    directory = pathlib.Path(folder)
    subprocess.run(["git", "-C", folder, "init", "-q"], check=True)
    target = directory / "target with spaces;$(echo bad).txt"
    target.write_text("before editor\n")
    config = directory / "recon.toml"
    config.write_text("no_animations = true\n")
    record = directory / "editor-result.json"
    helper = directory / "fake editor.py"
    helper.write_text('''import json, os, pathlib, sys, termios
directory = pathlib.Path(__file__).parent
flags = termios.tcgetattr(0)[3]
(directory / "editor-result.json").write_text(json.dumps({
    "args": sys.argv[1:], "cwd": os.getcwd(),
    "canonical": bool(flags & termios.ICANON), "echo": bool(flags & termios.ECHO),
}))
if (directory / "fail-editor").exists():
    sys.exit(17)
pathlib.Path(sys.argv[-1]).write_text("after editor\\n")
''')
    command = shlex.join([sys.executable, str(helper), "--wait"])
    for visual, editor, missing in [
        (command, "recon-missing-editor", False),
        ("", command, False),
        ("recon-missing-editor", command, True),
    ]:
        record.unlink(missing_ok=True)
        (directory / "fail-editor").unlink(missing_ok=True)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 140, 0, 0))
        process = subprocess.Popen(
            [str(root / "target/release/recon"), folder, "--config", str(config)],
            stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, "TERM": "xterm-256color", "VISUAL": visual, "EDITOR": editor},
        )
        os.close(slave)
        output = bytearray()

        def collect(seconds=0.2):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], max(0, end - time.monotonic()))[0]:
                    try:
                        data = os.read(master, 65536)
                    except OSError:
                        break
                    if not data:
                        break
                    output.extend(data)

        try:
            collect(0.5)
            os.write(master, b"/target\re")
            collect(0.5)
            if missing:
                assert b"Cannot start editor" in output, output[-3000:]
                assert not record.exists()
            else:
                assert record.exists(), output[-3000:]
                result = json.loads(record.read_text())
                assert result["args"] == ["--wait", str(target.resolve())], result
                assert pathlib.Path(result["cwd"]) == directory.resolve(), result
                assert result["canonical"] and result["echo"], result
                assert target.read_text() == "after editor\n"
                (directory / "fail-editor").touch()
                output.clear()
                os.write(master, b"e")
                collect(0.5)
                assert b"Editor exited with" in output, output[-3000:]
            assert not termios.tcgetattr(master)[3] & termios.ICANON
            os.write(master, b"q")
            process.wait(timeout=5)
            collect()
            assert process.returncode == 0, output[-3000:]
            assert termios.tcgetattr(master)[3] & termios.ICANON
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
print("PASS editor PTY: VISUAL/EDITOR precedence, literal arguments, cooked handoff, failures, raw-mode resume, clean quit")
