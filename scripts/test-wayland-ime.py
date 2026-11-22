#!/usr/bin/env python3
"""Verify native Wayland composition reaches the Aether-owned PTY exactly once."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import shutil
import socket
import subprocess
import tempfile
import time


def run(args):
    fixtures = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))
    wait_for = fixtures["wait_for"]
    binary = args.binary.resolve(strict=True)
    ime_client = args.ime_client.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    webdriver = shutil.which("WebKitWebDriver")
    if not webdriver:
        raise RuntimeError("WebKitWebDriver is required")
    env = {name: os.environ[name] for name in ("PATH", "DBUS_SESSION_BUS_ADDRESS") if name in os.environ}
    env.update(
        GDK_BACKEND="wayland",
        GTK_IM_MODULE="wayland",
        LC_ALL="C.UTF-8",
        LIBGL_ALWAYS_SOFTWARE="1",
        TAURI_WEBVIEW_AUTOMATION="true",
    )
    for name in ("XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "HOME"):
        directory = root / name
        directory.mkdir(mode=0o700)
        env[name] = str(directory)
    config = root / "sway.conf"
    config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\nfocus_follows_mouse no\n")
    with contextlib.ExitStack() as cleanup:
        runtime = Path(cleanup.enter_context(tempfile.TemporaryDirectory(prefix="aether-ime-", dir="/tmp")))
        env["XDG_RUNTIME_DIR"] = str(runtime)
        compositor_env = dict(env, WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1")
        if args.compositor_libraries:
            compositor_env["LD_LIBRARY_PATH"] = str(args.compositor_libraries.resolve(strict=True))

        def spawn(command, name, environment=env):
            with (root / name).open("wb") as log:
                process = subprocess.Popen(command, cwd=root, env=environment, stdin=subprocess.DEVNULL,
                                           stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(fixtures["stop_driver"], process)
            return process

        sway = spawn([args.sway, "--unsupported-gpu", "-c", config], "sway.log", compositor_env)

        def wayland_display():
            if sway.poll() is not None:
                raise RuntimeError((root / "sway.log").read_text(errors="replace"))
            return next((entry.name for entry in runtime.glob("wayland-*") if entry.is_socket()), None)

        env["WAYLAND_DISPLAY"] = wait_for(wayland_display, "Sway headless output did not start")
        spawn([args.wtype, "-s", "600000", "-k", "F24"], "seat.log")
        control = root / "control"
        control.mkdir()
        ime = spawn([ime_client, control], "ime.log")
        wait_for((control / "connected").exists, "input-method-v2 did not connect")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        driver_process = spawn([webdriver, "--host=127.0.0.1", f"--port={port}"], "webdriver.log",
                               dict(env, WAYLAND_DEBUG="client"))
        driver = fixtures["Driver"](port)

        def driver_ready():
            if driver_process.poll() is not None:
                raise RuntimeError("Wayland WebKitWebDriver exited")
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        wait_for(driver_ready, "Wayland WebKitWebDriver did not listen")
        driver.open(binary)
        cleanup.callback(driver.close)
        wait_for(lambda: fixtures["opened_page"](driver), "embedded Aether page did not load")
        driver.click("#open")
        wait_for(lambda: driver.execute(fixtures["READ"])["transport"] == "channel",
                 "the normal local output Channel did not attach")

        def capture(path):
            driver.type("#input", "stty raw -echo; exec cat > " + shlex.quote(str(path)))
            driver.click("#send")
            wait_for(path.exists, "owned PTY did not start the raw capture")

        def focus_ime():
            driver.click("#reveal-cursor")
            driver.click("main canvas")
            wait_for(lambda: driver.execute("return document.activeElement.id === 'ime-input';"),
                     "the real IME input did not receive focus")

        receipt = root / "pty-receipt"
        capture(receipt)
        focus_ime()

        (control / "start").touch()
        wait_for((control / "preedit").exists, "compositor did not deliver preedit")
        trace_path = root / "webdriver.log"
        preedit_pattern = re.compile(r'zwp_text_input_v3@(\d+)\.preedit_string\("ni", 2, 2\)')
        preedit = wait_for(lambda: preedit_pattern.search(trace_path.read_text(errors="replace")),
                           "the Wayland WebView did not receive preedit")
        anchor = wait_for(lambda: driver.execute("""
            const layer = document.querySelector('#terminal-preedit');
            const rect = layer.getBoundingClientRect();
            return layer.textContent === 'ni' && !layer.hidden &&
              rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight
              ? {position:[rect.left, rect.top], viewport:[innerWidth,innerHeight],
                 input:document.querySelector('#ime-input').getBoundingClientRect().toJSON(),
                 canvas:document.querySelector('main canvas').getBoundingClientRect().toJSON(), scroll:[scrollX,scrollY]} : null;
        """), "preedit did not paint by the visible terminal cursor")
        context = f"zwp_text_input_v3@{preedit[1]}"
        rectangle_pattern = re.compile(re.escape(context) + r"\.set_cursor_rectangle\((-?\d+), (-?\d+), (\d+), (\d+)\)")

        def caret_rectangle():
            matches = rectangle_pattern.findall(trace_path.read_text(errors="replace"), preedit.start())
            if matches:
                x, y, width, height = map(int, matches[-1])
                left, top = anchor["position"]
                if width > 0 and height > 0 and abs(x - left) <= 24 and abs(y - top) <= 6:
                    return [x, y, width, height]
            return None

        cursor = wait_for(caret_rectangle, f"Wayland IME cursor rectangle did not follow the terminal caret: {anchor}")
        assert receipt.read_bytes() == b"", "preedit reached the PTY before commit"
        (control / "commit").touch()
        wait_for((control / "committed").exists, "compositor did not send commit")
        expected = "你".encode()
        wait_for(lambda: receipt.stat().st_size >= len(expected), "committed text did not reach the PTY")
        time.sleep(0.25)
        assert receipt.read_bytes() == expected, receipt.read_bytes().hex()
        wait_for(lambda: driver.execute("return document.querySelector('#terminal-preedit').hidden;"),
                 "committed preedit did not clear")
        trace = trace_path.read_text(errors="replace")
        assert f'{context}.commit_string("你")' in trace
        (control / "finish").touch()
        assert ime.wait(timeout=5) == 0

        left = driver.execute(fixtures["READ"])["current_tab"]
        driver.click("#new-local-tab")

        def right_tab():
            state = driver.execute(fixtures["READ"])
            return state["current_tab"] if state["current_tab"] != left and state["writable"] else None

        right = wait_for(right_tab, "a second real PTY did not activate")
        other_receipt = root / "second-pty-receipt"
        capture(other_receipt)
        fixtures["click_tab"](driver, left)
        focus_ime()
        abandoned = root / "abandoned-composition"
        abandoned.mkdir()
        abandoned_ime = spawn([ime_client, abandoned], "abandoned-ime.log")
        wait_for((abandoned / "connected").exists, "second input-method client did not attach")
        (abandoned / "start").touch()
        wait_for(lambda: driver.execute("return document.querySelector('#terminal-preedit').textContent === 'ni';"),
                 "cannot begin composition before switching tab")
        fixtures["click_tab"](driver, right)
        wait_for(lambda: driver.execute("return document.querySelector('#terminal-preedit').hidden;"),
                 "changing the shell view did not clear its composition")
        assert receipt.read_bytes() == expected and other_receipt.read_bytes() == b""
        fixtures["stop_driver"](abandoned_ime)
        focus_ime()
        subprocess.run([args.wtype, "z"], env=env, timeout=5, check=True)
        wait_for(lambda: other_receipt.read_bytes() == b"z", "fresh input did not reach the selected second PTY")
        assert receipt.read_bytes() == expected

        with binary.open("rb") as executable:
            digest = hashlib.file_digest(executable, "sha256").hexdigest()
        result = dict(binary_sha256=digest, backend="wayland", transport="channel",
                      preedit_bytes=0, committed_hex=expected.hex(),
                      preedit_geometry=anchor, cursor_rectangle=cursor, pane_switch_cancelled_preedit=True)
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ime-client", type=Path, required=True)
    parser.add_argument("--sway", default="sway")
    parser.add_argument("--wtype", default="wtype")
    parser.add_argument("--compositor-libraries", type=Path)
    run(parser.parse_args())
