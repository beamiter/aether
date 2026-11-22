#!/usr/bin/env python3
"""Verify two real local Aether PTYs through the built helper, without SSH."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shlex
import signal
import stat
import subprocess
import tempfile
import time


LIMIT = 16 * 1024 * 1024
ROWS, COLS = 12, 100


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def until(predicate, label):
    end = time.monotonic() + 12
    while time.monotonic() < end:
        if value := predicate():
            return value
        time.sleep(0.02)
    raise TimeoutError(label)


def cancel(_number, _frame):
    # GNU timeout's TERM may also reach our child group. Unwind owned relay
    # and owner handles before the outer KILL deadline; do not signal PTY PIDs.
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    raise SystemExit("two-PTY fixture canceled")


def stop(process):
    try:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=1)
    finally:
        process.stdin.close()
        process.stdout.close()


class Relay:
    def __init__(self, process):
        self.process = process
        self.request = 0

    def call(self, kind, error=None, **args):
        self.request += 1
        request = self.request
        payload = json.dumps({"protocol": 1, "request": request,
                              "command": {"kind": kind, **args}},
                             separators=(",", ":")).encode("ascii") + b"\n"
        # This fixture only sends a few hundred bytes per synchronous request.
        require(len(payload) < 4096, "fixture exceeded its request cap")
        require(self.process.stdin.write(payload) == len(payload), "short relay write")
        self.process.stdin.flush()
        data = bytearray()
        fd = self.process.stdout.fileno()
        end = time.monotonic() + 5
        while True:
            remaining = end - time.monotonic()
            if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                raise TimeoutError(f"{kind} reply deadline")
            chunk = os.read(fd, 65536)
            require(chunk, f"{kind} relay exited without replying")
            data.extend(chunk)
            require(len(data) <= LIMIT, "oversized fixture reply")
            if b"\n" in chunk:
                require(data.endswith(b"\n") and data.count(b"\n") == 1,
                        "expected one newline-terminated response")
                break
        reply = json.loads(data)
        require(reply["protocol"] == 1 and reply["request"] == request,
                "response identity changed")
        result = reply["result"]
        if error is None:
            require(set(result) == {"Ok"}, f"{kind}: {result.get('Err')}")
            return result["Ok"]
        require(set(result) == {"Err"} and error in result["Err"],
                f"{kind}: expected {error!r}, got {result}")


class Grid:
    def __init__(self, owner):
        self.owner = owner
        self.frame = None

    def admit(self, frame, delta=False):
        owner = self.owner
        require(frame["session"] == owner["session"] and frame["epoch"] == owner["epoch"],
                "received grid for a different PTY")
        if delta:
            old = self.frame
            require(old and frame["base_revision"] == old["revision"] and
                    frame["layout"] == old["layout"], "unexpected delta base")
            rows = old["rows"].copy()
            seen = set()
            for changed in frame["rows"]:
                index = changed["index"]
                require(0 <= index < len(rows) and index not in seen, "invalid delta index")
                seen.add(index)
                rows[index] = changed["cells"]
            frame = {**old, **frame, "rows": rows}
        if self.frame:
            require(frame["revision"] > self.frame["revision"], "non-increasing within-view revision")
        require(frame["cols"] == COLS and len(frame["rows"]) == ROWS and
                all(len(row) == COLS for row in frame["rows"]), "unexpected dimensions")
        self.frame = frame

    def poll(self, relay):
        if update := relay.call("poll", owner=self.owner):
            require(update["kind"] in ("snapshot", "delta"), "unexpected frame kind")
            frame = update["frame"]
            self.admit(frame, update["kind"] == "delta")
            relay.call("ack", owner=self.owner, revision=frame["revision"])
        return self.text()

    def text(self):
        if not self.frame:
            return ""
        return "\n".join("".join(c["text"] for c in row if c["width"]) for row in self.frame["rows"])


def write(relay, owner, line):
    data = line.encode("ascii") + b"\n"
    require(relay.call("write", owner=owner, bytes=list(data)) == len(data),
            "whole synthetic line was not accepted")


def marker(relay, grid, label):
    pattern = re.compile(rf"\b{re.escape(label)}=([0-9]+):([a-z]+)\b")
    return until(lambda: pattern.search(grid.poll(relay)), label).groups()


def shell_line(label, assignment=""):
    # The exact marker never occurs verbatim in the command echo.
    prefix, suffix = label[:-1], label[-1]
    return f"{assignment}printf '\\n%s%s=%s:%s\\n' {prefix} {suffix} \"$$\" \"$aether_fixture\""


def exercise(helper, root, cleanup, env):
    endpoint = root / "owner.sock"
    owner_log = root / "owner.log"

    def start(mode, name):
        with (root / f"{name}.log").open("xb") as log:
            p = subprocess.Popen([helper, mode, endpoint], cwd=root, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=log, bufsize=0)
        cleanup.callback(stop, p)
        return p

    daemon = start("serve", "owner")
    def ready():
        if daemon.poll() is not None:
            with owner_log.open("rb") as log:
                detail = log.read(8192).decode("utf-8", errors="replace")
            raise RuntimeError(f"terminal owner failed: {detail}")
        try:
            socket = endpoint.lstat()
        except FileNotFoundError:
            return False
        require(stat.S_ISSOCK(socket.st_mode) and socket.st_uid == os.geteuid(),
                "unexpected private endpoint")
        # The real owner chmods after bind. Do not start a relay early.
        return stat.S_IMODE(socket.st_mode) == 0o600
    until(ready, "private owner socket")
    run_sessions(root, lambda name: Relay(start("stdio", name)))


def run_sessions(root, start_relay):
    gate, written = root / "release-a", root / "written-a"
    a_relay = start_relay("relay-a")
    b_relay = start_relay("relay-b")
    a = a_relay.call("open", rows=ROWS, cols=COLS)
    b = b_relay.call("open", rows=ROWS, cols=COLS)
    require(all(re.fullmatch(r"[0-9a-f]{32}", o["session"]) for o in (a, b)) and
            a["session"] != b["session"], "two PTYs must have distinct identities")
    require(all(o["epoch"] == 1 and o["generation"] == 1 for o in (a, b)),
            "fresh helper must issue first attachment for each PTY")
    ag, bg = Grid(a), Grid(b)
    write(a_relay, a, shell_line("A_READY", "{ stty -echo && aether_fixture=alpha; } && "))
    write(b_relay, b, shell_line("B_READY", "{ stty -echo && aether_fixture=bravo; } && "))
    apid, av = marker(a_relay, ag, "A_READY")
    bpid, bv = marker(b_relay, bg, "B_READY")
    require(apid != bpid and (av, bv) == ("alpha", "bravo"),
            "double open must create independent shell state")
    require("B_READY=" not in ag.text() and "A_READY=" not in bg.text(),
            "initial paint contains another session's output")

    # Commit exactly one bounded command to A before detaching. The sidecar is
    # written only after printf writes the marker; host fixture waits for it
    # before creating the replacement relay, not merely before its first poll.
    gp, wp = (shlex.quote(str(p)) for p in (gate, written))
    armed = "printf '\\n%s%s\\n' A_ARME D; "
    waiting = f"i=0; while [ ! -f {gp} ] && [ \"$i\" -lt 200 ]; do sleep .05; i=$((i+1)); done; "
    after = shell_line("A_DETACHED")
    write(a_relay, a, f"{armed}{waiting}[ -f {gp} ] && {{ {after} && printf '%s' \"$$\" > {wp}; }}")
    until(lambda: "A_ARMED" in ag.poll(a_relay), "A command is running before detach")
    a_relay.call("detach", owner=a)
    a_relay.call("poll", owner=a, error="stale terminal attachment")
    stop(a_relay.process)
    gate.touch(exist_ok=False)
    write(b_relay, b, shell_line("B_STILL"))
    require(marker(b_relay, bg, "B_STILL") == (bpid, "bravo"), "B stopped while A detached")
    require("A_DETACHED=" not in bg.text(), "A detached output crossed into B")
    def completed():
        return written.is_file() and written.read_text() == apid
    until(completed, "A child wrote detached output before any new relay")

    new = start_relay("relay-a-reattach")
    resumed = new.call("attach_read_only", session=a["session"], epoch=a["epoch"])
    a2 = resumed["owner"]
    require(a2["session"] == a["session"] and a2["epoch"] == a["epoch"] and
            a2["generation"] == a["generation"] + 2, "reattachment recreated the PTY or kept old view")
    view = Grid(a2)
    view.admit(resumed["frame"])
    new.call("ack", owner=a2, revision=view.frame["revision"])
    require(marker(new, view, "A_DETACHED") == (apid, "alpha"),
            "A reattached to a different shell or lost previously detached output")
    require(view.text().count("A_DETACHED=") == 1 and "B_STILL=" not in view.text(),
            "detached output duplicated or terminal boundaries crossed")
    new.call("write", owner=a2, bytes=[120], error="read-only")
    new.call("ack", owner=a, revision=view.frame["revision"], error="stale terminal attachment")
    new.call("close", owner=a, error="stale terminal attachment")
    new.call("close", owner=a2)
    new.call("status", owner=a2, error="terminal session unavailable")
    write(b_relay, b, shell_line("B_AFTER_A"))
    require(marker(b_relay, bg, "B_AFTER_A") == (bpid, "bravo"), "closing A killed B")
    b_relay.call("close", owner=b)
    b_relay.call("status", owner=b, error="terminal session unavailable")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("helper", type=Path, help="exact built aether-terminal-host executable")
    args = parser.parse_args()
    helper = args.helper.resolve(strict=True)
    require(helper.is_file() and os.access(helper, os.X_OK), "helper is not executable")
    with helper.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    signal.signal(signal.SIGTERM, cancel)
    with tempfile.TemporaryDirectory(prefix="aether-dual-pty-") as name:
        root = Path(name)
        env = {"PATH": "/usr/bin:/bin", "HOME": name, "XDG_RUNTIME_DIR": name,
               "LC_ALL": "C", "LANG": "C"}
        with contextlib.ExitStack() as cleanup:
            exercise(helper, root, cleanup, env)
    print(f"PASS built helper {digest}: 2 independent local PTYs; A preserved same PID, "
          "shell state and epoch after detach/reattach; B stayed writable; both closed")


if __name__ == "__main__":
    main()
