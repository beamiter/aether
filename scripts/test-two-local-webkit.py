#!/usr/bin/env python3
"""Drive the exact Aether desktop binary and two owned local PTYs in WebKitGTK."""
import argparse
import contextlib
import http.client
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from urllib.parse import quote

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"
READ = """
return {
  status: document.querySelector('#status').textContent,
  health: document.querySelector('#health').textContent,
  cursor: document.querySelector('#cursor-status').textContent,
  draft: document.querySelector('#input').value,
  writable: !document.querySelector('#input').disabled &&
            !document.querySelector('#send').disabled,
  resize: !document.querySelector('#resize').disabled,
  chooser: !document.querySelector('#local-shell-options').hidden,
  options: Array.from(document.querySelector('#local-shell-selection').options,
                       option => option.value),
  selected: document.querySelector('#local-shell-selection').value,
  choose: !document.querySelector('#attach-selected-local').disabled,
  second: !document.querySelector('#open-second-local').disabled,
  dump: document.querySelector('#history-output').value,
  pixel: Array.from(document.querySelector('main canvas')
                    .getContext('2d').getImageData(2, 2, 1, 1).data)
};
"""


class ProtocolError(RuntimeError):
    def __init__(self, command, error):
        super().__init__(f"{command}: {error}")
        self.error = error


class Driver:
    def __init__(self, port):
        self.port = port
        self.session = None

    def request(self, method, path, data=None, timeout=7):
        with contextlib.closing(http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)) as connection:
            body = None if data is None else json.dumps(data)
            headers = {} if body is None else {"Content-Type": "application/json"}
            connection.request(method, path, body, headers)
            response = connection.getresponse()
            message = json.loads(response.read(131072))
            value = message["value"]
            if response.status != 200:
                code = value.get("error", "unknown error") if isinstance(value, dict) else "unknown error"
                raise ProtocolError(f"{method} {path}", code)
            return value

    def open(self, binary):
        result = self.request("POST", "/session", {
            "capabilities": {
                "alwaysMatch": {"webkitgtk:browserOptions": {"binary": str(binary)}},
                "firstMatch": [{}],
            },
        }, timeout=25)
        session = result["sessionId"]
        if not isinstance(session, str) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,128}", session):
            raise ValueError("unusable WebKit WebDriver session identifier")
        self.session = session

    def command(self, method, path, data=None):
        assert self.session is not None
        return self.request(method, f"/session/{self.session}{path}", data)

    def execute(self, read_only_script):
        return self.command("POST", "/execute/sync", {"script": read_only_script, "args": []})

    def element_path(self, selector):
        item = self.command("POST", "/element", {"using": "css selector", "value": selector})[ELEMENT]
        return "/element/" + quote(item, safe="")

    def click(self, selector):
        self.command("POST", self.element_path(selector) + "/click", {})

    def type(self, selector, text):
        self.command("POST", self.element_path(selector) + "/value", {"text": text})

    def close(self):
        if self.session is not None:
            self.command("DELETE", "")
            self.session = None


def wait_for(predicate, label, timeout=10):
    stop_at = time.monotonic() + timeout
    while time.monotonic() < stop_at:
        if value := predicate():
            return value
        time.sleep(.08)
    raise AssertionError(label)


def opened_page(driver):
    return driver.execute("""
        return location.protocol === 'tauri:' && location.hostname === 'localhost' && Boolean(globalThis.__TAURI__?.core?.invoke)
            && Boolean(document.querySelector('#open'));
    """)


def shell_identity(sidecar):
    pid = int(sidecar.read_text())
    if pid <= 1:
        raise ValueError("bad owned shell PID")
    return pid, birth(pid)


