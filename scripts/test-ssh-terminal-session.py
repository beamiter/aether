#!/usr/bin/env python3
"""Exercise a real remote PTY across independent OpenSSH connections."""
import argparse
import contextlib
import json
import os
import pathlib
import select
import shlex
import subprocess
import time

SSH = [
    "ssh", "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
    "-o", "ConnectTimeout=5", "-o", "ControlMaster=no", "-o", "ControlPath=none",
    "-o", "ClearAllForwardings=yes",
]
MAX_RECORD = 16 * 1024 * 1024

SUPERVISE = '''
import pathlib, socket, subprocess, sys, tempfile, time
with tempfile.TemporaryDirectory(prefix='aether-ssh-') as directory:
    endpoint = pathlib.Path(directory) / 'terminal.sock'

    def launch_owner():
        owner = subprocess.Popen([sys.argv[1], 'serve', str(endpoint)], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL)
        deadline = time.monotonic() + 6
        try:
            while True:
                assert owner.poll() is None, 'terminal owner exited before binding'
                assert time.monotonic() < deadline, 'terminal owner socket timed out'
                try:
                    with socket.socket(socket.AF_UNIX) as probe:
                        probe.connect(str(endpoint))
                    return owner
                except (FileNotFoundError, ConnectionRefusedError):
                    time.sleep(0.02)
        except BaseException:
            reap_owner(owner)
            raise

    def reap_owner(owner):
        if owner.poll() is None:
            owner.terminate()
        try:
            owner.wait(5)
        except subprocess.TimeoutExpired:
            owner.kill()
            owner.wait(5)

    owner = launch_owner()
    try:
        print(endpoint, flush=True)
        for command in sys.stdin.buffer:
            assert command.strip() == b'restart'
            reap_owner(owner)
            owner = launch_owner()
            print(endpoint, flush=True)
    finally:
        reap_owner(owner)
'''

RELEASE_DETACHED_OUTPUT = '''
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
(root / 'proceed').touch()
deadline = time.monotonic() + 5
while not (root / 'written').is_file():
    assert time.monotonic() < deadline, 'PTY did not produce output while SSH was closed'
    time.sleep(0.02)
pid = int((root / 'pty-pid').read_text())
stat = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
print(json.dumps([pid, stat[19]]))
'''

CONFIRM_REAPED = '''
import pathlib, sys, time
stat = pathlib.Path(f'/proc/{int(sys.argv[1])}/stat')
deadline = time.monotonic() + 5
while True:
    try:
        state = stat.read_text().rsplit(')', 1)[1].split()
    except FileNotFoundError:
        break
    if state[19] != sys.argv[2]:
        break
    assert time.monotonic() < deadline, 'owned PTY child did not exit after close'
    time.sleep(0.02)
'''


