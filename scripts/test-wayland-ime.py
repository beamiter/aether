#!/usr/bin/env python3
"""Verify native Wayland composition reaches the Aether-owned PTY exactly once."""
import argparse
from collections import Counter
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


def native_surface_presentations(trace):
    request = re.compile(r"wp_presentation[@#]\d+\.feedback\(wl_surface[@#](\d+),\s*"
                         r"new id wp_presentation_feedback[@#](\d+)\)")
    reply = re.compile(r"wp_presentation_feedback[@#](\d+)\.(presented|discarded)\(")
    pending = {}
    presented = Counter()
    for line in trace.read_text(errors="replace").splitlines():
        if match := request.search(line):
            pending[match[2]] = match[1]
        elif match := reply.search(line):
            surface = pending.pop(match[1], None)
            if surface is not None and match[2] == "presented":
                presented[surface] += 1
    return max(presented.values(), default=0)


def exercise_companion(driver, fixtures, profile, compositor_ipc, compositor_tools):
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    Atspi.init()
    wait_for = fixtures["wait_for"]

    def named(role, name):
        pending = [Atspi.get_desktop(0)]
        while pending:
            node = pending.pop()
            if node is None:
                continue
            actual = node.get_role()
            if actual == role and node.get_name() == name and node.get_state_set().contains(Atspi.StateType.SHOWING):
                return node
            if actual not in (Atspi.Role.DOCUMENT_WEB, Atspi.Role.DOCUMENT_FRAME):
                pending.extend(node.get_child_at_index(i) for i in range(node.get_child_count()))
        return None

    def label(text):
        return named(Atspi.Role.LABEL, text)

    def residence():
        return json.loads((profile / "residence.json").read_text())

    def active_body():
        current = residence()
        if current["phase"] == "active":
            return current["possible_body"]
        return None

    def retired(token):
        current = residence()
        return current["possible_body"] is None and token in current["retired_tokens"]

    def state():
        return driver.execute(fixtures["READ"])

    def current_writable():
        value = state()
        return value["current_tab"] if value["writable"] else None

    def received_status(outcome):
        ledger = json.loads((profile / "dialogue-index.json").read_text())
        return [record for record in ledger["results"] if record["status"] == outcome]

    toggle = wait_for(lambda: named(Atspi.Role.TOGGLE_BUTTON, "Eva"), "Eva's GTK control did not appear")
    process = toggle.get_process_id()
    assert not active_body(), "a window without a local shell acquired Eva"
    driver.click("#open-integrated")
    first = wait_for(current_writable, "the integrated shell did not activate")
    token = wait_for(active_body, "the coordinator did not accept a native Aether body frame")
    assert token["host_id"].startswith("aether-")
    wait_for(lambda: label("Eva ready"), "the native Eva slot did not report its presented body")

    marker = "AETHER_EVA_PRIVATE_COMMAND_MARKER"
    driver.type("#ime-input", f"printf '%s\\n' {marker}\ue007")
    wait_for(lambda: label("Command succeeded"), "Eva missed the first PTY's successful command")
    results = wait_for(lambda: received_status("succeeded"), "the shared coordinator missed the successful result")
    assert len(results) == 1 and results[0]["host_id"] == token["host_id"]

    driver.click("#new-jsh-tab")
    second = wait_for(lambda: (tab if (tab := current_writable()) and tab != first else None), "the peer shell did not activate")
    driver.click("#split-local")
    wait_for(lambda: state()["split"] and label("Eva ready"), "the split peer inherited the first shell's feedback")
    assert not label("Command succeeded")
    driver.type("#ime-input", "false\ue007")
    wait_for(lambda: label("Command failed"), "Eva missed the split peer's failed command")
    failures = wait_for(lambda: received_status("failed"), "the shared coordinator missed the failed result")
    assert len(failures) == 1 and failures[0]["task_id"] != results[0]["task_id"]
    assert failures[0]["host_id"] == token["host_id"]

    fixtures["click_tab"](driver, first)
    wait_for(lambda: label("Eva ready"), "returning to the first tab replayed previous feedback")
    driver.type("#close", "\ue007")
    wait_for(lambda: state()["status"].startswith("Terminal closed"), "the original shell did not close")
    wait_for(lambda: label("Eva follows the focused local shell."), "the closed tab kept its companion feedback")
    fixtures["click_tab"](driver, second)
    wait_for(lambda: label("Eva ready"), "the surviving tab inherited stale feedback")
    live_token = wait_for(active_body, "the surviving tab did not present its Eva body")
    assert live_token["host_id"] == token["host_id"]
    assert not label("Command failed")
    assert marker not in (profile / "dialogue-index.json").read_text()

    swaymsg = shutil.which("swaymsg")
    assert swaymsg
    subprocess.run([swaymsg, "-s", str(compositor_ipc), f"[pid={process}] kill"],
                   env=compositor_tools, check=True, capture_output=True, timeout=5)
    wait_for(lambda: retired(live_token), "closing Aether did not retire its exact presented body")
    wait_for(lambda: not named(Atspi.Role.TOGGLE_BUTTON, "Eva"), "Aether stayed open after withdrawal was accepted")
    driver.session = None
    return {"case_id": "PRODUCT.COMPANION.001.NATIVE.001", "backend": "wayland", "result": "passed",
            "compositor_output": "x11", "presentation_protocol": "wp_presentation",
            "native_body_presented_and_retired": True, "split_feedback_isolated": True,
            "opaque_success_and_failure_recorded": True, "window_closes_after_withdrawal": True}


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
    if args.jsh:
        fixture_bin = root / "bin"
        fixture_bin.mkdir()
        (fixture_bin / "jsh").symlink_to(args.jsh.resolve(strict=True))
        env["PATH"] = str(fixture_bin) + ":" + env["PATH"]
        fixtures["prepare_workflows"](root, env)
    config = root / "sway.conf"
    config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\nfocus_follows_mouse no\n")
    with contextlib.ExitStack() as cleanup:
        runtime = Path(cleanup.enter_context(tempfile.TemporaryDirectory(prefix="aether-ime-", dir="/tmp")))
        env["XDG_RUNTIME_DIR"] = str(runtime)
        compositor_env = dict(env, WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1")
        if args.companion_daemon:
            if not os.environ.get("DISPLAY") or not os.environ.get("XAUTHORITY"):
                raise RuntimeError("run the Eva journey with xvfb-run for the private Sway output")
            # The wlroots headless backend can omit wp_presentation receipts.
            # Only Sway uses X11 output. Aether stays on native Wayland.
            compositor_env.update(WLR_BACKENDS="x11", DISPLAY=os.environ["DISPLAY"],
                                  XAUTHORITY=os.environ["XAUTHORITY"], WAYLAND_DEBUG="server")
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
        if args.companion_daemon:
            profile = root / "eva-profile"
            profile.mkdir(mode=0o700)
            eva_socket = runtime / "eva.sock"
            env.update(EVA_COMPANION_MODE="shared", EVA_COMPANION_SOCKET=str(eva_socket),
                       XDG_SESSION_ID="aether-native-qa", NO_AT_BRIDGE="0")
            env["AT_SPI_BUS_ADDRESS"] = os.environ["AT_SPI_BUS_ADDRESS"] = fixtures["private_accessibility_address"]()
            daemon = args.companion_daemon.resolve(strict=True)
            process = spawn([daemon, profile], "eva.log")

            def eva_ready():
                if process.poll() is not None:
                    raise RuntimeError((root / "eva.log").read_text(errors="replace"))
                return eva_socket.is_socket()

            wait_for(eva_ready, "the private Eva daemon did not listen")
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
        if args.companion_daemon:
            compositor_ipc = wait_for(lambda: next(runtime.glob("sway-ipc.*.sock"), None), "no private compositor IPC")
            result = exercise_companion(driver, fixtures, profile, compositor_ipc, compositor_env)
            presentations = native_surface_presentations(root / "sway.log")
            assert presentations >= 3, "Sway did not present the probe, body, and clearance on the same surface"
            result.update(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                          daemon_sha256=hashlib.sha256(daemon.read_bytes()).hexdigest(),
                          presented_surface_commits=presentations)
            (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result), flush=True)
            return
        driver.click("#open")
        wait_for(lambda: driver.execute(fixtures["READ"])["transport"] == "channel",
                 "the normal local output Channel did not attach")

        def capture(path):
            driver.type("#input", "stty raw -echo; exec cat > " + shlex.quote(str(path)))
            driver.click("#send")
            wait_for(path.exists, "owned PTY did not start the raw capture")

        def focus_ime():
            driver.click("#reveal-cursor")
            fixtures["click_visible_terminal"](driver)
            wait_for(lambda: driver.execute("return document.activeElement.id === 'ime-input';"),
                     "the real IME input did not receive focus")
            # Let the native WebKit view present the textarea's cursor after
            # the focus moves from the command field to the terminal.
            assert driver.command("POST", "/execute/async", {"args": [], "script": """
                const finish = arguments[0];
                requestAnimationFrame(() => requestAnimationFrame(() =>
                    finish(document.activeElement.id === 'ime-input')));
            """}), "IME input lost focus before its caret was rendered"

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

        compositor_socket = wait_for(
            lambda: next((entry for entry in runtime.glob("sway-ipc.*.sock") if entry.is_socket()), None),
            "private compositor control socket did not start")
        swaymsg = Path(args.sway).with_name("swaymsg")

        def control_compositor(*arguments):
            return json.loads(subprocess.check_output([swaymsg, "-s", compositor_socket, *arguments],
                                                      env=compositor_env, timeout=5))

        original_workspace, = (entry["name"] for entry in control_compositor("-t", "get_workspaces")
                               if entry["focused"])
        blurred = root / "blurred-composition"
        blurred.mkdir()
        blurred_ime = spawn([ime_client, blurred], "blurred-ime.log")
        wait_for((blurred / "connected").exists, "window-blur input method did not attach")
        (blurred / "start").touch()
        wait_for(lambda: driver.execute("return document.querySelector('#terminal-preedit').textContent === 'ni';"),
                 "cannot begin composition before losing window focus")
        assert control_compositor("workspace", "ime-window-unfocused")[0]["success"]
        wait_for(lambda: driver.execute("return document.querySelector('#terminal-preedit').hidden;"),
                 "Wayland window focus loss did not clear preedit")
        assert other_receipt.read_bytes() == b"z" and receipt.read_bytes() == expected
        fixtures["stop_driver"](blurred_ime)
        assert control_compositor("workspace", shlex.quote(original_workspace))[0]["success"]
        focus_ime()
        subprocess.run([args.wtype, "w"], env=env, timeout=5, check=True)
        wait_for(lambda: other_receipt.read_bytes() == b"zw", "returning to the window changed PTY input")
        assert receipt.read_bytes() == expected

        with binary.open("rb") as executable:
            digest = hashlib.file_digest(executable, "sha256").hexdigest()
        result = dict(binary_sha256=digest, backend="wayland", transport="channel",
                      preedit_bytes=0, committed_hex=expected.hex(),
                      preedit_geometry=anchor, cursor_rectangle=cursor, pane_switch_cancelled_preedit=True,
                      window_blur_cancelled_preedit=True)
        if args.jsh:
            driver.click("#close")
            wait_for(lambda: driver.execute(fixtures["READ"])["status"].startswith("Terminal closed"),
                     "second raw capture did not close")
            fixtures["click_tab"](driver, left)
            driver.click("#close")
            wait_for(lambda: fixtures["opened_page"](driver), "first raw capture did not close")
            driver.click("#open-integrated")
            wait_for(lambda: not driver.execute("return document.querySelector('#terminal-chat-toggle').disabled;"),
                     "the native jsh view did not attach")
            driver.click("#terminal-chat-toggle")
            wait_for(lambda: driver.execute("""
                return document.querySelector('#active-pane').dataset.chatSurface === 'prompt' &&
                       !document.querySelector('#terminal-chat-send').disabled;
            """), "Chat did not dock the integrated shell prompt")
            focus_ime()
            driver.type("#ime-input", "printf '%s' A")
            chat_control = root / "chat-composition"
            chat_control.mkdir()
            chat_ime = spawn([ime_client, chat_control], "chat-ime.log")
            wait_for((chat_control / "connected").exists, "Chat input method did not connect")
            (chat_control / "start").touch()
            wait_for(lambda: driver.execute("""
                return document.querySelector('#terminal-preedit').textContent === 'ni' &&
                       document.querySelector('#terminal-chat-send').disabled &&
                       !document.querySelector('#command-block-list details');
            """), "Chat's live composer did not display native preedit and suspend Send")
            (chat_control / "commit").touch()
            wait_for((chat_control / "committed").exists, "Chat input method did not send commit")
            wait_for(lambda: driver.execute("""
                return document.querySelector('#terminal-preedit').hidden &&
                       !document.querySelector('#terminal-chat-send').disabled;
            """), "Chat's pending composition did not settle")
            assert chat_ime.wait(timeout=5) == 0
            chat_receipt = root / "chat-pty-receipt"
            driver.type("#ime-input", " >> " + shlex.quote(str(chat_receipt)))
            driver.click("#terminal-chat-send")
            wait_for(lambda: chat_receipt.is_file() and chat_receipt.read_bytes() == b"A" + expected,
                     "Chat did not send the composed shell line exactly once")
            assert wait_for(lambda: driver.execute("""
                return Array.from(document.querySelectorAll('#command-block-list details code'), code => code.textContent)
                    .filter(text => text.startsWith("printf '%s' A"));
            """), "Chat did not show the completed command") == ["printf '%s' A你 >> " + str(chat_receipt)]
            result["terminal_chat_ime_commit_preserved_and_sent_once"] = True
            driver.click("#close")
            wait_for(lambda: fixtures["opened_page"](driver), "the integrated Chat shell did not close")
            result.update(fixtures["exercise_terminal_workflows"](driver, root))
            fixtures["exercise_restart"](driver, root, binary, integrated=True)
            result["native_restart_restores_jsh_terminal_chat_and_workflows"] = True
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ime-client", type=Path, required=True)
    parser.add_argument("--jsh", type=Path, help="installed jsh for Terminal Chat and workflows")
    parser.add_argument("--companion-daemon", type=Path, help="run the private native Eva journey instead of the IME journey")
    parser.add_argument("--sway", default="sway")
    parser.add_argument("--wtype", default="wtype")
    parser.add_argument("--compositor-libraries", type=Path)
    args = parser.parse_args()
    if args.companion_daemon and not args.jsh:
        parser.error("--companion-daemon requires the real --jsh fixture")
    run(args)
