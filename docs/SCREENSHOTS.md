# Documentation screenshots

The README images are captures of the real release binary running in a
132 × 30 pseudo-terminal on macOS. They are not interface mockups. The capture
script creates a disposable `waypoint` Git repository, runs Recon, writes files,
stages a change, sends navigation keys, and renders the resulting terminal cells
to PNG using pyte and Pillow. Plain-text captures accompany each PNG for review.
No files or commits in the Recon checkout are used as demo content.

## Regenerate

Use a Unix/macOS environment with Python 3, Git, and a monospace TrueType font:

```sh
cargo build --release --locked
python3 -m venv /tmp/recon-docs-tools
/tmp/recon-docs-tools/bin/pip install pyte==0.8.2 Pillow==12.0.0
/tmp/recon-docs-tools/bin/python scripts/capture-docs.py
```

The default font is macOS Menlo. On Linux, pass `--font /path/to/monospace.ttf`.
Use `--binary /path/to/recon` or `--output /path/to/images` to override the defaults.
The capture script requires Unix PTY APIs; it does not validate Windows rendering.

## Scenes

| Image | Real actions performed |
| --- | --- |
| [Overview](images/overview.png) | Stage settings, edit documentation, delete a tracked note, add an untracked test, then open Recon against HEAD. |
| [File history](images/file-history.png) | Save two successive search improvements while Recon watches; enter `/src/search` to filter the history. |
| [Fresh baseline](images/fresh-baseline.png) | Clear the filter, press `c`, then change the page size and documentation again. Git status is retained alongside the new baseline. |

Animations are disabled for stable captures. The palette is Recon's default dark
theme on a dark terminal background. Text, colors, borders, and selection come
from the actual terminal stream; the renderer supplies the font and outer padding.
Timestamps and temporary paths naturally differ between runs. The script checks
for expected scene text before saving each image and removes its temporary
repository when it exits. Visually inspect regenerated PNGs before committing.
