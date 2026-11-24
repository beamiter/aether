#!/usr/bin/env python3
"""Reject split core revisions or machine-local dependency paths before CI builds."""
import argparse
from pathlib import Path
import json
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
URL = "https://github.com/beamiter/jterm_core"


def read(path):
    with (ROOT / path).open("rb") as stream:
        return tomllib.load(stream)


def require(condition, message):
    if not condition:
        raise SystemExit(message)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--local-candidate", action="store_true",
                    help="check both adapters against the sibling shared core checkout")
args = parser.parse_args()
dependencies = [read(f"{crate}/Cargo.toml")["dependencies"]["jterm_engine"]
                for crate in ("session", "bridge")]
runtime = read("session/Cargo.toml")["target"]['cfg(target_os = "linux")']["dependencies"]["jterm_runtime"]
protocol = read("bridge/Cargo.toml")["dependencies"]["jterm_protocol"]
projection = {"features": ["engine-projection"]}
desktop = read("src-tauri/Cargo.toml")["dependencies"]
remote_client = desktop["jterm_remote_client"]
workspace_core = desktop.get("jterm_core")
desktop_engine = desktop.get("jterm_engine")
if args.local_candidate:
    path = "../../jterm_core/crates/jterm_engine"
    require(dependencies == [{"path": path}] * 2, "expected the same local engine in both adapters")
    require((ROOT / "session" / path / "src/terminal.rs").is_file(), "missing sibling shared terminal")
    runtime_path = "../../jterm_core/crates/jterm_runtime"
    require(runtime == {"path": runtime_path, "features": ["linux-pty"]}, "unexpected local PTY runtime")
    require((ROOT / "session" / runtime_path / "src/pty_session.rs").is_file(), "missing sibling PTY runtime")
    protocol_path = "../../jterm_core/crates/jterm_protocol"
    require(protocol == {"path": protocol_path, **projection}, "unexpected local terminal protocol")
    require((ROOT / "bridge" / protocol_path / "src/terminal.rs").is_file(), "missing sibling terminal protocol")
    remote_path = "../../jterm_core/crates/jterm_remote_client"
    require(remote_client == {"path": remote_path}, "unexpected local JWM client")
    require((ROOT / "src-tauri" / remote_path / "src/lib.rs").is_file(), "missing sibling JWM client")
    if workspace_core is not None:
        require(workspace_core == {"path": "../../jterm_core"}, "unexpected local Workspace core")
        require((ROOT / "src-tauri/../../jterm_core/src/agent_workspace.rs").is_file(),
                "missing sibling Workspace core")
    if desktop_engine is not None:
        require(desktop_engine == {"path": path}, "desktop engine path disagrees")
    source = None
else:
    revision = dependencies[0].get("rev", "")
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "release requires a published core commit SHA")
    require(dependencies == [{"git": URL, "rev": revision}] * 2, "session and bridge core pins disagree")
    require(runtime == {"git": URL, "rev": revision, "features": ["linux-pty"]}, "PTY runtime pin disagrees")
    require(protocol == {"git": URL, "rev": revision, **projection}, "terminal protocol pin disagrees")
    require(remote_client == {"git": URL, "rev": revision}, "JWM client pin disagrees")
    if workspace_core is not None:
        require(workspace_core == {"git": URL, "rev": revision}, "Workspace core pin disagrees")
    if desktop_engine is not None:
        require(desktop_engine == {"git": URL, "rev": revision}, "desktop engine pin disagrees")
    source = f"git+{URL}?rev={revision}#{revision}"
for crate in ("session", "bridge", "src-tauri"):
    packages = read(f"{crate}/Cargo.lock")["package"]
    expected = ["jterm_engine", "jterm_runtime"]
    if crate != "session":
        expected.append("jterm_protocol")
    if crate == "src-tauri":
        expected.append("jterm_remote_client")
        if workspace_core is not None:
            expected.append("jterm_core")
    for name in expected:
        matches = [p for p in packages if p["name"] == name]
        require(len(matches) == 1, f"{crate}: expected one {name}")
        require(matches[0].get("source") == source, f"{crate}: unexpected {name} source")
for crate, dependency, target in (
    ("bridge", "jterm-headless-session-spike", "session"),
    ("src-tauri", "tauri-terminal-frame-bridge", "bridge"),
):
    path = read(f"{crate}/Cargo.toml")["dependencies"][dependency]["path"]
    require(not Path(path).is_absolute(), f"{crate}: absolute dependency path")
    require((ROOT / crate / path).resolve() == ROOT / target, f"{crate}: unexpected {dependency} path")
config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
require((ROOT / "src-tauri" / config["build"]["frontendDist"]).resolve() == ROOT / "web",
        "unexpected embedded frontend path")
require((ROOT / "web/index.html").is_file(), "missing embedded frontend")
print(f"Shared core and dependency paths agree: {source or 'local candidate'}")