def birth(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[19]
    except FileNotFoundError:
        return None


def exercise(driver, owned):
    def state():
        return driver.execute(READ)

    def close_without_stale_presentation():
        driver.click("#close")
        wait_for(lambda: state()["status"].startswith("Terminal closed"), "selected shell did not close")
        closed = state()
        assert closed["pixel"] == [0, 0, 0, 0], "a closed shell left pixels in the WebKit canvas"
        assert closed["cursor"] == closed["health"] == closed["dump"] == "", "closed shell status or text remains visible"

    def marker_on_screen(marker, absent, color):
        def sample():
            s = state()
            return s["dump"].splitlines().count(marker) == 1 and absent not in s["dump"] and s["pixel"] == color
        def snapshot():
            driver.click("#copy-visible")
            return sample()
        return wait_for(snapshot, f"native grid/canvas did not isolate {marker}")

    def launch(column, color, marker, pid_path):
        r, g, b, _ = color
        cmd = (f"stty -echo; printf '%s' \"$$\" > {shlex.quote(str(pid_path))}; "
               f"printf '\\033[2J\\033[H\\033[48;2;{r};{g};{b}m%20s\\033[0m\\r\\n%s\\r\\n' '' {marker}")
        assert state()["writable"]
        driver.type("#input", cmd)
        driver.click("#send")
        return wait_for(lambda: pid_path.is_file() and pid_path.stat().st_size > 0, f"owned shell {column} never started")

    def list_shells(n):
        driver.click("#manage-local")
        def roster():
            s = state()
            return s if s["chooser"] and len(s["options"]) == n else None
        return wait_for(roster, f"expected {n} local shell choices")

    def detach():
        driver.click("#detach")
        wait_for(lambda: state()["status"].startswith("View detached."), "view did not detach")

    def select(label):
        assert re.fullmatch(r"[1-9][0-9]{0,9}", label)
        before = state()
        assert label in before["options"]
        if before["selected"] != label:
            # WebDriver focuses the real select and dispatches Home or End.
            # No synthetic DOM assignment or reliance on a native popup rect.
            key = "\ue011" if label == before["options"][0] else "\ue010"
            driver.type("#local-shell-selection", key)
        wait_for(lambda: state()["selected"] == label and state()["choose"], "explicit selection was not accepted")
        driver.click("#attach-selected-local")
        wait_for(lambda: "view-only" in state()["status"], "retained view was not read-only")

    driver.click("#open")
    wait_for(lambda: state()["writable"], "first local shell did not open")
    a, b = "AETHER_NATIVE_LOCAL_A", "AETHER_NATIVE_LOCAL_B"
    red, blue = [206, 48, 70, 255], [31, 96, 208, 255]
    a_pid_path, b_pid_path = owned / "a.pid", owned / "b.pid"
    forbidden_a, forbidden_b = owned / "forbidden-a", owned / "forbidden-b"
    draft_a, draft_b = (f"printf cross > {shlex.quote(str(path))}" for path in (forbidden_a, forbidden_b))
    launch("A", red, a, a_pid_path)
    marker_on_screen(a, b, red)
    a_pid, a_birth = shell_identity(a_pid_path)
    assert a_birth is not None
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first command adoption not settled")
    driver.type("#input", draft_a)
    detach()

    first = list_shells(1)
    assert first["second"], "second open was not expressly available"
    driver.click("#open-second-local")
    wait_for(lambda: state()["status"].startswith("Second local shell opened"), "explicit second local shell did not open")
    assert state()["writable"] and state()["draft"] == ""
    launch("B", blue, b, b_pid_path)
    marker_on_screen(b, a, blue)
    b_pid, b_birth = shell_identity(b_pid_path)
    assert b_birth is not None and b_pid != a_pid
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second command adoption not settled")
    driver.type("#input", draft_b)
    detach()

    choices = list_shells(2)
    assert not choices["choose"] and not choices["second"] and choices["selected"] == "", "ambiguous choice was implicitly authorized"
    a_label, b_label = sorted(choices["options"], key=int)
    assert a_label == first["options"][0] and a_label != b_label
    select(a_label)
    marker_on_screen(a, b, red)
    current = state()
    assert birth(a_pid) == a_birth
    assert not current["writable"] and not current["resize"] and current["draft"] == draft_a
    try:
        driver.click("#send")
    except ProtocolError as err:
        if err.error not in {"element not interactable", "invalid element state"}:
            raise
    detach()

    choices = list_shells(2)
    assert not choices["choose"] and choices["selected"] == ""
    select(b_label)
    marker_on_screen(b, a, blue)
    current = state()
    assert birth(b_pid) == b_birth
    assert not current["writable"] and not current["resize"] and current["draft"] == draft_b
    assert not forbidden_a.exists() and not forbidden_b.exists(), "a parked draft executed"
    close_without_stale_presentation()
    wait_for(lambda: birth(b_pid) != b_birth, "second owned shell not reaped")
    select(list_shells(1)["options"][0])
    marker_on_screen(a, b, red)
    assert state()["draft"] == draft_a and birth(a_pid) == a_birth
    close_without_stale_presentation()
    wait_for(lambda: birth(a_pid) != a_birth, "first owned shell not reaped")
    list_shells(0)
    assert not forbidden_a.exists() and not forbidden_b.exists(), "a parked draft executed"
    print(json.dumps({"native_webkitgtk": True, "owned_local_ptys": 2,
                      "distinct_canvas_colors_and_markers": True, "explicit_view_only_reattach": True,
                      "parked_drafts_isolated": True, "closed_canvas_and_status_cleared": True,
                      "owned_direct_children_reaped": True}), flush=True)


def stop_driver(child):
    # The driver launches the browser in this private process group.
    with contextlib.suppress(ProcessLookupError):
        os.killpg(child.pid, signal.SIGTERM)
    try:
        child.wait(timeout=4)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=4)


