#!/usr/bin/env python3
"""Drive Aether’s native WebKitGTK terminal over an isolated Xvfb display and SSH."""
import argparse
import contextlib
import errno
import json
import os
from pathlib import Path, PurePosixPath
import runpy
import select
import shlex
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

DETACHED_WRITES = 8192
DETACHED_WRITE = b"\x1b[12;1H" + b" " * 64

READ = """
return {
    status: document.querySelector('#status').textContent,
    health: document.querySelector('#health').textContent,
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


def exercise(app, destination, helper, legacy_helper=False):
    probe = runpy.run_path(str(Path(__file__).with_name("test-ssh-terminal-session.py")))
    native = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))
    wait = native["wait_for"]
    executable = shutil.which("WebKitWebDriver")
    if not executable or not os.environ.get("DISPLAY") or not os.environ.get("XAUTHORITY"):
        raise RuntimeError("native SSH smoke needs WebKitWebDriver and authenticated Xvfb")
    root = Path(tempfile.mkdtemp(prefix="jwm-aether-ssh-webview."))
    print(root, flush=True)

    def start(args, name, *, group=False, child_env=None):
        with (root / name).open("wb") as log:
            child = subprocess.Popen(args, env=env if child_env is None else child_env,
                                     cwd=root, stdout=log, stderr=log,
                                     start_new_session=group)
        cleanup.callback(native["stop_driver"] if group else stop, child)
        return child

    with contextlib.ExitStack() as cleanup:
        supervisor = probe["Channel"](destination, ["python3", "-c", probe["SUPERVISE"], helper])
        # The remote supervisor can spend ten seconds stopping and reaping
        # the owner before the SSH channel can close.
        cleanup.callback(supervisor.close, timeout=15)
        endpoint = supervisor.line().decode()
        remote_socket = PurePosixPath(endpoint)
        owned = remote_socket.parent
        assert remote_socket.is_absolute() and remote_socket.name == "terminal.sock"
        assert owned.name.startswith("aether-ssh-")

        env = {name: os.environ[name] for name in (
            "DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "XAUTHORITY", "GDK_SCALE", "PATH",
            "SSH_AUTH_SOCK", "XDG_DATA_DIRS",
        ) if name in os.environ}
        env.update({
            "HOME": str(root), "LC_ALL": "C.UTF-8", "GDK_BACKEND": "x11",
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "TAURI_WEBVIEW_AUTOMATION": "true",
            "AETHER_SSH_TERMINAL_HELPER": helper, "AETHER_SSH_TERMINAL_SOCKET": endpoint,
            "JTERM_WORKSPACE_STATE_DIR": str(root / "workspace"),
        })
        for name in ("CONFIG", "CACHE", "RUNTIME", "DATA", "STATE"):
            directory = root / name.lower()
            directory.mkdir(mode=0o700)
            env["XDG_RUNTIME_DIR" if name == "RUNTIME" else f"XDG_{name}_HOME"] = str(directory)

        manager = shutil.which("metacity") or shutil.which("openbox")
        assert manager, "native SSH smoke needs an X11 window manager"
        manager_env = env.copy()
        if "LD_LIBRARY_PATH" in os.environ:
            manager_env["LD_LIBRARY_PATH"] = os.environ["LD_LIBRARY_PATH"]
        start([manager, "--sm-disable"], "wm.log", child_env=manager_env)
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

        def run(*args, timeout=4):
            return subprocess.run(list(map(str, args)), env=env, text=True, check=True,
                                  capture_output=True, timeout=timeout).stdout

        def find_window():
            found = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(desktop)],
                                   env=env, text=True, capture_output=True, timeout=4)
            return found.stdout.strip().splitlines()[-1] if found.stdout.strip() else None

        window = wait(find_window, "native Aether WebView window")
        run("xdotool", "windowactivate", "--sync", window)

        def snap(name):
            run("import", "-limit", "thread", 1, "-window", window, root / (name + ".png"), timeout=12)

        def stop_desktop():
            desktop_fd = os.pidfd_open(desktop)
            try:
                driver.close()
                assert select.select([desktop_fd], [], [], 7)[0], "owned Aether process did not exit"
            finally:
                os.close(desktop_fd)

        def restart_desktop():
            old = desktop
            driver.open(app)
            wait(lambda: native["opened_page"](driver), "restarted Aether page did not load")
            pid = wait(lambda: owned_desktop(app, webkit.pid), "restarted Aether process")
            assert pid != old
            return pid

        def catalog():
            path = root / "data" / "local.jterm.frame-lab" / "ssh-terminals" / "sessions.json"
            observed = path.stat()
            assert stat.S_IMODE(observed.st_mode) == 0o600 and observed.st_uid == os.geteuid()
            saved = json.loads(path.read_text())
            assert saved["version"] == 1
            return saved["shells"]

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

        def wait_for_input():
            try:
                wait(lambda: state()["writable"], "remote shell does not accept input")
            except AssertionError as error:
                snap("input-unavailable")
                current = state()
                raise AssertionError(f"{error}. Aether reported {current['status']!r} "
                                     f"and {current['health']!r}") from error

        def close_shell():
            driver.click("#close")
            try:
                wait(lambda: not relays() and state()["status"].startswith("Terminal closed"),
                     "explicit close did not retire the SSH relay", timeout=18)
            except AssertionError as error:
                snap("close-unconfirmed")
                raise AssertionError(f"{error}. Aether reported {state()['status']!r}") from error

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
        wait_for_input()
        first, = wait(relays, "Aether starts OpenSSH")
        snap("connected")

        if legacy_helper:
            assert "Restart recovery requires a current SSH terminal helper" in state()["status"]
            submit("printf '%s%s\\r\\n' LEGACY_ WORKS")
            wait(lambda: visible("LEGACY_WORKS"), "the older helper did not carry shell input and output")
            close_shell()
            driver.click("#show-saved-ssh")
            wait(lambda: state()["status"] == "No saved SSH shells.", "the older helper created a saved entry")
            stop_desktop()
            return dict(legacy_helper=True, shell_input_and_output=True,
                        saved_catalog_remains_empty=True, artifacts=str(root))

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

        restart_gate = str(owned / "allow-restart-output")
        witness = str(owned / "restart-written")
        submit(
            f"(n=0; while [ ! -f {quote(restart_gate)} ] && [ \"$n\" -lt 600 ]; "
            "do n=$((n+1)); sleep 0.05; done; "
            f"if [ -f {quote(restart_gate)} ]; then "
            "printf '\\033[6;1H\\033[2K%s\\r\\n' \"$SENTINEL\"_RESTART; "
            f"printf done > {quote(witness)}; fi) & "
            "printf '\\033[5;1H\\033[2K%s\\r\\n' WEBVIEW_RESTART_ARMED"
        )
        wait(lambda: visible("WEBVIEW_RESTART_ARMED"), "remote shell starts the restart witness")
        saved, = catalog()
        assert set(saved) == {"id", "destination", "instance", "target", "created_ms"}
        assert saved["destination"] == destination
        assert set(saved["target"]) == {"session", "epoch"}

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
        stop_desktop()
        actual_identity = json.loads(probe["remote_python"](
            destination,
            "import json,pathlib,sys,time; gate,witness,owner=map(pathlib.Path,sys.argv[1:]); "
            "gate.touch(); deadline=time.monotonic()+5; "
            "exec('while not witness.exists():\\n assert time.monotonic()<deadline\\n time.sleep(.02)'); "
            "pid=int(owner.read_text()); "
            "born=pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19]; "
            "print(json.dumps([pid,born]))",
            restart_gate, witness, str(owned / "pty-pid"),
        ))
        assert actual_identity == [remote_pid, born], "restart witness ran in a different PTY"

        desktop = restart_desktop()
        window = wait(find_window, "restarted native WebView window")
        run("xdotool", "windowactivate", "--sync", window)
        assert not relays(), "desktop startup implicitly reconnected an SSH shell"
        driver.click("#show-saved-ssh")
        wait(lambda: driver.execute("return !document.querySelector('#resume-saved-ssh').disabled"),
             "the native saved shell directory did not appear after restart")
        assert not relays(), "listing saved shells implicitly connected to an SSH host"
        driver.click("#resume-saved-ssh")
        resumed, = wait(relays, "saved view creates a new SSH relay")
        assert resumed not in (first, second, third)
        restarted_grid = wait(lambda: visible("WEBVIEW_PTY_SURVIVED_RESTART"),
                              "saved view lost output produced while Aether was closed")
        assert restarted_grid.count("WEBVIEW_PTY_SURVIVED_RESTART") == 1
        assert "view-only" in state()["status"] and not state()["writable"]
        assert state()["pixel"] == [22, 63, 91, 255]
        snap("application-restart")

        close_shell()
        probe["remote_python"](destination, probe["CONFIRM_REAPED"], str(remote_pid), born)
        assert catalog() == [], "closing the remote PTY left a saved entry"
        snap("closed")

        driver.type("#ssh-terminal-host", destination)
        driver.click("#open-ssh")
        fourth, = wait(relays, "second Aether SSH terminal did not start")
        wait_for_input()
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
        wait(lambda: driver.execute("return !document.querySelector('#detach').disabled"),
             "recovery should finish before closing the exit output")
        close_shell()

        driver.click("#open-ssh")
        wait(lambda: len(relays()) == 1 and state()["writable"], "third SSH shell did not open")
        old_entry, = catalog()
        stop_desktop()
        supervisor.child.stdin.write(b"restart\n")
        supervisor.child.stdin.flush()
        assert supervisor.line().decode() == endpoint

        desktop = restart_desktop()
        window = wait(find_window, "native window after SSH owner replacement")
        run("xdotool", "windowactivate", "--sync", window)
        driver.click("#show-saved-ssh")
        wait(lambda: driver.execute("return !document.querySelector('#resume-saved-ssh').disabled"),
             "saved entry is missing before owner replacement check")
        driver.click("#resume-saved-ssh")
        wait(lambda: "SSH terminal owner changed" in state()["status"], "replacement owner was not surfaced")
        assert not relays() and not state()["writable"]
        assert catalog() == [old_entry], "replacement owner discarded a saved identity"
        driver.click("#show-saved-ssh")
        wait(lambda: driver.execute("return !document.querySelector('#forget-saved-ssh').disabled"),
             "saved entry is unavailable for removal")
        driver.click("#forget-saved-ssh")
        try:
            wait(lambda: state()["status"] == "No saved SSH shells.", "saved entry was not forgotten")
        except AssertionError as error:
            snap("forget-unconfirmed")
            raise AssertionError(f"{error}. Aether reported {state()['status']!r} "
                                 f"with {len(catalog())} saved shells on disk") from error
        assert catalog() == []
        stop_desktop()

        return dict(
            native_webview=True, detached_content_recovered=True,
            same_pty_input_resumed=True, read_only_grid=True, read_only_write_blocked=True,
            remote_pty_reaped=True, clean_native_exit=True, native_clipboard=True,
            native_remote_rgb_paint=True, detached_pty_bytes=DETACHED_WRITES * len(DETACHED_WRITE),
            recovered_exit_output_stays_read_only=True,
            native_application_restart_kept_the_same_remote_pty=True,
            detached_output_recovered_across_application_restart=True,
            owner_restart_refused_to_attach_and_preserved_record=True,
            artifacts=str(root),
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path, help="path to the built Aether Tauri executable")
    parser.add_argument("destination", help="SSH destination with a verified host key")
    parser.add_argument("helper", help="absolute path to aether-terminal-host on the SSH host")
    parser.add_argument("--legacy-helper", action="store_true", help="verify shell access with a helper released before the catalog")
    args = parser.parse_args()
    app = args.app.resolve()
    if not app.is_file() or not os.access(app, os.X_OK):
        parser.error("the Aether Tauri executable must exist and be executable")
    # Isolate the display and the service activation environment from the live desktop.
    launch = ("import json, runpy, sys; smoke = runpy.run_path(sys.argv[1]); "
              "print(json.dumps(smoke['exercise'](*sys.argv[2:5], sys.argv[5] == '1')), flush=True)")
    scratch = tempfile.mkdtemp(prefix="jwm-aether-ssh-bus.")
    try:
        bus_env = os.environ | {"XDG_RUNTIME_DIR": scratch}
        for name in ("CACHE", "CONFIG", "DATA", "STATE"):
            directory = Path(scratch) / name.lower()
            directory.mkdir(mode=0o700)
            bus_env[f"XDG_{name}_HOME"] = str(directory)
        status = subprocess.call([
            "dbus-run-session", "--", "xvfb-run", "-a", "-s", "-screen 0 1600x1200x24 -nolisten tcp",
            sys.executable, "-c", launch, str(Path(__file__).resolve()), str(app), args.destination, args.helper,
            str(int(args.legacy_helper)),
        ], env=bus_env)
    finally:
        # The portal's private FUSE mount can disconnect just before unmounting.
        deadline = time.monotonic() + 3
        while True:
            try:
                shutil.rmtree(scratch)
                break
            except OSError as error:
                if error.errno not in {errno.EBUSY, errno.ECONNABORTED, errno.ENOTCONN, errno.EPERM} or time.monotonic() >= deadline:
                    raise
                time.sleep(0.05)
    sys.exit(status)
