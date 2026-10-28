#!/usr/bin/env python3
"""Check real frame submissions, settling, transparency, and terminal cleanup."""
import fcntl
import math
import os
import pathlib
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time

root = pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='recon-motion-') as folder:
    subprocess.run(['git','-C',folder,'init','-q'],check=True)
    pathlib.Path(folder,'animated.txt').write_text('incoming edit\n')
    for fps,mode,theme in [(30,'','dark'),(60,'','dark'),(120,'','dark'),(144,'','light'),(60,'--reduced-motion','dark'),(60,'--no-animations','light')]:
        master,slave=pty.openpty()
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',24,100,0,0))
        args=[str(root/'target/release/recon'),folder,'--fps',str(fps),'--theme',theme,'--animation-ms','800']
        if mode:
            args.append(mode)
        child=subprocess.Popen(args,stdin=slave,stdout=slave,stderr=slave,env={**os.environ,'TERM':'xterm-256color'})
        os.close(slave)
        def collect(seconds):
            result=bytearray()
            deadline=time.monotonic()+seconds
            while time.monotonic()<deadline:
                if select.select([master],[],[],max(0,deadline-time.monotonic()))[0]:
                    try:
                        data=os.read(master,65536)
                    except OSError:
                        break
                    if not data:
                        break
                    result.extend(data)
            return result
        try:
            # Wait for first presentation instead of including process startup in the sample.
            initial=bytearray()
            deadline=time.monotonic()+5
            while b'RECON' not in initial and time.monotonic()<deadline:
                initial.extend(collect(.005))
            assert b'RECON' in initial
            initial.extend(collect(.2))
            window=1.0
            frames=collect(window)
            count=frames.count(b'\x1b[?2026h')
            assert count<=math.ceil(fps*window)+2,(fps,count)
            if mode:
                assert count<=5,(mode,count)
            else:
                assert count>=3,(fps,count)
            # Recovery warnings now animate as three-second notifications.
            # Allow their lifetime to finish before checking settled redraws.
            if b'RECOVERY ONLY' in initial+frames:
                collect(2.2)
            idle=collect(.3)
            assert b'\x1b[?2026h' not in idle,'idle redrawing continued after activity settled'
            all_output=initial+frames+idle
            for sequence in re.findall(rb'\x1b\[([0-9;]*)m',all_output):
                params=[int(v or b'0') for v in sequence.split(b';')]
                index=0
                while index<len(params):
                    code=params[index]
                    assert code!=48,'opaque background emitted'
                    if code in (38,58) and index+1<len(params):
                        index+=5 if params[index+1]==2 else 3
                    else:
                        index+=1
            os.write(master,b'q')
            final=collect(.2)
            assert child.wait(timeout=5)==0
            assert b'\x1b[?1049l' in final,'alternate screen was not restored'
            assert b'\x1b[?1006l' in final,'mouse capture was not disabled'
            print(f'PASS cap={fps} theme={theme} mode={mode or "animated"} active_frames={count} idle_frames=0',flush=True)
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
