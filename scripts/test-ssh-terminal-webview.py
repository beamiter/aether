#!/usr/bin/env python3
"""Drive Aether’s native WebKitGTK terminal over an isolated Xvfb display and SSH."""
import argparse
import contextlib
import json
import os
from pathlib import Path, PurePosixPath
import runpy
import select
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile

DETACHED_WRITES = 8192
DETACHED_WRITE = b"\x1b[12;1H" + b" " * 64

READ = """
return {
    status: document.querySelector('#status').textContent,
    writable: !document.querySelector('#input').disabled &&
              !document.querySelector('#send').disabled,
    draft: document.querySelector('#input').value,
    focused: document.hasFocus() ? document.activeElement?.id : null,
    snapshot: document.querySelector('#history-output').value,
    pixel: Array.from(document.querySelector('main canvas')
                      .getContext('2d').getImageData(2, 2, 1, 1).data),
};
"""


def stop(child):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(4)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(4)


def owned_desktop(app, group):
    processes = subprocess.check_output(["ps", "-eo", "pid=,pgid="], text=True)
    for entry in processes.splitlines():
        pid, pgid = map(int, entry.split())
        if pgid != group:
            continue
        with contextlib.suppress(FileNotFoundError, PermissionError, ProcessLookupError):
            if os.path.samefile(f"/proc/{pid}/exe", app):
                return pid
    return None


