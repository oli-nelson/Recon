#!/usr/bin/env python3
"""Set the Cargo package and lockfile version without resolving dependencies."""
import re
import sys
from pathlib import Path


def set_version(version, root):
    version = version.removeprefix("v")
    number = r"(0|[1-9][0-9]{0,3})"
    if not re.fullmatch(rf"{number}\.{number}\.{number}(-(alpha|beta|rc)\.(0|[1-9][0-9]*))?", version):
        raise ValueError("use X.Y.Z or X.Y.Z-{alpha,beta,rc}.N")
    outputs = {}
    for filename, header in [("Cargo.toml", "[package]"), ("Cargo.lock", "[[package]]")]:
        path = root / filename
        source = path.read_text()
        sections = re.split(r"(?m)(?=^\[)", source)
        matches = 0
        for index, section in enumerate(sections):
            if section.startswith(header + "\n") and re.search(r'^name = "recon"$', section, re.M):
                sections[index], count = re.subn(r'^version = "[^"]+"$', f'version = "{version}"', section, flags=re.M)
                if count != 1:
                    raise ValueError(f"expected one package version in {filename}")
                matches += 1
        if matches != 1:
            raise ValueError(f"expected one recon package in {filename}")
        outputs[path] = "".join(sections)
    for path, value in outputs.items():
        path.write_text(value)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: set-release-version.py <version>")
    set_version(sys.argv[1], Path(__file__).resolve().parent.parent)