class Channel:
    def __init__(self, destination, command):
        self.child = subprocess.Popen(
            [*SSH, "--", destination, shlex.join(command)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        )
        self.buffer = bytearray()
        self.next_request = 1
        self.was_cut = False

    def line(self):
        deadline = time.monotonic() + 10
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0, "timed out reading from SSH"
            ready, _, _ = select.select([self.child.stdout], [], [], remaining)
            assert ready, "timed out reading from SSH"
            data = os.read(self.child.stdout.fileno(), 65536)
            assert data, f"SSH relay closed before reply, status={self.child.poll()}"
            self.buffer.extend(data)
            assert len(self.buffer) <= MAX_RECORD, "oversized reply"
        content, _, rest = self.buffer.partition(b"\n")
        self.buffer = rest
        return content

    def rpc(self, kind, **fields):
        sequence = self.next_request
        self.next_request += 1
        request = dict(protocol=1, request=sequence, command=dict(kind=kind, **fields))
        self.child.stdin.write(json.dumps(request).encode() + b"\n")
        self.child.stdin.flush()
        line = self.line()
        reply = json.loads(line)
        assert (reply["protocol"], reply["request"]) == (1, sequence)
        return reply["result"], len(line) + 1

    def call(self, kind, **fields):
        result, _ = self.rpc(kind, **fields)
        assert list(result) == ["Ok"], result
        return result["Ok"]

    def reject(self, kind, reason, **fields):
        result, _ = self.rpc(kind, **fields)
        assert result == {"Err": reason}, result

    def cut(self):
        assert self.child.poll() is None, "transport already exited before the fault"
        self.child.kill()
        assert self.child.wait(5) < 0
        self.was_cut = True

    def close(self):
        self.child.stdin.close()
        try:
            status = self.child.wait(7)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait(5)
            raise AssertionError("SSH test child did not terminate cleanly")
        finally:
            self.child.stdout.close()
        assert status == 0 or self.was_cut, f"SSH test child exited with {status}"


class Grid:
    def __init__(self, owner):
        self.owner = owner
        self.frame = None

    def apply(self, update):
        frame = update["frame"]
        assert frame["version"] == 3
        assert frame["session"] == self.owner["session"]
        assert frame["epoch"] == self.owner["epoch"]
        if update["kind"] == "snapshot":
            assert len(frame["rows"]) <= 128 and 1 <= frame["cols"] <= 256
            assert all(len(row) == frame["cols"] for row in frame["rows"])
            self.frame = frame
        else:
            assert update["kind"] == "delta"
            assert frame["base_revision"] == self.frame["revision"]
            assert frame["revision"] > frame["base_revision"]
            assert frame["layout"] == self.frame["layout"]
            assert 0 < len(frame["rows"]) < len(self.frame["rows"])
            patched = set()
            for row in frame["rows"]:
                i = row["index"]
                assert 0 <= i < len(self.frame["rows"]) and i not in patched
                assert len(row["cells"]) == self.frame["cols"]
                self.frame["rows"][i] = row["cells"]
                patched.add(i)
            self.frame["revision"] = frame["revision"]
        return frame["revision"]

    def contains(self, row, marker):
        return "".join(cell["text"] for cell in self.frame["rows"][row]).strip() == marker


def write_line(channel, owner, command):
    data = command.encode() + b"\n"
    assert channel.call("write", owner=owner, bytes=list(data)) == len(data)


def wait_for_row(channel, grid, row, marker, *, acknowledge=True):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        result, size = channel.rpc("poll", owner=grid.owner)
        assert list(result) == ["Ok"], result
        update = result["Ok"]
        if update is not None:
            revision = grid.apply(update)
            arrived = grid.contains(row, marker)
            if not arrived or acknowledge:
                channel.call("ack", owner=grid.owner, revision=revision)
            if arrived:
                return update["kind"], revision, size
        time.sleep(0.025)
    visible = ["".join(cell["text"] for cell in cells).rstrip() for cells in grid.frame["rows"]]
    raise AssertionError(f"missing terminal row {row}: {marker}, observed {visible!r}")


def remote_python(destination, source, *arguments):
    return subprocess.check_output(
        [*SSH, "--", destination, shlex.join(["python3", "-c", source, *arguments])],
        timeout=12, text=True,
    )


def exercise(destination, binary):
    with contextlib.ExitStack() as clean:
        def open_channel(command):
            channel = Channel(destination, command)
            clean.callback(channel.close)
            return channel

        supervisor = open_channel(["python3", "-c", SUPERVISE, binary])
        socket = supervisor.line().decode()
        endpoint = pathlib.PurePosixPath(socket)
        root = endpoint.parent
        assert endpoint.is_absolute() and root.name.startswith("aether-ssh-")
        assert endpoint.name == "terminal.sock"
        first = open_channel([binary, "stdio", socket])
        original = first.call("open", rows=16, cols=80)
        assert len(original["session"]) == 32 and original["generation"] == 1
        grid = Grid(original)
        initial, snapshot_size = first.rpc("poll", owner=original)
        assert initial["Ok"]["kind"] == "snapshot"
        first.call("ack", owner=original, revision=grid.apply(initial["Ok"]))
        write_line(first, original, "stty -echo; printf '\\033[2;1H\\033[2K%s\\r\\n' ECHO_DISABLED")
        wait_for_row(first, grid, 1, "ECHO_DISABLED")
        quote = shlex.quote
        write_line(
            first, original,
            "SENTINEL=PTY_SURVIVED_SSH; "
            f"printf '%s' \"$$\" > {quote(str(root / 'pty-pid'))}; "
            "printf '\\033[3;1H\\033[2K%s\\r\\n' INITIAL_DELTA; "
            f"(n=0; while [ ! -f {quote(str(root / 'proceed'))} ] && [ \"$n\" -lt 100 ]; "
            "do n=$((n+1)); sleep 0.05; done; "
            f"if [ -f {quote(str(root / 'proceed'))} ]; then "
            "awk 'BEGIN { for (i=0; i<8192; i++) printf \"%c[12;1H%64s\", 27, \"\" }' || exit; "
            "printf '\\033[7;1H\\033[2K%s\\r\\n' DETACHED_OUTPUT; "
            f"printf done > {quote(str(root / 'written'))}; fi) &",
        )
        kind, unacknowledged, delta_size = wait_for_row(
            first, grid, 2, "INITIAL_DELTA", acknowledge=False,
        )
        assert kind == "delta" and delta_size < snapshot_size
        first.cut()
        pid, start = json.loads(remote_python(destination, RELEASE_DETACHED_OUTPUT, str(root)))

        second = open_channel([binary, "stdio", socket])
        pending = second.call("recover", owner=original)
        second.cut()  # Lose the connection again before applying or acknowledging recovery.
        third = open_channel([binary, "stdio", socket])
        recovered = third.call("recover", owner=original)
        assert recovered == pending
        current = recovered["owner"]
        assert current["session"] == original["session"] and current["epoch"] == original["epoch"]
        assert current["generation"] == original["generation"] + 1
        third.reject("ack", "stale terminal attachment", owner=original, revision=unacknowledged)
        third.reject("write", "stale terminal attachment", owner=original, bytes=[88])
        third.reject("close", "stale terminal attachment", owner=original)
        grid = Grid(current)
        recovered_frame = grid.apply(dict(kind="snapshot", frame=recovered["frame"]))
        assert grid.contains(2, "INITIAL_DELTA")
        third.call("ack", owner=current, revision=recovered_frame)
        if not grid.contains(6, "DETACHED_OUTPUT"):
            wait_for_row(third, grid, 6, "DETACHED_OUTPUT")
        write_line(third, current, "printf '\\033[9;1H\\033[2K%s\\r\\n' \"$SENTINEL\"")
        wait_for_row(third, grid, 8, "PTY_SURVIVED_SSH")

        third.call("detach", owner=current)
        third.cut()
        fourth = open_channel([binary, "stdio", socket])
        view = fourth.call("attach_read_only", session=current["session"], epoch=current["epoch"])
        readonly = view["owner"]
        assert readonly["generation"] > current["generation"]
        grid = Grid(readonly)
        revision = grid.apply(dict(kind="snapshot", frame=view["frame"]))
        assert all(grid.contains(row, marker) for row, marker in [
            (2, "INITIAL_DELTA"), (6, "DETACHED_OUTPUT"), (8, "PTY_SURVIVED_SSH"),
        ])
        fourth.call("ack", owner=readonly, revision=revision)
        fourth.reject("write", "reconnected terminal view is read-only", owner=readonly, bytes=[88])
        fourth.call("close", owner=readonly)
        remote_python(destination, CONFIRM_REAPED, str(pid), start)
        return dict(
            ssh_transports=4, initial_snapshot_bytes=snapshot_size, lost_delta_bytes=delta_size,
            same_shell_after_network_cut=True, detached_output_recovered=True,
            detached_output_bytes=8192 * (len("\x1b[12;1H") + 64),
            recovery_snapshot_retried=True, stale_ack_write_and_close_rejected=True,
            new_attachment_read_only=True, owned_pty_reaped=True,
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", help="SSH destination with an existing verified host key")
    parser.add_argument("remote_binary", help="absolute path to the remote aether-terminal-host")
    args = parser.parse_args()
    assert args.remote_binary.startswith("/"), "expected an absolute remote binary path"
    print(json.dumps(exercise(args.destination, args.remote_binary), indent=2))
