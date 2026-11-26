#!/usr/bin/env python3
"""Real old/new WebKit storage round trip in one private HOME/data profile.

Old application support remains limited to its own <=2-tab legacy snapshot.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import runpy
import shlex
import socket
import subprocess
import tempfile

L = runpy.run_path(str(Path(__file__).with_name("test-layout-webkit.py")))
F, require, digest = L["F"], L["require"], L["digest"]
LEGACY = "aether.local-shell-layout"
MODERN = "aether.local-shell-layout.v5"


def run(args):
    old, new = args.previous.absolute(), args.candidate.absolute()
    for binary, expected in [(old, args.previous_sha256), (new, args.candidate_sha256)]:
        require(binary.is_file() and not binary.is_symlink() and os.access(binary, os.X_OK), "regular executable required")
        require(digest(binary) == expected, "wrong old/new executable")
    require(args.previous_sha256 != args.candidate_sha256, "two distinct actual builds required")
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    driver = None
    identities = []
    versions = []
    with tempfile.TemporaryDirectory(prefix="aether-layout-rollback-", ignore_cleanup_errors=True) as temporary, contextlib.ExitStack() as cleanup:
        owned = Path(temporary)
        env = {key: os.environ[key] for key in ["PATH", "DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "XAUTHORITY"] if key in os.environ}
        env.update(GDK_BACKEND=args.backend, LC_ALL="C.UTF-8", LIBGL_ALWAYS_SOFTWARE="1", TAURI_WEBVIEW_AUTOMATION="true")
        for key in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"]:
            directory = owned / key
            directory.mkdir(mode=0o700)
            env[key] = str(directory)

        def spawn(command, name, environment=env):
            log = cleanup.enter_context((output / name).open("wb"))
            child = subprocess.Popen(command, cwd=owned, env=environment, stdin=subprocess.DEVNULL,
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(F["stop_driver"], child)
            return child

        if args.backend == "wayland":
            env.pop("DISPLAY", None)
            env.pop("XAUTHORITY", None)
            config = owned / "sway.conf"
            config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n")
            sway = spawn(["sway", "--unsupported-gpu", "-c", str(config)], "sway.log",
                         dict(env, WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1"))

            def display():
                require(sway.poll() is None, "private compositor exited")
                return next((p.name for p in Path(env["XDG_RUNTIME_DIR"]).glob("wayland-*") if p.is_socket()), None)

            env["WAYLAND_DISPLAY"] = F["wait_for"](display, "private Wayland display did not start")
            spawn(["wtype", "-s", "600000", "-k", "F24"], "seat.log")
        else:
            require(env.get("DISPLAY") and env.get("XAUTHORITY"), "run with private authenticated Xvfb")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        web = spawn(["WebKitWebDriver", "--host=127.0.0.1", f"--port={port}"], "webdriver.log")
        driver = F["Driver"](port)

        def wait(predicate, label):
            return F["wait_for"](predicate, label, timeout=20)

        def ready():
            require(web.poll() is None, "WebKitWebDriver exited")
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        wait(ready, "WebDriver did not listen")
        cleanup.callback(driver.close)

        def state():
            return driver.execute(F["READ"])

        def stored():
            return driver.execute(f"return {{legacy:localStorage.getItem({json.dumps(LEGACY)}),modern:localStorage.getItem({json.dumps(MODERN)})}};")

        def open_app(binary, label):
            driver.open(binary)
            wait(lambda: F["opened_page"](driver), "native old/new page did not load")
            driver.command("POST", "/window/rect", {"width": 1380, "height": 1000})
            versions.append({"label": label, "binary": str(binary), "storage": stored()})

        def close_app():
            driver.close()
            for pid, birth in identities:
                wait(lambda: F["birth"](pid) != birth, "version transition retained an owned PTY")
            identities.clear()

        def identify(label):
            path = owned / (label + ".pid")
            driver.type("#input", f"printf '%s' \"$$\" > {shlex.quote(str(path))}")
            driver.click("#send")
            wait(lambda: path.is_file() and path.read_text().isdigit(), "real restored shell did not answer")
            identity = F["shell_identity"](path)
            require(identity[1], "missing real process identity")
            identities.append(identity)
            wait(lambda: state()["writable"] and not state()["draft"], "write receipt not settled")

        def restore(count, label):
            wait(lambda: state()["restore_layout"], "expected explicit restore is unavailable")
            require(not state()["tabs"] and not state()["writable"], "version transition launched shells automatically")
            driver.click("#restore-local-layout")
            phrase = "Opened one fresh shell" if count == 1 else "Opened two fresh shells" if count == 2 else f"Opened {count} fresh shells"
            wait(lambda: state()["status"].startswith(phrase) and len(state()["tabs"]) == count, "wrong version/layout restore")
            epochs = list(state()["tabs"])
            for index, epoch in enumerate(epochs):
                F["click_tab"](driver, epoch)
                identify(label + str(index))
            return epochs

        def close_tabs():
            for epoch in reversed(list(state()["tabs"])):
                F["click_tab"](driver, epoch)
                driver.click("#close")
                wait(lambda: state()["status"].startswith("Terminal closed"), "user clear did not close a tab")
            for pid, birth in identities:
                wait(lambda: F["birth"](pid) != birth, "clear did not reap its PTY")
            identities.clear()

        try:
            # Original supported two-tab state from the ACTUAL older executable.
            open_app(old, "old-initial")
            for count in [1, 2]:
                driver.click("#open" if count == 1 else "#new-local-tab")
                wait(lambda: state()["writable"] and len(state()["tabs"]) == count, "old two-tab setup failed")
                identify("old-initial-" + str(count))
            driver.click("#split-local")
            wait(lambda: bool(state()["split"]), "old split setup failed")
            legacy = stored()["legacy"]
            require(json.loads(legacy)["version"] in [3, 4] and len(json.loads(legacy)["tabs"]) == 2, "old app did not write its supported snapshot")
            require(driver.execute("return document.querySelector('#new-local-tab').disabled;"), "this oracle assumes genuine old <=2-tab code")
            close_app()

            open_app(new, "new-from-legacy")
            epochs = restore(2, "new-legacy-")
            driver.click("#new-local-tab")
            wait(lambda: len(state()["tabs"]) == 3 and state()["writable"], "modern third tab unavailable")
            identify("new-third")
            epochs = list(state()["tabs"])
            for epoch, (cols, rows) in zip(epochs, [(71, 17), (83, 12), (95, 19)]):
                F["click_tab"](driver, epoch)
                for selector, value in [("#cols", cols), ("#rows", rows)]:
                    driver.clear(selector)
                    driver.type(selector, str(value))
                driver.click("#resize")
                wait(lambda: state()["cursor"].startswith(f"{cols} × {rows}"), "modern geometry did not reach the native frame")
            F["click_tab"](driver, epochs[1])
            modern = stored()["modern"]
            require(stored()["legacy"] == legacy, "modern writes replaced original legacy bytes")
            require(len(json.loads(modern)["layout"]["tabs"]) == 3, "full modern snapshot missing")
            close_app()

            open_app(old, "old-downgrade")
            epochs = restore(2, "old-restored-")
            require(stored()["modern"] == modern, "old restore overwrote modern multi-tab state")
            require(driver.execute("return document.querySelector('#new-local-tab').disabled;"), "no >2-tab backward support is claimed")
            F["click_tab"](driver, epochs[0])
            if state()["split"]:
                driver.click("#split-local")
                wait(lambda: not state()["split"], "older deliberate split change not applied")
            changed = stored()["legacy"]
            require(changed != legacy and stored()["modern"] == modern, "genuine older intent was not independent")
            close_app()

            open_app(new, "new-honors-old-change")
            require(stored()["modern"] == modern, "reading older change destroyed modern backup")
            epochs = restore(2, "new-changed-")
            require(not state()["split"], "new reader ignored older split change")
            require(stored()["legacy"] == changed, "new reader overwrote changed legacy snapshot")
            close_tabs()  # Actual user close-all must suppress every fallback.
            cleared = stored()
            require(cleared["legacy"] is None and json.loads(cleared["modern"])["layout"] is None, "clear left a resurrectable legacy layout")
            close_app()

            for binary, label in [(old, "old-after-clear"), (new, "new-after-clear")]:
                open_app(binary, label)
                require(not state()["restore_layout"] and not state()["tabs"], "cleared tabs resurrected across versions")
                close_app()

            open_app(old, "old-new-intent")
            driver.click("#open")
            wait(lambda: state()["writable"], "new old-app intent did not open a shell")
            identify("old-after-clear-new")
            require(len(json.loads(stored()["legacy"])["tabs"]) == 1, "older new state not saved")
            close_app()
            open_app(new, "new-honors-later-old-save")
            restore(1, "new-final-")
            close_tabs()
            result = {"case_id": "LAYOUT.STORAGE.ROLLBACK.001", "result": "passed", "backend": args.backend,
                      "previous_sha256": args.previous_sha256, "candidate_sha256": args.candidate_sha256,
                      "actual_older_two_tab_snapshot_preserved": True, "modern_three_tab_snapshot_survives_downgrade": True,
                      "older_writer_change_honored_without_implicit_loss": True, "clear_does_not_resurrect_old_tabs": True,
                      "later_legitimate_old_save_after_clear_honored": True, "old_code_never_claimed_more_than_two_tabs": True,
                      "version_transitions": versions}
            (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result), flush=True)
        except BaseException:
            with contextlib.suppress(Exception):
                (output / "failure-state.json").write_text(json.dumps({"view": state(), "storage": stored(), "transitions": versions}, indent=2) + "\n")
            raise
    require(digest(old) == args.previous_sha256 and digest(new) == args.candidate_sha256, "old/new binary changed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--previous-sha256", required=True)
    parser.add_argument("--candidate-sha256", required=True)
    parser.add_argument("--backend", choices=["x11", "wayland"], default="x11")
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args())
