#!/usr/bin/env python3
"""Exercise executable config discovery/creation and CLI precedence in isolation."""
import os
import pathlib
import subprocess
import tempfile
import tomllib

root=pathlib.Path(__file__).resolve().parents[1]
binary=root/'target/release/recon'
with tempfile.TemporaryDirectory(prefix='recon-config-') as folder:
    base=pathlib.Path(folder)
    repo=base/'repo'; repo.mkdir()
    subprocess.run(['git','-C',str(repo),'init','-q'],check=True)
    config_home=base/'config-home'
    env={**os.environ,'XDG_CONFIG_HOME':str(config_home),'APPDATA':str(config_home)}
    def run(*args):
        return subprocess.run([str(binary),str(repo),'--once',*args],env=env,text=True,capture_output=True)
    result=run(); assert result.returncode==0,result.stderr
    config=config_home/'recon/config.toml'
    assert tomllib.loads(config.read_text())['fps']==60
    assert 'branch' in result.stdout and '(unborn)' in result.stdout
    config.write_text('fps = 0\n')
    result=run(); assert result.returncode!=0 and 'fps' in result.stderr and str(config) in result.stderr
    result=run('--fps','120'); assert result.returncode==0,result.stderr
    assert config.read_text()=='fps = 0\n','existing config was overwritten'
    other=base/'custom/settings.toml'
    result=run('--config',str(other)); assert result.returncode==0,result.stderr
    assert other.exists()
    other.write_text('no_animations = true\n')
    result=run('--config',str(other),'--no-animations=false'); assert result.returncode==0,result.stderr
    print('PASS executable config: platform discovery, atomic defaults, partial files, CLI/boolean overrides, custom path, validation, preservation, branch header')
