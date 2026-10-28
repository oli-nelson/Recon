#!/usr/bin/env python3
"""Capture real Recon PTY output as PNGs. See docs/SCREENSHOTS.md.

Requires Unix, Git, pyte and Pillow. No production repository is modified.
"""
import argparse
import codecs
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
COLS, ROWS = 132, 30
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/recon')
parser.add_argument('--font', type=Path, default=Path('/System/Library/Fonts/Menlo.ttc'))
parser.add_argument('--output', type=Path, default=ROOT / 'docs/images')
args = parser.parse_args()
font = ImageFont.truetype(str(args.font), 20)
bold_font = (ImageFont.truetype(str(args.font), 20, index=1)
             if args.font.name == 'Menlo.ttc' else font)
cell_width = round(font.getlength('M'))
cell_height = 28
args.output.mkdir(parents=True, exist_ok=True)

# Recon uses truecolor; named colors cover terminal defaults and ANSI fallbacks.
colors = dict(zip(
    ['black', 'red', 'green', 'brown', 'blue', 'magenta', 'cyan', 'white'],
    ['181b22', 'e06c75', '98c379', 'e5c07b', '61afef', 'c678dd', '56b6c2', 'd8dee9']))


def render(screen, name):
    background, foreground = '#16191f', '#d8dee9'
    pad = 24
    image = Image.new('RGB', (COLS * cell_width + pad * 2,
                             ROWS * cell_height + pad * 2), background)
    draw = ImageDraw.Draw(image)
    def color(value, default):
        return default if value == 'default' else '#' + colors.get(value, value)
    for y in range(ROWS):
        for x in range(COLS):
            char = screen.buffer[y][x]
            fg, bg = color(char.fg, foreground), color(char.bg, background)
            if char.reverse:
                fg, bg = bg, fg
            left, top = pad + x * cell_width, pad + y * cell_height
            if bg != background:
                draw.rectangle((left, top, left + cell_width - 1,
                                top + cell_height - 1), fill=bg)
            draw.text((left, top), char.data,
                      font=bold_font if char.bold and char.data.isascii() else font, fill=fg)
            if char.underscore:
                draw.line((left, top + 25, left + cell_width, top + 25), fill=fg)
    image.save(args.output / f'{name}.png', optimize=True)
    (args.output / f'{name}.txt').write_text('\n'.join(line.rstrip() for line in screen.display) + '\n')
    print(f'Captured {name}', flush=True)


with tempfile.TemporaryDirectory(prefix='recon-screenshots-') as temp:
    base = Path(temp)
    repo = base / 'waypoint'
    repo.mkdir()
    def git(*command):
        return subprocess.run(['git', '-C', str(repo), *command], check=True,
                              capture_output=True, text=True,
                              env={**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull,
                                   'GIT_CONFIG_NOSYSTEM': '1'})
    def write(path, text):
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    git('init', '-q', '-b', 'feature/trail-search')
    write('src/search.py', 'def find_trails(trails, query):\n    return [trail for trail in trails if query in trail.name]\n')
    write('src/settings.py', 'PAGE_SIZE = 10\nSHOW_DISTANCE = False\n')
    write('docs/usage.md', '# Waypoint\n\nFind a trail for your next weekend.\n')
    write('notes.txt', 'Move search ideas into the project docs.\n')
    git('add', '.')
    # Fixture-only identity; no commits are made in the Recon checkout.
    git('-c', 'user.name=Screenshot Fixture', '-c', 'user.email=fixture@example.invalid',
        '-c', 'commit.gpgsign=false', 'commit', '-qm', 'Add the sample trail finder')
    write('src/settings.py', 'PAGE_SIZE = 25\nSHOW_DISTANCE = True\n')
    git('add', 'src/settings.py')
    write('docs/usage.md', '# Waypoint\n\nFind a trail for your next weekend.\nSearch by name, then sort by distance.\n')
    (repo / 'notes.txt').unlink()
    write('tests/test_search.py', 'def test_search_ignores_case():\n    assert search("RIDGE") == search("ridge")\n')
    config = base / 'config.toml'
    config.write_text('theme = "dark"\nsidebar_width = 65\nno_animations = true\n')
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.Stream(screen)
    decoder = codecs.getincrementaldecoder('utf-8')()
    process = subprocess.Popen([str(args.binary.resolve()), str(repo), '--config', str(config)],
                               stdin=slave, stdout=slave, stderr=slave,
                               env={**{k: v for k, v in os.environ.items() if k != 'NO_COLOR'},
                                    'TERM': 'xterm-256color', 'COLORTERM': 'truecolor'})
    os.close(slave)
    def collect(seconds=0.5):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                data = os.read(master, 65536)
                stream.feed(decoder.decode(data))
        if process.poll() is not None:
            raise RuntimeError('Recon exited before capture')
    def keys(data):
        os.write(master, data)
        collect()
    def capture(name, expected):
        display = '\n'.join(screen.display)
        for value in expected:
            assert value in display, f'{name}: missing {value!r}\n{display}'
        render(screen, name)
    try:
        collect(1.5)
        capture('overview', ['RECON', 'HEAD', 'WATCHING', 'settings.py', 'test_search.py'])
        write('src/search.py', 'def find_trails(trails, query):\n    query = query.casefold()\n    return [trail for trail in trails if query in trail.name.casefold()]\n')
        collect()
        write('src/search.py', 'def find_trails(trails, query):\n    query = query.strip().casefold()\n    matches = [trail for trail in trails if query in trail.name.casefold()]\n    return sorted(matches, key=lambda trail: trail.distance)\n')
        collect()
        keys(b'/src/search\r')
        capture('file-history', ['search.py', 'casefold', 'sorted'])
        keys(b'\x1b')
        keys(b'c')
        write('src/settings.py', 'PAGE_SIZE = 50\nSHOW_DISTANCE = True\n')
        collect()
        write('docs/usage.md', '# Waypoint\n\nFind a trail for your next weekend.\nSearch by name, then sort by distance.\nSearch is case-insensitive and ignores extra spaces.\n')
        collect()
        keys(b'z')  # Dismiss the temporary clear notice, restoring normal hints.
        capture('fresh-baseline', ['last clear', 'PAGE_SIZE = 50', 'case-insensitive'])
    finally:
        if process.poll() is None:
            os.write(master, b'q')
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
        os.close(master)
