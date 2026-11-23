#!/usr/bin/env python3
"""Run the native Eva journey against JWM in an authenticated private Xvfb."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import runpy
import shlex
import socket
import subprocess
import tempfile
import time


def run(args):
    fixtures = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))
    companion = runpy.run_path(str(Path(__file__).with_name("test-wayland-ime.py")))["exercise_companion"]
    wait_for = fixtures["wait_for"]
    env = {name: os.environ[name] for name in ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS", "PATH")}
    binary, daemon, jwm, jsh = (path.resolve(strict=True) for path in (args.binary, args.daemon, args.jwm, args.jsh))
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    launch = root / "bin"
    launch.mkdir(mode=0o700)
    (launch / "jsh").symlink_to(jsh)
    profile = root / "profile"
    profile.mkdir(mode=0o700)
    env.update(PATH=f"{launch}:{env['PATH']}", GDK_BACKEND="x11", LC_ALL="C.UTF-8", NO_AT_BRIDGE="0",
               XDG_SESSION_ID="aether-x11-qa", TAURI_WEBVIEW_AUTOMATION="true", LIBGL_ALWAYS_SOFTWARE="1",
               GSETTINGS_BACKEND="memory", GSK_RENDERER="cairo", JWM_COMPOSITOR="1", JWM_COMPOSITOR_API="glx",
               JWM_X11_FRAME_SYNC="0" if args.without_frame_feedback else "1", EVA_COMPANION_MODE="shared")
    for name in ("XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "HOME"):
        directory = root / name
        directory.mkdir(mode=0o700)
        env[name] = str(directory)
    env["AT_SPI_BUS_ADDRESS"] = os.environ["AT_SPI_BUS_ADDRESS"] = fixtures["private_accessibility_address"]()
    if args.peer_forge:
        config = Path(env["XDG_CONFIG_HOME"]) / "forge"
        config.mkdir(mode=0o700)
        (config / "config.toml").write_text('shell = "/bin/sh"\njsh_update_check = "never"\neva_companion_enabled = true\n')

    with contextlib.ExitStack() as cleanup:
        runtime = Path(cleanup.enter_context(tempfile.TemporaryDirectory(prefix="aether-x11-eva-", dir="/tmp")))
        eva_socket = runtime / "eva.sock"
        env.update(XDG_RUNTIME_DIR=str(runtime), EVA_COMPANION_SOCKET=str(eva_socket))

        def spawn(name, *command):
            with (root / (name + ".log")).open("wb") as log:
                child = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
                                         start_new_session=True)
            cleanup.callback(fixtures["stop_driver"], child)
            return child

        def command(*argv):
            return subprocess.check_output(argv, cwd=root, env=env, text=True, timeout=5).strip()

        spawn("eva", daemon, profile)
        wait_for(eva_socket.is_socket, "the private Eva daemon did not listen")
        spawn("jwm", jwm, "--backend", "x11rb")
        ipc = runtime / "jwm-ipc.sock"
        wait_for(ipc.is_socket, "the private JWM did not listen")
        if not args.without_frame_feedback:
            wait_for(lambda: "_NET_WM_FRAME_TIMINGS" in command("xprop", "-root", "_NET_SUPPORTED"),
                     "JWM did not advertise extended frame timing")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        spawn("webdriver", "WebKitWebDriver", "--host=127.0.0.1", f"--port={port}")
        driver = fixtures["Driver"](port)

        def ready():
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        wait_for(ready, "loopback WebKitWebDriver did not listen")
        driver.open(binary)
        cleanup.callback(driver.close)
        wait_for(lambda: fixtures["opened_page"](driver), "embedded Aether page did not load")

        def active_window(process):
            focused = command("xdotool", "getactivewindow")
            assert int(command("xdotool", "getwindowpid", focused)) == process, "Aether must own the focused window"
            return focused

        def close_window(process):
            active_window(process)
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(5)
                peer.connect(str(ipc))
                peer.sendall(b'{"command":"kill_client","args":{}}\n')
                with peer.makefile("rb") as reply:
                    assert json.loads(reply.readline())["success"], "JWM declined the native close"

        def handoff(process, initial):
            aether_window = active_window(process)
            spawn("forge", args.peer_forge.resolve(strict=True), "--no-restore", "--mode", "block")

            def presented(prefix, previous):
                state = json.loads((profile / "residence.json").read_text())
                body = state["possible_body"]
                if (state["phase"] == "active" and body and body["host_id"].startswith(prefix)
                        and previous in state["retired_tokens"]):
                    assert body["companion_id"] == initial["companion_id"]
                    return body
                return None

            forge_token = wait_for(lambda: presented("forge-", initial), "Forge did not receive Eva after Aether retired")
            command("xdotool", "windowactivate", "--sync", aether_window)
            token = wait_for(lambda: presented("aether-", forge_token), "Aether did not receive Eva after Forge retired")
            assert token["host_id"] == initial["host_id"] and token != initial
            return token

        if args.without_frame_feedback:
            assert "_NET_WM_FRAME_TIMINGS" not in command("xprop", "-root", "_NET_SUPPORTED")
            driver.click("#open-integrated")
            wait_for(lambda: driver.execute(fixtures["READ"])["writable"], "the jsh shell did not activate")
            marker = root / "exit-marker"
            driver.type("#ime-input", f"touch {shlex.quote(str(marker))}\ue007")
            wait_for(marker.is_file, "the command did not reach the real PTY")
            until = time.monotonic() + 2
            while time.monotonic() < until:
                residence = json.loads((profile / "residence.json").read_text())
                assert residence["phase"] == "unplaced" and residence["possible_body"] is None
                time.sleep(.08)
            result = {"result": "passed", "admission_blocked_without_presentation": True}
        else:
            result = companion(driver, fixtures, profile, close_window, handoff if args.peer_forge else None)
            result["presentation_protocol"] = "_NET_WM_FRAME_TIMINGS"
            if args.peer_forge:
                result.update(aether_forge_round_trip=True,
                              forge_sha256=hashlib.sha256(args.peer_forge.read_bytes()).hexdigest())
        result.update(backend="x11", compositor="jwm", **{
            name + "_sha256": hashlib.sha256(path.read_bytes()).hexdigest()
            for name, path in (("binary", binary), ("daemon", daemon), ("jwm", jwm))
        })
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    for name in ("daemon", "jwm", "jsh", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    variant = parser.add_mutually_exclusive_group()
    variant.add_argument("--without-frame-feedback", action="store_true")
    variant.add_argument("--peer-forge", type=Path)
    run(parser.parse_args())
