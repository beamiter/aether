#!/usr/bin/env python3
"""Reject split core revisions or machine-local dependency paths before CI builds."""
from pathlib import Path
import json
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
URL = "https://github.com/beamiter/jterm_core"


def read(path):
    with (ROOT / path).open("rb") as stream:
        return tomllib.load(stream)


revision = read("session/Cargo.toml")["dependencies"]["jterm_engine"]["rev"]
assert re.fullmatch(r"[0-9a-f]{40}", revision), "core must use a full commit SHA"
for crate in ("session", "bridge"):
    dependency = read(f"{crate}/Cargo.toml")["dependencies"]["jterm_engine"]
    assert dependency["git"] == URL and dependency["rev"] == revision, crate
for crate in ("session", "bridge", "src-tauri"):
    engines = [p for p in read(f"{crate}/Cargo.lock")["package"] if p["name"] == "jterm_engine"]
    assert len(engines) == 1, f"{crate}: expected one shared engine"
    assert engines[0]["source"] == f"git+{URL}?rev={revision}#{revision}", crate
for crate, dependency, target in (
    ("bridge", "jterm-headless-session-spike", "session"),
    ("src-tauri", "tauri-terminal-frame-bridge", "bridge"),
):
    path = read(f"{crate}/Cargo.toml")["dependencies"][dependency]["path"]
    assert not Path(path).is_absolute(), f"{crate}: absolute dependency path"
    assert (ROOT / crate / path).resolve() == ROOT / target, crate
config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
assert (ROOT / "src-tauri" / config["build"]["frontendDist"]).resolve() == ROOT / "web"
assert (ROOT / "web/index.html").is_file(), "missing embedded frontend"
print(f"Shared core and local dependency paths agree: {revision}")