def interrupted(_signum, _frame):
    # Let the outer timeout take the same cleanup path as an ordinary failure.
    raise TimeoutError("native WebKit smoke interrupted")


def run(binary):
    executable = shutil.which("WebKitWebDriver")
    if not executable or not os.environ.get("DISPLAY") or not os.environ.get("XAUTHORITY"):
        raise RuntimeError("run with official WebKitWebDriver and authenticated Xvfb")
    with tempfile.TemporaryDirectory(prefix="aether-native-two-") as scratch:
        owned = Path(scratch)
        env = {name: os.environ[name] for name in ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS") if name in os.environ}
        env.update({"HOME": scratch, "PATH": "/usr/bin:/bin", "LC_ALL": "C.UTF-8",
                    "GDK_BACKEND": "x11", "LIBGL_ALWAYS_SOFTWARE": "1",
                    "TAURI_WEBVIEW_AUTOMATION": "true"})
        for name in ("CACHE", "CONFIG", "DATA", "RUNTIME", "STATE"):
            path = owned / name.lower()
            path.mkdir(mode=0o700)
            env["XDG_RUNTIME_DIR" if name == "RUNTIME" else f"XDG_{name}_HOME"] = str(path)
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        driver = Driver(port)
        with (owned / "driver.log").open("wb") as log:
            child = subprocess.Popen([executable, "--host=127.0.0.1", f"--port={port}"],
                                     env=env, cwd=owned, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            try:
                def ready():
                    if child.poll() is not None:
                        raise RuntimeError("WebKitWebDriver exited before readiness")
                    try:
                        return driver.request("GET", "/status", timeout=1).get("ready")
                    except OSError:
                        return False
                wait_for(ready, "loopback WebKitWebDriver not ready")
                driver.open(binary)
                wait_for(lambda: opened_page(driver), "embedded Aether tauri: page did not load")
                exercise(driver, owned)
            except Exception:
                with (owned / "driver.log").open("rb") as tail:
                    tail.seek(max(0, tail.seek(0, 2) - 4096))
                    print(tail.read().decode("utf-8", errors="replace"))
                raise
            finally:
                try:
                    driver.close()
                finally:
                    stop_driver(child)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="exact built tauri-terminal-lab executable")
    args = parser.parse_args()
    if not args.binary.is_absolute() or args.binary.is_symlink() or not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("an absolute regular executable is required")
    signal.signal(signal.SIGTERM, interrupted)
    run(args.binary)