def exercise(app, destination, helper):
    probe = runpy.run_path(str(Path(__file__).with_name("test-ssh-terminal-session.py")))
    native = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))
    wait = native["wait_for"]
    executable = shutil.which("WebKitWebDriver")
    if not executable or not os.environ.get("DISPLAY") or not os.environ.get("XAUTHORITY"):
        raise RuntimeError("native SSH smoke needs WebKitWebDriver and authenticated Xvfb")
    root = Path(tempfile.mkdtemp(prefix="jwm-aether-ssh-webview."))
    print(root, flush=True)

    def start(args, name, *, group=False):
        with (root / name).open("wb") as log:
            child = subprocess.Popen(args, env=env, cwd=root, stdout=log, stderr=log,
                                     start_new_session=group)
        cleanup.callback(native["stop_driver"] if group else stop, child)
        return child

    with contextlib.ExitStack() as cleanup:
        supervisor = probe["Channel"](destination, ["python3", "-c", probe["SUPERVISE"], helper])
        cleanup.callback(supervisor.close)
        endpoint = supervisor.line().decode()
        remote_socket = PurePosixPath(endpoint)
        owned = remote_socket.parent
        assert remote_socket.is_absolute() and remote_socket.name == "terminal.sock"
        assert owned.name.startswith("aether-ssh-")

        env = os.environ | {
            "GDK_BACKEND": "x11", "LIBGL_ALWAYS_SOFTWARE": "1",
            "TAURI_WEBVIEW_AUTOMATION": "true",
            "AETHER_SSH_TERMINAL_HELPER": helper, "AETHER_SSH_TERMINAL_SOCKET": endpoint,
            "JTERM_WORKSPACE_STATE_DIR": str(root / "workspace"),
        }
        for name in ("CONFIG", "CACHE", "RUNTIME", "DATA", "STATE"):
            directory = root / name.lower()
            directory.mkdir(mode=0o700)
            env["XDG_RUNTIME_DIR" if name == "RUNTIME" else f"XDG_{name}_HOME"] = str(directory)

        start(["metacity", "--sm-disable"], "wm.log")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        driver = native["Driver"](port)
        webkit = start([executable, "--host=127.0.0.1", f"--port={port}"], "driver.log", group=True)

        def driver_ready():
            if webkit.poll() is not None:
                raise RuntimeError("WebKitWebDriver exited before readiness")
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        wait(driver_ready, "loopback WebKitWebDriver did not start")
        driver.open(app)
        cleanup.callback(driver.close)
        wait(lambda: native["opened_page"](driver), "embedded Aether tauri: page did not load")
        desktop = wait(lambda: owned_desktop(app, webkit.pid), "owned native Aether process")

        def run(*args):
            return subprocess.run(list(map(str, args)), env=env, text=True, check=True,
                                  capture_output=True, timeout=4).stdout

        def find_window():
            found = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(desktop)],
                                   env=env, text=True, capture_output=True, timeout=4)
            return found.stdout.strip().splitlines()[-1] if found.stdout.strip() else None

        window = wait(find_window, "native Aether WebView window")
        run("xdotool", "windowactivate", "--sync", window)

        def snap(name):
            run("import", "-window", window, root / (name + ".png"))

        def relays():
            found = subprocess.run(["ps", "--ppid", str(desktop), "-o", "pid=,comm="],
                                   env=env, text=True, capture_output=True, timeout=4)
            return [int(pid) for pid, comm in (line.split(None, 1)
                    for line in found.stdout.splitlines()) if comm == "ssh"]

        def cut_relay(relay):
            assert relays() == [relay]
            relay_fd = os.pidfd_open(relay)
            try:
                argv = Path(f"/proc/{relay}/cmdline").read_bytes().split(b"\0")
                assert destination.encode() in argv and any(endpoint.encode() in arg for arg in argv)
                signal.pidfd_send_signal(relay_fd, signal.SIGKILL)
            finally:
                os.close(relay_fd)
            wait(lambda: not relays(), "old OpenSSH child is reaped after loss")
            wait(lambda: state()["status"].startswith("SSH terminal paused:"),
                 "native WebView did not surface relay loss before recovery")

        def state():
            return driver.execute(READ)

        def clipboard():
            try:
                return run("xclip", "-o", "-selection", "clipboard")
            except subprocess.CalledProcessError:
                return ""

        def focused_control(control):
            wait(lambda: state()["focused"] == control, f"native WebView did not focus {control}")

        def submit(command):
            assert state()["writable"], "view lost input rights before command"
            run("xdotool", "windowactivate", "--sync", window)
            driver.clear("#input")
            # Home focuses the command control without submitting any text.
            driver.type("#input", "\ue011")
            owner = subprocess.Popen(["xclip", "-selection", "clipboard", "-i", "-quiet"],
                                     env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
            try:
                with owner.stdin as writer:
                    writer.write(command.encode())
                wait(lambda: clipboard() == command, "X11 clipboard did not take ownership")
                focused_control("input")
                run("xdotool", "key", "--clearmodifiers", "ctrl+v")
                wait(lambda: state()["draft"] == command, "native WebView did not paste clipboard")
            except AssertionError:
                current = state()
                print("SSH paste state:", current["status"], "focused:", current["focused"],
                      "writable:", current["writable"], "draft length:", len(current["draft"]), flush=True)
                snap("paste-failed")
                raise
            finally:
                stop(owner)
            driver.type("#send", "\ue007")
            try:
                wait(lambda: not state()["draft"], "native WebView did not send pasted command")
            except AssertionError:
                current = state()
                print("SSH input state:", current["status"], "writable:", current["writable"],
                      "draft length:", len(current["draft"]), flush=True)
                snap("send-failed")
                raise

        def visible(marker):
            driver.click("#copy-visible")
            lines = [line.strip() for line in state()["snapshot"].splitlines()]
            return lines if marker in lines else None

        def assert_clipboard(lines):
            driver.click("#history-output")
            focused_control("history-output")
            run("xdotool", "key", "--clearmodifiers", "ctrl+a", "ctrl+c")
            wait(lambda: [line.strip() for line in clipboard().splitlines()] == lines,
                 "native WebView did not copy the recovered grid to X11")

        snap("start")
        driver.type("#ssh-terminal-host", destination)
        driver.click("#open-ssh")
        first, = wait(relays, "Aether starts OpenSSH")
        wait(lambda: state()["writable"], "remote shell does not accept input")
        snap("connected")

        submit("stty -echo; printf '\\033[1;1H\\033[48;2;22;63;91m%20s\\033[0m' ''; "
               "printf '\\033[2;1H\\033[2K%s\\r\\n' ECHO_DISABLED")
        wait(lambda: visible("ECHO_DISABLED"), "initial remote output reaches WebKit")
        wait(lambda: state()["pixel"] == [22, 63, 91, 255], "remote RGB row is not painted on canvas")
        quote = shlex.quote
        detached_writer = shlex.join([
            "python3", "-c",
            f"import sys; sys.stdout.buffer.write({DETACHED_WRITE!r} * {DETACHED_WRITES})",
        ])
        submit("SENTINEL=WEBVIEW_PTY_SURVIVED; "
               f"printf '%s' \"$$\" > {quote(str(owned / 'pty-pid'))}; "
               "printf '\\033[3;1H\\033[2K%s\\r\\n' WEBVIEW_READY; "
               f"(n=0; while [ ! -f {quote(str(owned / 'proceed'))} ] && [ \"$n\" -lt 600 ]; "
               "do n=$((n+1)); sleep 0.05; done; "
               f"if [ -f {quote(str(owned / 'proceed'))} ]; then "
               f"{detached_writer} || exit; "
               "printf '\\033[7;1H\\033[2K%s\\r\\n' WEBVIEW_DETACHED_OUTPUT; "
               f"printf done > {quote(str(owned / 'written'))}; fi) &")
        before = wait(lambda: visible("WEBVIEW_READY"), "initial marker in canvas grid")
        assert "WEBVIEW_DETACHED_OUTPUT" not in before
        snap("before-cut")

        cut_relay(first)
        remote_pid, born = json.loads(probe["remote_python"](
            destination, probe["RELEASE_DETACHED_OUTPUT"], str(owned)))
        snap("link-lost")

        driver.click("#recover")
        second, = wait(relays, "recovery starts new OpenSSH child")
        assert first != second
        recovered = wait(lambda: visible("WEBVIEW_DETACHED_OUTPUT"),
                         "disconnected output is missing after recovery")
        assert recovered.count("WEBVIEW_READY") == recovered.count("WEBVIEW_DETACHED_OUTPUT") == 1
        assert state()["pixel"] == [22, 63, 91, 255], "recovered canvas lost the remote RGB row"
        assert_clipboard(recovered)
        snap("recovered")

        wait(lambda: state()["writable"], "recovered view did not regain input rights")
        submit("printf '\\033[9;1H\\033[2K%s\\r\\n' \"$SENTINEL\"")
        wait(lambda: visible("WEBVIEW_PTY_SURVIVED"), "recovered shell lost its variable or input")

        driver.click("#detach")
        wait(lambda: not relays() and state()["status"].startswith("View detached."),
             "explicit detach did not retire relay")
        snap("detached")
        driver.click("#attach")
        third, = wait(relays, "read-only attach creates fresh relay")
        assert third not in (first, second)
        readonly = wait(lambda: visible("WEBVIEW_PTY_SURVIVED"), "read-only reattachment lost the grid")
        assert "view-only" in state()["status"] and not state()["writable"]
        for marker in ["WEBVIEW_READY", "WEBVIEW_DETACHED_OUTPUT", "WEBVIEW_PTY_SURVIVED"]:
            assert readonly.count(marker) == 1
        snap("read-only")

        forbidden = str(owned / "read-only-write")
        native["click_visible_terminal"](driver)
        run("xdotool", "type", "--clearmodifiers", f"printf forbidden > {quote(forbidden)}")
        run("xdotool", "key", "Return")
        probe["remote_python"](destination,
            "import pathlib, sys; assert not pathlib.Path(sys.argv[1]).exists(), 'read-only view sent input'",
            forbidden,
        )
        driver.click("#close")
        wait(lambda: not relays() and state()["status"].startswith("Terminal closed"),
             "explicit close did not retire relay")
        probe["remote_python"](destination, probe["CONFIRM_REAPED"], str(remote_pid), born)
        snap("closed")

        driver.click("#open-ssh")
        fourth, = wait(relays, "second Aether SSH terminal did not start")
        wait(lambda: state()["writable"], "second remote terminal was read-only before exit")
        identity_path = str(owned / "exit-identity")
        gate = str(owned / "allow-exit")
        submit(f"printf '%s %s\\n' \"$$\" \"$(awk '{{print $22}}' /proc/$$/stat)\" > {quote(identity_path)}; "
               "printf '%s%s\\n' EXIT_ READY; "
               f"while [ ! -f {quote(gate)} ]; do sleep 0.05; done; "
               "printf '\\033[10;1H\\033[2K%s%s\\r\\n' EXIT_ OUTPUT; exit")
        wait(lambda: visible("EXIT_READY"), "second remote shell did not reach its exit gate")
        cut_relay(fourth)
        remote_pid, born = probe["remote_python"](
            destination,
            "import pathlib,sys; pathlib.Path(sys.argv[2]).touch(); print(pathlib.Path(sys.argv[1]).read_text())",
            identity_path, gate,
        ).split()
        probe["remote_python"](destination, probe["CONFIRM_REAPED"], remote_pid, born)
        driver.click("#recover")
        fifth, = wait(relays, "exited terminal recovery did not reconnect")
        assert fifth != fourth
        wait(lambda: visible("EXIT_OUTPUT") and "Input remains disabled" in state()["status"],
             "recovered exit output did not carry the host's read-only input grant")
        assert not state()["writable"]
        driver.click("#close")
        wait(lambda: not relays(), "closing the recovered exit output left an SSH relay")

        desktop_fd = os.pidfd_open(desktop)
        try:
            driver.close()
            assert select.select([desktop_fd], [], [], 7)[0], "owned Aether WebView did not exit"
        finally:
            os.close(desktop_fd)
        return dict(
            native_webview=True, ssh_children=5, detached_content_recovered=True,
            same_pty_input_resumed=True, read_only_grid=True, read_only_write_blocked=True,
            remote_pty_reaped=True, clean_native_exit=True, native_clipboard=True,
            native_remote_rgb_paint=True, detached_pty_bytes=DETACHED_WRITES * len(DETACHED_WRITE),
            recovered_exit_output_stays_read_only=True,
            artifacts=str(root),
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path, help="path to the built Aether Tauri executable")
    parser.add_argument("destination", help="SSH destination with a verified host key")
    parser.add_argument("helper", help="absolute path to aether-terminal-host on the SSH host")
    args = parser.parse_args()
    app = args.app.resolve()
    if not app.is_file() or not os.access(app, os.X_OK):
        parser.error("the Aether Tauri executable must exist and be executable")
    # Use a private display and session bus even when invoked from a live desktop.
    launch = ("import json, runpy, sys; smoke = runpy.run_path(sys.argv[1]); "
              "print(json.dumps(smoke['exercise'](*sys.argv[2:])), flush=True)")
    os.execvp("dbus-run-session", [
        "dbus-run-session", "--", "xvfb-run", "-a", "-s", "-screen 0 1600x1200x24 -nolisten tcp",
        sys.executable, "-c", launch, str(Path(__file__).resolve()), str(app), args.destination, args.helper,
    ])
