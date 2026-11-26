#!/usr/bin/env python3
"""Native three-tab/two-pane restore oracle; use private Xvfb or headless Sway."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import runpy
import shlex
import socket
import struct
import subprocess
import tempfile
import termios

HERE = Path(__file__).resolve().parent
F = runpy.run_path(str(HERE / "test-two-local-webkit.py"))
SIZES = [(71, 17), (83, 12), (95, 19)]
COLORS = [[196, 36, 55, 255], [30, 81, 198, 255], [34, 177, 76, 255]]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def kernel_size(tty):
    fd = os.open(tty, os.O_RDONLY | os.O_NOCTTY)
    try:
        rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))
        return [cols, rows]
    finally:
        os.close(fd)


def run(args):
    binary = args.binary.absolute()
    require(binary.is_file() and not binary.is_symlink() and os.access(binary, os.X_OK), "regular executable required")
    require(digest(binary) == args.sha256, "unexpected native executable")
    before_sources = {name: digest(HERE.parent / name) for name in
                      ["web/app.js", "web/local-layout.js", "scripts/test-two-local-webkit.py", "scripts/test-layout-webkit.py"]}
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    last_state = None
    with tempfile.TemporaryDirectory(prefix="aether-layout-", ignore_cleanup_errors=True) as temporary, contextlib.ExitStack() as cleanup:
        owned = Path(temporary)
        env = {key: os.environ[key] for key in ["PATH", "DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS"] if key in os.environ}
        env.update(GDK_BACKEND=args.backend, LC_ALL="C.UTF-8", LIBGL_ALWAYS_SOFTWARE="1", TAURI_WEBVIEW_AUTOMATION="true")
        for key in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"]:
            folder = owned / key
            folder.mkdir(mode=0o700)
            env[key] = str(folder)

        def spawn(command, name, environment=env):
            log = cleanup.enter_context((output / name).open("wb"))
            process = subprocess.Popen(command, cwd=owned, env=environment, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(F["stop_driver"], process)
            return process

        if args.backend == "wayland":
            env.pop("DISPLAY", None)
            env.pop("XAUTHORITY", None)
            config = owned / "sway.conf"
            config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n")
            compositor = spawn(["sway", "--unsupported-gpu", "-c", str(config)], "sway.log",
                               dict(env, WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1"))

            def display():
                require(compositor.poll() is None, "private compositor exited")
                return next((p.name for p in Path(env["XDG_RUNTIME_DIR"]).glob("wayland-*") if p.is_socket()), None)

            env["WAYLAND_DISPLAY"] = F["wait_for"](display, "private Wayland display did not listen")
            spawn(["wtype", "-s", "600000", "-k", "F24"], "seat.log")
        else:
            require(env.get("DISPLAY") and env.get("XAUTHORITY"), "run with authenticated private Xvfb")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        web = spawn(["WebKitWebDriver", "--host=127.0.0.1", f"--port={port}"], "webdriver.log")
        driver = F["Driver"](port)

        def ready():
            require(web.poll() is None, "WebKitWebDriver exited")
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        F["wait_for"](ready, "WebDriver did not listen")
        driver.open(binary)
        cleanup.callback(driver.close)

        def state():
            nonlocal last_state
            last_state = driver.execute(F["READ"])
            return last_state

        def wait(predicate, label):
            return F["wait_for"](predicate, label, timeout=20)

        def snapshot():
            driver.click("#copy-visible")
            return state()["dump"]

        def physical_identity(label, index):
            pidfile, sizefile = owned / (label + ".pid"), owned / (label + ".size")
            command = (f"stty -echo; printf '%s' \"$$\" > {shlex.quote(str(pidfile))}; "
                       f"stty size > {shlex.quote(str(sizefile))}; "
                       f"printf '\\033[2J\\033[H\\033[48;2;{COLORS[index][0]};{COLORS[index][1]};{COLORS[index][2]}m%20s\\033[0m\\r\\nLAYOUT_{label}\\r\\n' ''")
            driver.type("#input", command)
            driver.click("#send")
            wait(lambda: pidfile.is_file() and sizefile.is_file() and sizefile.read_text().strip(), "real shell did not report identity/size")
            pid, birth = F["shell_identity"](pidfile)
            tty = str(Path(f"/proc/{pid}/fd/0").readlink())
            require(birth and tty.startswith("/dev/pts/"), "not a real live PTY")
            rows, cols = map(int, sizefile.read_text().split())
            require([cols, rows] == list(SIZES[index]) == kernel_size(tty), "physical size not restored exactly")
            wait(lambda: f"LAYOUT_{label}" in snapshot() and state()["pixel"] == COLORS[index], "native Canvas did not isolate the selected shell")
            wait(lambda: state()["writable"] and not state()["draft"], "input receipt did not settle")
            return {"pid": pid, "birth": birth, "tty": tty, "size": [cols, rows]}

        def resize(index):
            cols, rows = SIZES[index]
            for selector, value in [("#cols", cols), ("#rows", rows)]:
                driver.clear(selector)
                driver.type(selector, str(value))
            driver.click("#resize")
            wait(lambda: state()["cursor"].startswith(f"{cols} × {rows}"), "accepted native geometry not painted")

        try:
            wait(lambda: F["opened_page"](driver), "native page did not load")
            driver.command("POST", "/window/rect", {"width": 1380, "height": 1000})
            before, epochs = [], []
            for index in range(3):
                button = "#open" if index == 0 else "#new-local-tab"
                require(not driver.execute(f"return document.querySelector({json.dumps(button)}).disabled;"), "third tab unavailable")
                driver.click(button)
                wait(lambda: state()["writable"] and len(state()["tabs"]) == index + 1, "new tab did not get its own lease")
                epochs.append(state()["current_tab"])
                resize(index)
                before.append(physical_identity("before-" + str(index), index))
            require(len(set(epochs)) == len({(v["pid"], v["birth"]) for v in before}) == 3, "tabs not independent")
            F["click_tab"](driver, epochs[0])
            driver.click("#split-local")
            wait(lambda: state()["split"] == "left" and state()["peer_geometry"] == "83x12", "two panes plus parked third tab not visible")
            forbidden = owned / "must-not-replay"
            driver.type("#input", "touch " + shlex.quote(str(forbidden)))
            driver.click("#focus-peer-pane")
            wait(lambda: state()["current_tab"] == epochs[1] and state()["peer_geometry"] == "71x17", "focus did not preserve the independent peer size")
            driver.click("#detach")
            wait(lambda: state()["status"].startswith("View detached."), "explicit detach not acknowledged")
            driver.click("#manage-local")
            wait(lambda: len(state()["options"]) == 3, "three retained tabs not discoverable")
            driver.type("#local-shell-selection", "\ue011\ue015")
            wait(lambda: state()["selected"] == epochs[1] and state()["choose"], "view-only selection not authorized")
            driver.click("#attach-selected-local")
            wait(lambda: "view-only" in state()["status"] and state()["current_tab"] == epochs[1], "reader did not attach to same session")
            require(not state()["writable"] and not state()["resize"], "reader gained resize authority")
            for selector, value in [("#cols", "99"), ("#rows", "6")]:
                driver.clear(selector)
                driver.type(selector, value)
            try:
                driver.click("#resize")
            except F["ProtocolError"] as error:
                require(error.error in ["element not interactable", "invalid element state"], "unrelated observer click failure")
            require(kernel_size(before[1]["tty"]) == list(SIZES[1]) and kernel_size(before[0]["tty"]) == list(SIZES[0]), "reader resized its own or observed PTY")
            saved = driver.execute("const modern=JSON.parse(localStorage.getItem('aether.local-shell-layout.v5')); return modern?.storageVersion===1 ? modern.layout : JSON.parse(localStorage.getItem('aether.local-shell-layout'));")
            require(saved["version"] == 5 and len(saved["tabs"]) == 3 and saved["active"] == 1 and saved["panes"] == [0, 1], "wrong persisted layout identity/pair")
            require([tab["geometry"] for tab in saved["tabs"]] == [{"cols": c, "rows": r} for c, r in SIZES], "unsent reader geometry overwrote authoritative sizes")
            require(all(not ({"draft", "command", "session", "owner"} & tab.keys()) for tab in saved["tabs"]), "live authority or input persisted")
            driver.close()
            for identity in before:
                wait(lambda: F["birth"](identity["pid"]) != identity["birth"], "old desktop retained its PTY")
            driver.open(binary)
            wait(lambda: F["opened_page"](driver) and state()["restore_layout"], "saved layout missing after native restart")
            require(not state()["tabs"] and not state()["writable"], "restart silently launched a shell")
            driver.click("#restore-local-layout")
            wait(lambda: state()["status"] == "Opened 3 fresh shells in the saved layout.", "full geometry restore did not complete")
            require(not forbidden.exists() and not state()["draft"], "restore replayed a parked command draft")
            restored_epochs = state()["tabs"]
            require(len(restored_epochs) == 3 and state()["current_tab"] == restored_epochs[1], "active tab not restored")
            wait(lambda: state()["split"] == "right" and state()["peer_geometry"] == "71x17" and state()["cursor"].startswith("83 × 12"), "saved independent split geometry not restored")
            after = []
            for index, epoch in enumerate(restored_epochs):
                F["click_tab"](driver, epoch)
                after.append(physical_identity("after-" + str(index), index))
            require(len({(v["pid"], v["birth"]) for v in before + after}) == 6, "restore reused old PTYs")
            require(all(kernel_size(v["tty"]) == list(SIZES[index]) for index, v in enumerate(after)), "switching third tab resized another session")
            for epoch in list(reversed(restored_epochs)):
                F["click_tab"](driver, epoch)
                driver.click("#close")
                wait(lambda: state()["status"].startswith("Terminal closed"), "restored tab not closed")
            for identity in after:
                wait(lambda: F["birth"](identity["pid"]) != identity["birth"], "restored child not reaped")
            require(not forbidden.exists(), "a parked draft executed after restore")
            result = {"case_id": "PRODUCT.LAYOUT.001.NATIVE.001", "result": "passed", "backend": args.backend,
                      "binary_sha256": args.sha256, "three_tabs_two_visible_panes": True,
                      "independent_physical_geometries_restored": True, "saved_active_tab_and_pair_restored": True,
                      "observer_resize_refused_and_unsent_size_not_persisted": True,
                      "restore_requires_explicit_action_and_uses_fresh_ptys": True, "all_owned_children_reaped": True,
                      "before": before, "after": after, "saved_layout": saved}
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result), flush=True)
        except BaseException:
            (output / "failure-state.json").write_text(json.dumps(last_state, indent=2) + "\n")
            raise
    require(digest(binary) == args.sha256, "native executable changed")
    require(all(digest(HERE.parent / name) == value for name, value in before_sources.items()), "source changed during native test")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--backend", choices=["x11", "wayland"], default="x11")
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args())
