#!/usr/bin/env python3
"""Drive the exact Aether desktop binary and two owned local PTYs in WebKitGTK."""
import argparse
import contextlib
import errno
import fcntl
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import pty
from pathlib import Path
import re
import shlex
import shutil
import signal
import socket
import sqlite3
import struct
import subprocess
import tempfile
import termios
import threading
import time
from urllib.parse import quote

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"
READ = """
const canvas = document.querySelector('main canvas');
const ctx = canvas.getContext('2d');
const peer = document.querySelector('#peer-canvas');
const peerScale = peer.width / (parseFloat(peer.style.width) || peer.width);
const history = document.querySelector('#history-output');
const scale = canvas.width / (parseFloat(canvas.style.width) || canvas.width);
return {
  status: document.querySelector('#status').textContent,
  health: document.querySelector('#health').textContent,
  transport: document.querySelector('#health').dataset.transport || '',
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
  tabs: Array.from(document.querySelectorAll('#local-tab-list button'), tab => tab.dataset.epoch),
  available_tabs: Array.from(document.querySelectorAll('#local-tab-list button:enabled'), tab => tab.dataset.epoch),
  current_tab: document.querySelector('#local-tab-list [aria-current]')?.dataset.epoch || null,
  split: document.querySelector('#terminal-panes').dataset.split || '',
  peer_geometry: peer.dataset.geometry || '',
  peer_pixel: Array.from(peer.getContext('2d').getImageData(2,2,1,1).data),
  peer_fifth_row_pixel: Array.from(peer.getContext('2d').getImageData(2,Math.floor(96 * peerScale) + 2,1,1).data),
  active_x: document.querySelector('#active-pane').getBoundingClientRect().left,
  peer_x: document.querySelector('#peer-pane').getBoundingClientRect().left,
  focused: document.activeElement?.id,
  restore_layout: !document.querySelector('#saved-local-layout').hidden &&
                  !document.querySelector('#restore-local-layout').disabled,
  dump: history.value,
  find_status: document.querySelector('#history-find-status').textContent,
  selected_history: history.value.slice(history.selectionStart, history.selectionEnd),
  selection_context: history.value.slice(Math.max(0, history.selectionStart - 6), history.selectionStart),
  pixel: Array.from(ctx.getImageData(2, 2, 1, 1).data),
  fifth_row_pixel: Array.from(ctx.getImageData(2, Math.floor(96 * scale) + 2, 1, 1).data)
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

    def clear(self, selector):
        self.command("POST", self.element_path(selector) + "/clear", {})

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
        const open = document.querySelector('#open');
        return document.readyState === 'complete'
            && location.protocol === 'tauri:' && location.hostname === 'localhost'
            && Boolean(globalThis.__TAURI__?.core?.invoke) && Boolean(open && !open.disabled);
    """)


def shell_identity(sidecar):
    pid = int(sidecar.read_text())
    if pid <= 1:
        raise ValueError("bad owned shell PID")
    return pid, birth(pid)


def birth(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def write_from_current_tab(driver, pid_file, color=None):
    command = f"printf '%s' \"$$\" > {shlex.quote(str(pid_file))}"
    if color:
        r, g, b, _ = color
        command = (f"stty -echo; printf '\\033[2J\\033[H\\033[48;2;{r};{g};{b}m%20s\\033[0m\\r\\n' ''; "
                   + command)
    driver.clear("#input")
    driver.type("#input", command)
    driver.click("#send")
    wait_for(lambda: pid_file.is_file() and pid_file.stat().st_size > 0, "writable tab did not accept input")
    return shell_identity(pid_file)


def click_tab(driver, number):
    assert re.fullmatch(r"[1-9][0-9]{0,9}", number)
    selector = f'#local-tab-list [data-epoch="{number}"]'
    wait_messages = {
        "Wait for the current input request to finish before switching shells.",
        "Wait for the current shell input before switching tabs.",
    }
    last_click = None
    attempted_status = None
    used_keyboard = False
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        state = driver.execute(READ)
        if state["current_tab"] == number and state["writable"]:
            return
        retry_requested = state["status"] in wait_messages and time.monotonic() - (last_click or 0) >= .25
        if (number in state["available_tabs"] and state["current_tab"] != number
                and (last_click is None or retry_requested)):
            try:
                driver.click(selector)
                last_click = time.monotonic()
                attempted_status = state["status"]
            except ProtocolError as error:
                if error.error != "stale element reference":
                    raise
        elif (number in state["available_tabs"] and state["current_tab"] != number
              and not used_keyboard and last_click is not None
              and state["status"] == attempted_status and time.monotonic() - last_click >= .5):
            # A native click can be lost while the notebook is remapped by
            # WebKit. Exercise the same button by keyboard when its click
            # produced no transition or refusal in the page.
            try:
                driver.type(selector, "\ue007")
                used_keyboard = True
            except ProtocolError as error:
                if error.error != "stale element reference":
                    raise
        time.sleep(.08)
    raise AssertionError(f"Shell {number} did not regain input: active={state['current_tab']}, "
                         f"writable={state['writable']}, status={state['status']!r}")


CLOSE_EVIDENCE_START = """
window.__aetherCloseEvidence?.();
delete window.__aetherCloseEvidence;
const close = document.querySelector('#close');
const detach = document.querySelector('#detach');
const status = document.querySelector('#status');
const events = [], transitions = [], phases = [];
let clipped = false;
function phase() {
  const text = status.textContent;
  if (text.startsWith('Terminal closed; direct child reaped.')) return 'closed_reaped';
  if (text.startsWith('Terminal closed; direct-child cleanup')) return 'closed_cleanup_pending';
  if (text.startsWith('Close error:')) return 'close_error';
  if (text.startsWith('Local shell ')) return 'local_shell';
  if (text.startsWith('Reconnected to the existing shell')) return 'reattached';
  if (text.startsWith('Terminal output') || text.startsWith('Local output stream paused:')) return 'output_status';
  return text ? 'other' : 'empty';
}
function snapshot() {
  const focus = document.activeElement;
  return {phase:phase(), close_disabled:close.disabled, detach_disabled:detach.disabled,
    focus:focus===close?'close':focus===detach?'detach':focus===document.body?'body':'other',
    selected:!!document.querySelector('#local-tab-list [aria-current]')};
}
const initial = snapshot();
function append(list,value) {
  if (list.length < 16) list.push(value);
  else clipped = true;
}
function note(event) {
  const route = event.composedPath();
  append(events, {type:event.type, trusted:event.isTrusted, on_close:route.includes(close),
    on_detach:route.includes(detach), close_disabled:close.disabled, detach_disabled:detach.disabled});
}
const kinds = ['keydown', 'keyup', 'click'];
for (const kind of kinds) document.addEventListener(kind, note, true);
function observe(records) {
  // oldValue records every transition, even if a refusal re-enables both
  // controls before the observer callback runs. Keep only bounded evidence.
  const states = {close:close.disabled, detach:detach.disabled};
  const start = Math.max(0, records.length - 32), changes = [];
  if (start) clipped = true;
  for (let i = records.length - 1; i >= start; i--) {
    const record = records[i];
    if (record.type !== 'attributes') continue;
    const control = record.target === close ? 'close' : 'detach';
    const before = record.oldValue !== null;
    if (before !== states[control]) changes.push({control, before_disabled:before, after_disabled:states[control]});
    states[control] = before;
  }
  for (const value of changes.reverse()) append(transitions,value);
  const current = phase();
  if (current !== (phases.at(-1) ?? initial.phase)) append(phases,current);
}
const observer = new MutationObserver(observe);
observer.observe(status, {childList:true, characterData:true, subtree:true});
for (const button of [close,detach])
  observer.observe(button, {attributes:true, attributeFilter:['disabled'], attributeOldValue:true});
window.__aetherCloseEvidence = () => {
  observe(observer.takeRecords());
  for (const kind of kinds) document.removeEventListener(kind, note, true);
  observer.disconnect();
  return {initial, final:snapshot(), events, transitions, phases, clipped};
};
"""


CLOSE_EVIDENCE_STOP = """
const finish = window.__aetherCloseEvidence;
delete window.__aetherCloseEvidence;
if (finish) finish();
"""
CLOSE_EVIDENCE_READ = CLOSE_EVIDENCE_STOP.replace("if (finish) finish();", "return finish?.() ?? null;")


def optional_close_evidence(driver, script):
    # Collect diagnostics without changing the native activation or its wait.
    with contextlib.suppress(Exception):
        if driver.session is not None:
            return driver.request("POST", f"/session/{driver.session}/execute/sync",
                                  {"script":script, "args":[]}, timeout=1)
    return None


def known_owned_shell(identity):
    child, original_birth = identity
    if original_birth is not None:
        with contextlib.suppress(Exception):
            return birth(child) == original_birth
    return None


def close_without_stale_presentation(driver, state, selected, peer, reap_label):
    pid, expected_birth = selected
    before = {"selected":known_owned_shell(selected), "peer":known_owned_shell(peer)}

    wait_for(lambda: driver.execute("return !document.querySelector('#close').disabled;"),
             "Close did not become available")
    optional_close_evidence(driver, CLOSE_EVIDENCE_START)
    try:
        # Activate the focused button through WebKitGTK. Viewport coordinates
        # can address a different control after the split changes scroll.
        driver.type("#close", "\ue007")
        wait_for(lambda: state()["status"].startswith("Terminal closed"), "selected shell did not close")
        closed = state()
        assert closed["pixel"] == [0, 0, 0, 0], "a closed shell left pixels in the WebKit canvas"
        assert closed["cursor"] == closed["health"] == closed["dump"] == "", "closed shell status or text remains visible"
        wait_for(lambda: birth(pid) != expected_birth, reap_label)
    except Exception:
        after = {"selected":known_owned_shell(selected), "peer":known_owned_shell(peer)}
        gesture = optional_close_evidence(driver, CLOSE_EVIDENCE_READ)
        with contextlib.suppress(Exception):
            print("Native Close failure:", json.dumps({
                "known_owned_before":before, "known_owned_after":after, "gesture":gesture,
            }), flush=True)
        raise
    finally:
        # A timed-out arm or capture may nevertheless have run in the page.
        # Idempotent cleanup retrieves no diagnostic data on success.
        optional_close_evidence(driver, CLOSE_EVIDENCE_STOP)


def exercise(driver, owned):
    def state():
        return driver.execute(READ)

    def marker_on_screen(marker, absent, color):
        def sample():
            s = state()
            return (s["transport"] == "channel" and s["dump"].splitlines().count(marker) == 1
                    and absent not in s["dump"] and s["pixel"] == color)
        def snapshot():
            driver.click("#copy-visible")
            return sample()
        try:
            return wait_for(snapshot, f"native grid/canvas did not isolate {marker}")
        except AssertionError as error:
            raise AssertionError(f"{error}\n{json.dumps(state(), ensure_ascii=False)}") from error

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

    def search_archive(query):
        driver.clear("#history-find-query")
        driver.type("#history-find-query", query)
        driver.click("#history-find")

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
    archive_query = "AETHER_ARCHIVE_NEEDLE"
    archive_command = ("stty -echo; printf 'older %s%s\\r\\n' AETHER_ARCHIVE_ NEEDLE; seq 1 90; "
                       "printf 'newer %s%s\\r\\n' AETHER_ARCHIVE_ NEEDLE; seq 91 124; "
                       "printf 'TERM profile: %s %s\\r\\n' \"$TERM\" \"$(tput colors)\"; "
                       "printf '%s%s\\r\\n' AETHER_HISTORY_ READY")
    driver.type("#input", archive_command)
    driver.click("#send")

    def archived():
        driver.click("#copy-visible")
        rows = state()["dump"].splitlines()
        return "AETHER_HISTORY_READY" in rows and "TERM profile: jterm-256color 256" in rows

    wait_for(archived, "the real PTY did not publish its terminfo profile and retained output")
    driver.click("#history-refresh")
    wait_for(lambda: state()["dump"].count(archive_query) == 2,
             "native history reader did not show scrolled PTY output")
    search_archive(archive_query)
    wait_for(lambda: state()["selected_history"] == archive_query and state()["selection_context"] == "newer ",
             "native find did not select the newest retained match")
    driver.type("#input", "printf 'later %s%s\\r\\n' AETHER_ARCHIVE_ NEEDLE; seq 1 52; "
                "printf '\\033[H\\033[48;2;17;33;49m \\033[0m'")
    driver.click("#send")
    wait_for(lambda: state()["pixel"] == [17, 33, 49, 255], "live output did not advance the real archive")
    driver.click("#history-find-older")
    wait_for(lambda: state()["selected_history"] == archive_query and state()["selection_context"] == "older ",
             "native find did not cross the archive page boundary")
    a, b = "AETHER_NATIVE_LOCAL_A", "AETHER_NATIVE_LOCAL_B"
    red, blue = [206, 48, 70, 255], [31, 96, 208, 255]
    a_pid_path, b_pid_path = owned / "a.pid", owned / "b.pid"
    forbidden_a, forbidden_b = owned / "forbidden-a", owned / "forbidden-b"
    draft_a, draft_b = (f"printf cross > {shlex.quote(str(path))}" for path in (forbidden_a, forbidden_b))
    launch("A", red, a, a_pid_path)
    marker_on_screen(a, b, red)
    a_pid, a_birth = shell_identity(a_pid_path)
    assert a_birth is not None
    driver.click("#recover")
    wait_for(lambda: state()["status"].startswith("Output snapshot recovered"), "local channel snapshot recovery did not complete")
    assert birth(a_pid) == a_birth, "channel recovery replaced the owned shell"
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first command adoption not settled")
    old_pixel = state()["fifth_row_pixel"]
    green = [38, 152, 87, 255]
    driver.type("#input", "printf '\\033[5;1H\\033[48;2;38;152;87m%20s\\033[0m\\033[3;1H' ''")
    driver.click("#send")
    assert old_pixel != green, "single-row update fixture already painted"
    wait_for(lambda: state()["fifth_row_pixel"] == green, "single-row PTY color did not reach Canvas")
    marker_on_screen(a, b, red)
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "single-row edit adoption not settled")
    driver.type("#input", ("printf '\\033[7;1H\\033[38;2;193;56;219;4:2;58:2::100:223:71m%080d"
                           "\\033[0m%s\\033[9;1H' 0 'AETHER_V4_WRAP_尾'"))
    driver.click("#send")

    def wrapped_rendition(color):
        driver.click("#copy-visible")
        if "0" * 80 + "AETHER_V4_WRAP_尾" not in state()["dump"]:
            return False
        painted = driver.execute("""
            const canvas = document.querySelector('main canvas');
            const scale = canvas.width / parseFloat(canvas.style.width);
            const ctx = canvas.getContext('2d');
            const sample = row => Array.from(ctx.getImageData(Math.floor(5.5 * scale),
                Math.floor((6 * 24 + row + 0.5) * scale), 1, 1).data);
            return [sample(21), sample(22), sample(23)];
        """)
        return painted[0] == painted[2] == color and painted[1] != color

    wait_for(lambda: wrapped_rendition([100, 223, 71, 255]),
             "v4 wrapped wide text and truecolor double underline did not reach native WebKit")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "styled wrap edit adoption not settled")
    driver.type("#input", ("printf '\\033[7;1H\\033[38;2;193;56;219;4:2;58:5:42m%080d"
                           "\\033[0m\\033[9;1H' 0"))
    driver.click("#send")
    wait_for(lambda: wrapped_rendition([0, 215, 135, 255]),
             "v4 indexed underline update did not repaint the unchanged text and span")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "underline update adoption not settled")
    driver.type("#input", ("printf '\\033[11;1He'; sleep 0.12; printf '\\314\\201 \\360\\237\\221\\251'; "
                           "sleep 0.12; printf '\\342\\200\\215'; sleep 0.12; "
                           "printf '\\360\\237\\222\\273!\\033[9;1H'"))
    driver.click("#send")

    def composed_cells():
        driver.click("#copy-visible")
        return "e\u0301 👩‍💻!" in state()["dump"]

    wait_for(composed_cells, "fragmented combining text and a wide joined emoji did not survive v4 copy")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "cluster edit adoption not settled")
    driver.type("#input", ("printf '\\033[12;1H.GH!Z\\033[12;2H\\342\\231\\245'; "
                           "sleep 0.12; printf '\\357\\270\\217\\033[13;1H.AB!Y"
                           "\\033[13;2H\\342\\214\\232'; sleep 0.12; "
                           "printf '\\357\\270\\216\\033[9;1H'"))
    driver.click("#send")

    def late_width_cells():
        driver.click("#copy-visible")
        output = state()["dump"]
        return ".♥️!Z" in output and ".⌚︎B!Y" in output

    wait_for(late_width_cells, "v4 did not copy the revised VS16 and VS15 cell widths")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "variation selector edit adoption not settled")
    driver.type("#input", draft_a)
    detach()

    first = list_shells(1)
    assert first["second"], "second open was not expressly available"
    driver.click("#open-second-local")
    wait_for(lambda: state()["status"].startswith("Second local shell opened"), "explicit second local shell did not open")
    assert state()["writable"] and state()["draft"] == ""
    launch("B", blue, b, b_pid_path)
    marker_on_screen(b, a, blue)
    search_archive(archive_query)
    wait_for(lambda: state()["find_status"] == "No matches in retained history.", "history search leaked the other shell")
    assert state()["selected_history"] != archive_query
    b_pid, b_birth = shell_identity(b_pid_path)
    assert b_birth is not None and b_pid != a_pid
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second command adoption not settled")
    driver.type("#input", draft_b)

    choices = list_shells(2)
    assert choices["writable"], "opening the roster revoked the current input lease"
    assert not choices["choose"] and not choices["second"] and choices["selected"] == "", "ambiguous choice was implicitly authorized"
    a_label, b_label = sorted(choices["options"], key=int)
    assert a_label == first["options"][0] and a_label != b_label
    select(a_label)
    marker_on_screen(a, b, red)
    current = state()
    assert current["fifth_row_pixel"] == green, "reattaching lost the edited row"
    assert birth(a_pid) == a_birth
    assert not current["writable"] and not current["resize"] and current["draft"] == draft_a
    try:
        driver.click("#send")
    except ProtocolError as err:
        if err.error not in {"element not interactable", "invalid element state"}:
            raise
    choices = list_shells(2)
    assert not choices["choose"] and choices["selected"] == ""
    select(b_label)
    marker_on_screen(b, a, blue)
    current = state()
    assert current["fifth_row_pixel"] != green, "a dirty row leaked into the other shell"
    assert birth(b_pid) == b_birth
    assert not current["writable"] and not current["resize"] and current["draft"] == draft_b
    assert not forbidden_a.exists() and not forbidden_b.exists(), "a parked draft executed"
    close_without_stale_presentation(driver, state, (b_pid, b_birth), (a_pid, a_birth), "second owned shell not reaped")
    select(list_shells(1)["options"][0])
    marker_on_screen(a, b, red)
    assert state()["draft"] == draft_a and birth(a_pid) == a_birth
    close_without_stale_presentation(driver, state, (a_pid, a_birth), (b_pid, b_birth), "first owned shell not reaped")
    list_shells(0)
    assert not forbidden_a.exists() and not forbidden_b.exists(), "a parked draft executed"

    driver.click("#open")
    wait_for(lambda: state()["writable"] and len(state()["tabs"]) == 1, "initial shell tab did not open")
    a_tab = state()["current_tab"]
    a_identity = write_from_current_tab(driver, owned / "tab-a.pid", red)
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first tab input still pending")
    split_gate = owned / "advance-inactive-pane"
    gate = shlex.quote(str(split_gate))
    inactive_command = (f"(n=0; while [ \"$n\" -lt 200 ] && [ ! -e {gate} ]; do "
                        f"n=$((n+1)); sleep .05; done; if [ -e {gate} ]; then "
                        "printf '\\033[5;1H\\033[48;2;38;152;87m%20s\\033[0m\\033[3;1H' ''; fi) &")
    driver.type("#input", inactive_command)
    driver.click("#send")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "inactive output job was not admitted")
    driver.type("#input", draft_a)
    driver.click("#new-local-tab")
    wait_for(lambda: state()["writable"] and len(state()["tabs"]) == 2 and state()["current_tab"] != a_tab,
             "new shell tab did not get its own lease")
    b_tab = state()["current_tab"]
    assert state()["draft"] == ""
    b_identity = write_from_current_tab(driver, owned / "tab-b.pid", blue)
    assert b_identity[0] != a_identity[0]
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second tab input still pending")
    driver.type("#input", draft_b)

    driver.click("#split-local")

    def split_with_b_active():
        s = state()
        return s["split"] == "right" and s["peer_x"] < s["active_x"] and s["pixel"] == blue and s["peer_pixel"] == red

    wait_for(split_with_b_active, "two native panes did not show their separate canvases")
    assert state()["peer_fifth_row_pixel"] != green
    split_gate.touch()
    wait_for(lambda: state()["peer_fifth_row_pixel"] == green, "inactive pane did not present new PTY output")
    assert state()["pixel"] == blue and state()["fifth_row_pixel"] != green
    assert state()["peer_geometry"] == "80x24"
    for selector, value in [("#cols", "84"), ("#rows", "12")]:
        driver.clear(selector)
        driver.type(selector, value)
    driver.click("#resize")
    wait_for(lambda: state()["cursor"].startswith("84 × 12"), "split resize never reached the active PTY")
    assert state()["peer_geometry"] == "80x24"
    driver.click("#focus-peer-pane")

    def split_with_a_active():
        s = state()
        return (s["current_tab"] == a_tab and s["focused"] == "ime-input"
                and s["split"] == "left" and s["active_x"] < s["peer_x"] and s["peer_geometry"] == "84x12")

    wait_for(split_with_a_active, "clicking the inactive pane did not transfer input and focus")
    assert state()["cursor"].startswith("80 × 24")
    assert state()["resize"] and state()["draft"] == draft_a
    assert write_from_current_tab(driver, owned / "tab-a-return.pid") == a_identity
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first tab return input still pending")
    click_tab(driver, b_tab)
    assert state()["draft"] == draft_b
    assert write_from_current_tab(driver, owned / "tab-b-return.pid") == b_identity
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second tab return input still pending")
    close_without_stale_presentation(driver, state, b_identity, a_identity, "closed writable tab was not reaped")
    click_tab(driver, a_tab)
    assert state()["tabs"] == [a_tab]
    assert write_from_current_tab(driver, owned / "tab-a-after-close.pid") == a_identity
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "surviving tab input still pending")
    close_without_stale_presentation(driver, state, a_identity, b_identity, "surviving owned tab was not reaped")
    assert not forbidden_a.exists() and not forbidden_b.exists(), "a writable tab replayed another draft"
    print(json.dumps({"native_webkitgtk": True, "owned_local_ptys": 2,
                      "native_pty_terminfo_profile": True,
                      "local_frames_delivered_by_tauri_channel": True,
                      "local_channel_recovers_same_pty": True,
                      "distinct_canvas_colors_and_markers": True, "explicit_view_only_reattach": True,
                      "direct_view_switch_from_live_roster": True,
                      "writable_tabs_return_to_same_owned_shell": True,
                      "writable_surviving_tab_after_close": True,
                      "split_pane_focus_and_resize_isolated": True,
                      "inactive_split_pane_streams_new_output": True,
                      "native_history_find_pages_and_session_isolation": True,
                      "native_history_find_keeps_its_source_while_output_appends": True,
                      "native_history_reads_archived_pty_output": True,
                      "native_v4_soft_wrap_wide_copy_and_double_underline": True,
                      "native_v4_truecolor_and_indexed_underline": True,
                      "native_v4_fragmented_combining_and_wide_cluster": True,
                      "native_v4_late_variation_selector_growth_and_contraction": True,
                      "single_row_canvas_update_and_reattach": True,
                      "parked_drafts_isolated": True, "closed_canvas_and_status_cleared": True,
                      "owned_direct_children_reaped": True}), flush=True)


def prepare_hyperlink_handler(owned, env):
    applications = Path(env["XDG_DATA_HOME"]) / "applications"
    applications.mkdir(mode=0o700)
    handler = owned / "record-hyperlink"
    handler.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$HOME/opened-hyperlinks\"\n")
    handler.chmod(0o700)
    name = "aether-hyperlink-fixture.desktop"
    (applications / name).write_text(
        f"[Desktop Entry]\nType=Application\nName=Aether hyperlink fixture\nExec={handler} %u\n"
        "MimeType=x-scheme-handler/https;\nNoDisplay=true\n")
    (Path(env["XDG_CONFIG_HOME"]) / "mimeapps.list").write_text(
        f"[Default Applications]\nx-scheme-handler/https={name};\n")


def exercise_hyperlinks(driver, owned):
    target = "https://aether-hyperlink.test/visible-target"
    opened = owned / "opened-hyperlinks"
    driver.click("#open")
    wait_for(lambda: driver.execute(READ)["transport"] == "channel", "hyperlink fixture shell did not attach")
    driver.command("POST", "/window/rect", {"width": 1380, "height": 1000})
    driver.click("#reveal-cursor")
    point = driver.execute("""
        const canvas = document.querySelector('main canvas');
        const rect = canvas.getBoundingClientRect(), clip = canvas.parentElement.getBoundingClientRect();
        const y = (Math.max(0, rect.top, clip.top) + Math.min(innerHeight, rect.bottom, clip.bottom)) / 2;
        return {row:Math.floor((y - rect.top) * 24 / rect.height), col:10};
    """)
    assert 0 <= point["row"] < 24

    def write_link(uri, marker):
        driver.type("#input", (f"stty -echo; printf '\\033[2J\\033[{point['row']+1};{point['col']+1}H"
                               f"\\033]8;;%s\\007{marker}\\033]8;;\\007' {shlex.quote(uri)}"))
        driver.click("#send")
        def painted():
            driver.click("#copy-visible")
            return marker in driver.execute(READ)["dump"]
        wait_for(painted, "hyperlink fixture text did not paint")

    def hover(pressed=False):
        driver.click("#reveal-cursor")
        geometry = driver.execute(f"""
            const canvas = document.querySelector('main canvas'), rect = canvas.getBoundingClientRect();
            const x = Math.floor(rect.left + ({point['col']} + 0.5) * rect.width / 80);
            const y = Math.floor(rect.top + ({point['row']} + 0.5) * rect.height / 24);
            return document.elementFromPoint(x,y) === canvas ? {{x,y}} : null;
        """)
        assert geometry, "hyperlink fixture cell is clipped"
        move = {"type": "pointerMove", "origin": "viewport", "duration": 0, **geometry}
        pointer = {"type": "pointer", "id": "aether-link-pointer", "parameters": {"pointerType": "mouse"},
                   "actions": [move]}
        actions = [pointer]
        if pressed:
            pointer["actions"] += [{"type": "pointerDown", "button": 0}, {"type": "pointerUp", "button": 0}]
            actions.append({"type": "key", "id": "aether-link-keys", "actions": [
                {"type": "keyDown", "value": "\ue009"}, {"type": "pause", "duration": 0},
                {"type": "pause", "duration": 0}, {"type": "keyUp", "value": "\ue009"}]})
        driver.command("POST", "/actions", {"actions": actions})
        driver.command("DELETE", "/actions")

    write_link(target, "VALID_LINK")
    hover()
    assert driver.execute("return document.querySelector('main canvas').title;") == f"Ctrl-click to open {target}"
    assert not opened.exists(), "hover opened the URI without a click"
    hover(pressed=True)
    wait_for(lambda: opened.is_file() and opened.read_text().splitlines() == [target],
             "explicit hyperlink did not reach the native default handler")
    write_link("https://user@aether-hyperlink.test/rejected", "REFUSED_LINK")
    hover()
    assert driver.execute("return document.querySelector('main canvas').title;") == ""
    hover(pressed=True)
    assert opened.read_text().splitlines() == [target], "a rejected target reached the handler"
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)["status"].startswith("Terminal closed"), "hyperlink shell did not close")
    print(json.dumps({"native_v4_osc8_visible_and_opens_once": True,
                      "native_v4_osc8_credentials_fail_closed": True}), flush=True)


def click_visible_terminal(driver):
    point = wait_for(lambda: driver.execute("""
        const canvas = document.querySelector('main canvas');
        const rect = canvas.getBoundingClientRect(), clip = canvas.parentElement.getBoundingClientRect();
        const left = Math.max(0, rect.left, clip.left), right = Math.min(innerWidth, rect.right, clip.right);
        const top = Math.max(0, rect.top, clip.top), bottom = Math.min(innerHeight, rect.bottom, clip.bottom);
        if (left >= right || top >= bottom) return null;
        const x = Math.floor((left + right) / 2), y = Math.floor((top + bottom) / 2);
        return document.elementFromPoint(x, y) === canvas ? {x, y} : null;
    """), "terminal canvas has no visible click target")
    driver.command("POST", "/actions", {"actions": [{
        "type": "pointer", "id": "terminal-pointer", "parameters": {"pointerType": "mouse"},
        "actions": [
            {"type": "pointerMove", "origin": "viewport", "duration": 0, **point},
            {"type": "pointerDown", "button": 0},
            {"type": "pointerUp", "button": 0},
        ],
    }]})
    driver.command("DELETE", "/actions")


def exercise_inline_graphics(driver, owned):
    fixture = owned / "inline-graphics-fixture.py"
    fixture.write_text(r'''
import base64
import os
from pathlib import Path
import select
import struct
import sys
import termios
import time
import tty
import zlib

root = Path(sys.argv[1])
fd = 0
saved = termios.tcgetattr(fd)

def send(data):
    os.write(1, data)

def wait_for_file(name):
    until = time.monotonic() + 45
    while not (root / name).exists():
        if time.monotonic() >= until:
            raise RuntimeError(name)
        time.sleep(0.03)

def expect(reply):
    received = b''
    until = time.monotonic() + 5
    while len(received) < len(reply):
        ready, _, _ = select.select([fd], [], [], max(0, until - time.monotonic()))
        if not ready:
            break
        received += os.read(fd, 128)
    if received != reply:
        raise RuntimeError(repr((received, reply)))

def png_chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

try:
    tty.setraw(fd)
    send(b'\x1b[3J\x1b[2J\x1b[H\x1b[?25l\x1b_Ga=q,i=31,f=24,s=1,v=1;AAAA\x1b\\')
    expect(b'\x1b_Gi=31;OK\x1b\\')
    # Multiple solid source pixels on each side keep sampled centers exact
    # when WebKit interpolates at different fractional device pixel positions.
    rgba = base64.b64encode(bytes([240, 28, 80, 255] * 4 + [34, 193, 91, 255] * 4))
    send(b'\x1b[5;2H\x1b_Ga=T,i=41,c=2,r=2,f=32,s=1,v=8,C=1,m=1;' + rgba[:4] + b'\x1b\\')
    send(b'\x1b_Gm=0;' + rgba[4:] + b'\x1b\\\x1b[6n')
    expect(b'\x1b_Gi=41;OK\x1b\\\x1b[5;2R')
    png = (b'\x89PNG\r\n\x1a\n' + png_chunk(b'IHDR', struct.pack('>2I5B', 1, 1, 8, 6, 0, 0, 0))
           + png_chunk(b'IDAT', zlib.compress(bytes([0, 31, 111, 243, 255]))) + png_chunk(b'IEND', b''))
    send(b'\x1b[5;5H\x1b_Ga=T,i=42,c=2,r=1,f=100,C=1;' + base64.b64encode(png) + b'\x1b\\')
    expect(b'\x1b_Gi=42;OK\x1b\\')
    (root / 'inline-ready').write_text(str(os.getpid()))
    wait_for_file('inline-scroll')
    send(b'\x1b[1S')
    wait_for_file('inline-erase')
    send(b'\x1b[2J\x1b[3J')
    (root / 'inline-erased').touch()
    wait_for_file('inline-finish')
except Exception as error:
    (root / 'inline-error').write_text(str(error))
    raise
finally:
    termios.tcsetattr(fd, termios.TCSANOW, saved)
    send(b'\x1b[?25h')
''')
    error = owned / "inline-error"
    red, green, blue = [240, 28, 80, 255], [34, 193, 91, 255], [31, 111, 243, 255]
    driver.click("#open")
    original = wait_for(lambda: (state := driver.execute(READ))["writable"] and state["transport"] == "channel" and state,
                        "graphics fixture did not attach to the real local Channel")
    driver.type("#input", f"python3 {shlex.quote(str(fixture))} {shlex.quote(str(owned))}")
    driver.click("#send")

    def pixels(row):
        if error.exists():
            raise AssertionError("PTY graphics fixture: " + error.read_text())
        return driver.execute(f"""
            const canvas = document.querySelector('main canvas');
            const g = canvas.getContext('2d');
            const scale = canvas.width / parseFloat(canvas.style.width);
            const sample = (x,y) => Array.from(g.getImageData(Math.floor(x*scale),Math.floor(y*scale),1,1).data);
            return [sample(22,{row}*24+12), sample(22,{row + 1}*24+12), sample(56,{row}*24+12)];
        """)

    ready = owned / "inline-ready"
    try:
        wait_for(lambda: pixels(4) == [red, green, blue] and ready.is_file() and ready.stat().st_size,
                 "real Kitty RGBA and PNG were not painted in WebKit")
    except AssertionError as failure:
        state = driver.execute(READ)
        raise AssertionError(f"{failure} (PTY ready={ready.exists()}, pixels={pixels(4)}, "
                             f"status={state['status']}, geometry={state['cursor']})") from failure
    child = int(ready.read_text())
    child_birth = birth(child)
    assert child_birth is not None
    (owned / "inline-scroll").touch()
    wait_for(lambda: pixels(3) == [red, green, blue] and pixels(4)[1] != green,
             "canvas row patches did not move both image resources with PTY scroll")

    driver.clear("#cols")
    driver.type("#cols", "76")
    driver.click("#resize")
    wait_for(lambda: "76 ×" in driver.execute(READ)["cursor"] and pixels(3) == [red, green, blue],
             "full snapshot after terminal resize lost the image resources")

    driver.click("#detach")
    wait_for(lambda: driver.execute("return !document.querySelector('#attach').disabled"),
             "graphics fixture did not detach")
    driver.click("#attach")
    wait_for(lambda: (view := driver.execute(READ))["current_tab"] == original["current_tab"] and
             view["transport"] == "channel" and not view["writable"] and pixels(3) == [red, green, blue],
             "read-only reconnect did not restore the graphics resources for the retained PTY")
    assert birth(child) == child_birth
    (owned / "inline-erase").touch()
    wait_for(lambda: (owned / "inline-erased").exists() and
             all(pixel not in (red, green, blue) for pixel in pixels(3)), "erase did not clear the painted slices")
    (owned / "inline-finish").touch()
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)["status"].startswith("Terminal closed"), "graphics fixture did not close")
    print(json.dumps({"native_inline_graphics_raw_and_png_pixels": True,
                      "native_inline_graphics_pty_reply_order": True,
                      "native_inline_graphics_scroll_resize_erase": True,
                      "native_inline_graphics_readonly_reconnect_same_pty": True,
                      "device_pixel_ratio": driver.execute("return devicePixelRatio")}), flush=True)


def exercise_history_graphics(driver, owned):
    fixture = owned / "history-graphics-fixture.py"
    fixture.write_text(r'''
import base64
import os
from pathlib import Path
import select
import sys
import termios
import time
import tty

root = Path(sys.argv[1])
original = termios.tcgetattr(0)
try:
    tty.setraw(0)
    image = base64.b64encode(bytes([240, 28, 80, 255] * 4 + [34, 193, 91, 255] * 4))
    os.write(1, b'\x1b[?25l\x1b[3J\x1b[2J\x1b[H' + b'x' * 80 + b'XY' +
        b'\x1b_Ga=T,i=51,f=32,s=1,v=8,c=2,r=2,C=1;' + image + b'\x1b\\')
    expected = b'\x1b_Gi=51;OK\x1b\\'
    received = b''
    stop = time.monotonic() + 5
    while len(received) < len(expected):
        ready, _, _ = select.select([0], [], [], max(0, stop - time.monotonic()))
        if not ready:
            break
        received += os.read(0, 128)
    if received != expected:
        raise RuntimeError(repr(received))
    os.write(1, b'\x1b[2;9H' + '🐍hist-image'.encode() + b'\x1b[24S')
    (root / 'history-raster-ready').write_text(str(os.getpid()))
    until = time.monotonic() + 45
    while not (root / 'history-raster-clear').exists():
        if time.monotonic() >= until:
            raise RuntimeError('waiting for history clear')
        time.sleep(.03)
    os.write(1, b'\x1b[3J\x1b[?25h')
    (root / 'history-raster-cleared').touch()
except Exception as error:
    (root / 'history-raster-error').write_text(str(error))
    raise
finally:
    termios.tcsetattr(0,termios.TCSANOW,original)
''')
    red, green = [240, 28, 80, 255], [34, 193, 91, 255]
    ready = owned / "history-raster-ready"
    error = owned / "history-raster-error"
    driver.click("#open")
    wait_for(lambda: (state := driver.execute(READ))["writable"] and state["transport"] == "channel",
             "retained graphics did not obtain a local Channel")
    driver.type("#input", f"python3 {shlex.quote(str(fixture))} {shlex.quote(str(owned))}")
    driver.click("#send")

    def archived_ready():
        if error.exists():
            raise AssertionError("graphics PTY: " + error.read_text())
        return ready.is_file() and ready.stat().st_size

    wait_for(archived_ready, "graphics did not reach the retained archive")

    def read(first_column):
        return driver.execute(f"""
            const canvas = document.querySelector('#history-canvas'), view = document.querySelector('#history-viewport');
            const output = document.querySelector('#history-output');
            if (view.hidden || canvas.width === 0 || !canvas.style.width) return null;
            const scale = canvas.width / parseFloat(canvas.style.width);
            const row = Number(canvas.dataset.firstRow);
            if (row > 1) return null;
            const sample = (x,y) => Array.from(canvas.getContext('2d').getImageData(
                Math.floor((12*x+6)*scale),Math.floor((24*(y-row)+12)*scale),1,1).data);
            return {{red:sample({first_column},1),green:sample(2,2),text:output.value,
                selected:output.value.slice(output.selectionStart,output.selectionEnd),
                width:parseFloat(canvas.style.width),tile:parseFloat(canvas.style.height),
                page:parseFloat(canvas.parentElement.style.height)}};
        """)

    driver.click("#history-refresh")
    first = wait_for(lambda: (page := read(2)) and page["red"] == red and page["green"] == green and page,
                     "the ordinary history projection did not paint both archived image slices")
    assert first["text"].startswith("x" * 80 + "XY") and "🐍hist-image" in first["text"]
    assert first["width"] == 80 * 12 and first["tile"] < first["page"]
    point = driver.execute("""
        const canvas = document.querySelector('#history-canvas'), geo = canvas.getBoundingClientRect();
        const x = Math.floor(geo.left + 9.5*12), y = Math.floor(geo.top + (1-Number(canvas.dataset.firstRow))*24 + 12);
        return document.elementFromPoint(x,y) === canvas ? {x,y} : null;
    """)
    assert point, "history viewport clipped the wide-character selection fixture"
    driver.command("POST", "/actions", {"actions": [{"id": "history-pointer", "type": "pointer",
        "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "origin": "viewport", "duration": 0, **point},
            {"type": "pointerDown", "button": 0}, {"type": "pointerUp", "button": 0}]}]})
    driver.command("DELETE", "/actions")
    wait_for(lambda: read(2)["selected"] == "🐍", "the wide continuation did not select its complete grapheme")

    driver.clear("#cols")
    driver.type("#cols", "52")
    driver.click("#resize")
    shifted = wait_for(lambda: (page := read(30)) and page["width"] == 52 * 12 and
                       page["red"] == red and page["green"] == green and page,
                       "the history image anchor did not follow logical reflow at the new width")
    assert shifted["text"] == first["text"] and shifted["selected"] == "🐍"

    driver.type("#history-find-query", "hist-image")
    driver.click("#history-find")
    hit = wait_for(lambda: (page := read(2)) and page["selected"] == "hist-image" and
                   page["red"] == red and page["green"] == green and page,
                   "the search match did not paint its original archived rows and select its exact text")
    assert hit["width"] == 80 * 12
    (owned / "history-raster-clear").touch()
    wait_for(lambda: (owned / "history-raster-cleared").exists() and driver.execute("""
        return document.querySelector('#history-viewport').hidden && document.querySelector('#history-output').value === '' &&
            document.querySelector('#history-label').textContent.startsWith('History source expired');
    """), "clearing the source did not invalidate the rendered page and selection")
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)["status"].startswith("Terminal closed"), "retained graphics fixture did not close")
    print(json.dumps({"native_history_retained_image_pixels": True,
                      "native_history_virtualized_canvas": True,
                      "native_history_projected_reflow_and_selection": True,
                      "native_history_search_uses_original_image_rows": True,
                      "native_history_clear_invalidates_selection": True,
                      "device_pixel_ratio":driver.execute("return devicePixelRatio")}), flush=True)


def exercise_height_shrink(driver):
    def state():
        return driver.execute(READ)

    driver.click("#open")
    original = wait_for(lambda: state() if state()["writable"] else None, "resize fixture has a local PTY")
    rows = int(driver.execute("return document.querySelector('#rows').value"))
    assert rows > 8
    driver.type("#input", r"stty -echo; printf '\033[3J\033[2J\033[H%s\033[%d;1H%s\033[%d;1H\0337%s\033[999;1H%s' "
                f"AETHER-RESIZE-ARCHIVE {rows - 4} AETHER-RESIZE-LIVE {rows - 3} "
                r"AETHER-RESIZE-DRAFT AETHER-RESIZE-END; IFS= read -r pause; "
                r"printf '\0338\033[J%s' AETHER-RESIZE-REPAINT; IFS= read -r hold")
    driver.click("#send")

    def visible():
        driver.click("#copy-visible")
        return state()["dump"]

    def resize(height):
        driver.clear("#rows")
        driver.type("#rows", str(height))
        driver.click("#resize")
        wait_for(lambda: f" × {height} " in state()["cursor"], "native WebKit accepted the resized viewport")

    def initial_witnesses():
        text = visible()
        return all(marker in text for marker in ("AETHER-RESIZE-ARCHIVE", "AETHER-RESIZE-LIVE",
                                                  "AETHER-RESIZE-DRAFT", "AETHER-RESIZE-END"))

    wait_for(initial_witnesses, "all resize witnesses were visible before the shrink")
    resize(8)
    assert state()["current_tab"] == original["current_tab"]
    live = visible()
    assert "AETHER-RESIZE-LIVE" in live and "AETHER-RESIZE-ARCHIVE" not in live
    driver.click("#history-refresh")
    wait_for(lambda: "AETHER-RESIZE-ARCHIVE" in state()["dump"], "displaced top row reached the retained history")
    driver.type("#input", "repaint")
    driver.click("#send")

    def repainted():
        text = visible()
        return text if "AETHER-RESIZE-REPAINT" in text else None

    live = wait_for(repainted, "PTY repainted from its saved cursor after the shrink")
    lines = [line.rstrip() for line in live.splitlines()]
    origin = lines.index("AETHER-RESIZE-LIVE") + 1
    assert lines[origin] == "AETHER-RESIZE-REPAINT", lines
    assert "AETHER-RESIZE-DRAFT" not in live and "AETHER-RESIZE-END" not in live
    resize(rows)
    driver.click("#close")
    wait_for(lambda: state()["status"].startswith("Terminal closed"), "resize fixture closed")
    print(json.dumps({"native_height_shrink_keeps_live_output_and_archives_displaced_rows": True,
                      "native_height_shrink_keeps_saved_prompt_origin": True}), flush=True)


def exercise_renderer_reload(driver, owned):
    def state():
        return driver.execute(READ)

    def painted(marker):
        driver.click("#copy-visible")
        return re.search(r"(?m)^" + re.escape(marker) + r" *$", state()["dump"])

    before = owned / "renderer-reload-before.pid"
    release = owned / "renderer-reload-release"
    after = owned / "renderer-reload-after.pid"
    forbidden = owned / "renderer-reload-unsent"
    driver.click("#open")
    original = wait_for(lambda: (view := state())["writable"] and view["current_tab"],
                        "reload fixture did not open a terminal")
    command = (f'printf "%s" "$$" > {shlex.quote(str(before))}; '
               "printf 'AETHER-RENDERER-BEFORE\\n'; "
               f'while test ! -f {shlex.quote(str(release))}; do sleep 0.05; done; '
               f'printf "%s" "$$" > {shlex.quote(str(after))}; '
               "printf 'AETHER-RENDERER-AFTER\\n'")
    driver.type("#input", command)
    driver.click("#send")
    wait_for(lambda: before.is_file() and painted("AETHER-RENDERER-BEFORE"),
             "the original renderer did not display its PTY output")
    driver.type("#input", f"printf replayed > {shlex.quote(str(forbidden))}")
    assert state()["draft"]

    driver.command("POST", "/refresh", {})
    wait_for(lambda: opened_page(driver) and state()["current_tab"] is None, "new WebKit page did not load")
    release.touch()
    wait_for(lambda: after.is_file() and after.read_text().isdigit(), "detached PTY did not advance during reload")
    assert after.read_text() == before.read_text()
    driver.click("#attach")
    wait_for(lambda: state()["current_tab"] == original and painted("AETHER-RENDERER-AFTER"),
             "new renderer did not reattach to the original PTY")
    observed = state()
    assert not observed["writable"] and not observed["resize"] and not observed["draft"]
    assert not forbidden.exists()
    assert re.search(r"(?m)^AETHER-RENDERER-BEFORE *$", observed["dump"])
    driver.click("#close")
    wait_for(lambda: state()["status"].startswith("Terminal closed"), "reload fixture did not close")
    print(json.dumps({"native_renderer_reload_keeps_same_pty": True,
                      "native_renderer_reload_recovers_detached_output_read_only": True}), flush=True)


def exercise_keyboard(driver, owned):
    probe = owned / "keyboard-probe.py"
    receipt = owned / "keyboard-bytes"
    probe.write_text('''import os, select, sys, termios, tty
mode = termios.tcgetattr(0)
try:
    tty.setraw(0)
    os.write(1, b"\\x1b[?1049h\\x1b[>31u\\x1b[?u\\x1b[2J\\x1b[HAETHER-KEYBOARD-READY")
    with open(sys.argv[1], "wb", buffering=0) as output:
        while select.select([0], [], [], 10)[0]:
            data = os.read(0, 256)
            output.write(data)
            if not data or data.endswith(b"q"):
                break
finally:
    os.write(1, b"\\x1b[?1049l\\r\\nAETHER-KEYBOARD-FINISHED\\r\\n")
    termios.tcsetattr(0, termios.TCSANOW, mode)
''')
    driver.click("#open")
    wait_for(lambda: driver.execute(READ)["transport"] == "channel", "keyboard fixture shell did not attach")
    driver.type("#input", f"/usr/bin/python3 {shlex.quote(str(probe))} {shlex.quote(str(receipt))}")
    driver.click("#send")

    def painted(marker):
        driver.click("#copy-visible")
        return marker in driver.execute(READ)["dump"]

    wait_for(lambda: painted("AETHER-KEYBOARD-READY"), "keyboard reporting did not reach the native canvas")
    driver.click("#reveal-cursor")
    click_visible_terminal(driver)
    wait_for(lambda: driver.execute("""
        return document.activeElement === document.querySelector('#ime-input') ||
               document.activeElement === document.querySelector('main canvas');
    """), "keyboard fixture did not focus the native input surface")
    actions = []
    for keys in (("\ue008", "\ue007"), ("\ue00c",), ("\ue009", "i"), ("q",)):
        actions.extend({"type": "keyDown", "value": key} for key in keys)
        actions.extend({"type": "keyUp", "value": key} for key in reversed(keys))
    driver.command("POST", "/actions", {"actions": [{"type": "key", "id": "terminal-keys", "actions": actions}]})
    driver.command("DELETE", "/actions")
    wait_for(lambda: painted("AETHER-KEYBOARD-FINISHED"), "keyboard probe did not return to the shell")
    expected = b"\x1b[?1u\x1b[13;2u\x1b[27u\x1b[105;5uq"
    actual = receipt.read_bytes()
    assert actual == expected, (expected, actual)
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)["status"].startswith("Terminal closed"), "keyboard shell did not close")
    print(json.dumps({"native_kitty_disambiguation": True, "pty_bytes_verified": len(actual)}), flush=True)


def exercise_restart(driver, owned, binary, integrated=False):
    def state():
        return driver.execute(READ)

    def chat_prompt():
        return driver.execute("""
            return document.querySelector('#active-pane').dataset.chatSurface === 'prompt' &&
                   !document.querySelector('#workflows').disabled;
        """)

    driver.click("#open")
    wait_for(lambda: state()["writable"] and len(state()["tabs"]) == 1, "restart fixture's first shell did not open")
    first = state()["current_tab"]
    first_before = write_from_current_tab(driver, owned / "before-restart-a.pid")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first restart fixture write is pending")
    forbidden = owned / "restart-must-not-run"
    driver.type("#input", f"printf replayed > {shlex.quote(str(forbidden))}")
    driver.click("#new-jsh-tab" if integrated else "#new-local-tab")
    wait_for(lambda: state()["writable"] and len(state()["tabs"]) == 2, "restart fixture's second shell did not open")
    second_before = write_from_current_tab(driver, owned / "before-restart-b.pid")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second restart fixture write is pending")
    if integrated:
        driver.click("#terminal-chat-toggle")
        wait_for(chat_prompt, "the new jsh tab did not offer Terminal Chat and workflows")
    driver.click("#split-local")
    wait_for(lambda: state()["split"] == "right" and state()["peer_geometry"] == "80x24", "restart fixture split was not visible")
    click_tab(driver, first)
    assert "replayed" in state()["draft"]

    driver.close()
    for identity in (first_before, second_before):
        wait_for(lambda: birth(identity[0]) != identity[1], "previous desktop survived shutdown with an owned shell")
    driver.open(binary)
    wait_for(lambda: opened_page(driver), "embedded page did not load after restarting Aether")
    wait_for(lambda: state()["restore_layout"], "saved shell layout unavailable after restarting Aether")
    assert not state()["writable"] and not state()["tabs"], "restart started a shell before the restore action"
    driver.click("#restore-local-layout")
    wait_for(lambda: state()["status"] == "Opened two fresh shells in the saved layout.", "layout restore did not finish")
    restored = state()
    first, second = restored["tabs"]
    assert restored["writable"] and restored["current_tab"] == first, "saved active position was lost"
    assert driver.execute("return !document.querySelector('#command').hidden;"), "the plain tab inherited Terminal Chat"
    wait_for(lambda: state()["split"] == "left" and state()["peer_geometry"] == "80x24", "saved split layout was lost")
    assert restored["draft"] == "" and not forbidden.exists(), "old command draft survived restart"
    first_after = write_from_current_tab(driver, owned / "after-restart-a.pid")
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "first restored shell write is pending")
    click_tab(driver, second)
    assert state()["draft"] == ""
    if integrated:
        wait_for(chat_prompt, "restore lost the jsh prompt or its Terminal Chat preference")
        receipt = owned / "after-restart-b.pid"
        driver.type("#ime-input", "printf '%s' \"$$\" > " + shlex.quote(str(receipt)) + "\ue007")
        wait_for(lambda: receipt.is_file() and receipt.read_text().isdigit(), "the restored live jsh did not receive input")
        second_after = shell_identity(receipt)
    else:
        second_after = write_from_current_tab(driver, owned / "after-restart-b.pid")
    assert len({first_before, second_before, first_after, second_after}) == 4, "restore reused an old shell identity"
    wait_for(lambda: state()["writable"] and state()["draft"] == "", "second restored shell write is pending")
    driver.click("#close")
    wait_for(lambda: birth(second_after[0]) != second_after[1], "restored second shell was not reaped")
    click_tab(driver, first)
    driver.click("#close")
    wait_for(lambda: birth(first_after[0]) != first_after[1], "restored first shell was not reaped")
    assert not forbidden.exists()
    result = {"native_restart_restores_tab_count_and_active_position": True,
              "native_restart_restores_split_panes": True,
              "restore_starts_fresh_pty_identities_without_replay": True}
    if integrated:
        result["native_restart_restores_jsh_terminal_chat_and_workflows"] = True
    print(json.dumps(result), flush=True)


def exercise_command_blocks(driver, owned, output_review=False, *, read_clipboard=None):
    # Leave room to reach the expanded card after its review controls appear.
    driver.command("POST", "/window/rect", {"width": 1380, "height": 1000})

    def state():
        return driver.execute(READ)

    def visible(text):
        def snapshot():
            driver.click("#copy-visible")
            return text in state()["dump"].splitlines()
        return wait_for(snapshot, f"native live grid did not display {text!r}")

    def send(command):
        wait_for(lambda: state()["writable"], "integrated shell does not own input")
        driver.type("#input", command)
        driver.click("#send")
        wait_for(lambda: state()["writable"] and state()["draft"] == "", "shell command input is still pending")

    def blocks():
        return driver.execute("""
            return Array.from(document.querySelectorAll('#command-block-list details'), card => ({
              command: card.querySelector('code').textContent,
              body: card.textContent,
              output: card.querySelector('textarea')?.value ?? null,
              output_surface: Boolean(card.querySelector('[aria-label="Command output"]')),
              empty_output: Array.from(card.querySelectorAll('p')).some(p => p.textContent === 'No output.'),
              source: card.querySelector('.command-block-output-source')?.textContent ?? null,
              open: card.open,
            }));
        """)

    def refresh_for(command):
        def read():
            if driver.execute("return document.querySelector('#command-block-refresh').disabled;"):
                return None
            if reported := next((item for item in blocks() if item["command"] == command), None):
                return reported
            driver.click("#command-block-refresh")
            return None
        return wait_for(read, f"native jsh did not report a completed block for {command!r}")

    driver.click("#open-integrated")
    wait_for(lambda: state()["writable"] and state()["transport"] == "channel", "integrated local shell did not open")
    jsh_tab = state()["current_tab"]

    def ready_prompt():
        driver.click("#copy-visible")
        return "❯" in state()["dump"]

    wait_for(ready_prompt, "local jsh did not reach its native prompt")
    receipt = owned / "jsh.pid"
    command = f'printf "%s" "$$" > {shlex.quote(str(receipt))}; cd /tmp; printf "AETHER_%s\\n" BLOCK_ONE; false'
    send(command)
    wait_for(lambda: receipt.is_file() and receipt.stat().st_size, "jsh did not write its live PID")
    identity = shell_identity(receipt)
    visible("AETHER_BLOCK_ONE")
    driver.click("#command-block-toggle")
    first = refresh_for(command)
    assert first["output"] == "AETHER_BLOCK_ONE\n" and "Exit 1" in first["body"], first
    assert "PTY-reported OSC 133 · jsh-" in first["body"], first
    driver.click("#command-block-list details summary")
    driver.click("#command-block-list details button")

    def copy_status():
        return driver.execute("""
            const status=document.querySelector('#command-block-status').textContent;
            const output=document.querySelector('#command-block-list textarea');
            if(status==='Output copied.')return 'clipboard';
            if(status.startsWith('Output selected.')&&output.selectionStart===0&&output.selectionEnd===output.value.length){
              return 'selection';
            }
            return null;
        """)

    if wait_for(copy_status, "native block output copy did not select only captured text") == "clipboard":
        expected_output = first["output"].encode()
        if read_clipboard is not None:
            try:
                wait_for(lambda: read_clipboard() == expected_output,
                         "Wayland clipboard did not receive the completed block text")
            except AssertionError as error:
                raise AssertionError((expected_output, read_clipboard())) from error
        elif shutil.which("xclip"):
            assert subprocess.check_output(["xclip", "-selection", "clipboard", "-o"], timeout=2) == expected_output
    driver.click("#command-block-return")
    assert state()["focused"] == "ime-input"
    send("pwd")
    visible("/tmp")
    driver.click("#command-block-toggle")
    pwd = refresh_for("pwd")
    assert pwd["output"] == "/tmp\n" and "Exit 0" in pwd["body"], pwd
    assert any(item["command"] == command and item["open"] for item in blocks()), "another block collapsed the previous card"
    # Reinsert the original revision and append a shell operator before pressing
    # Enter. That suffix only parses as part of the recalled, still-editable line.
    recall = "#command-block-list details[open] .command-block-recall"
    driver.type(recall, "\ue007")
    wait_for(lambda: driver.execute("return document.querySelector('#command-block-status').textContent") ==
             "Command inserted at the shell prompt.", "command was not inserted at the live prompt")
    recalled = owned / "recalled-line"
    driver.type("#ime-input", f"; printf recalled > {shlex.quote(str(recalled))}\ue007")
    wait_for(lambda: recalled.is_file() and recalled.read_text() == "recalled",
             "the appended recall command did not run after explicit Enter")

    # The actual live PTY receives hidden input while a command is running.
    # The input is never included in that command's OSC metadata or output.
    driver.click("#command-block-return")
    secret_receipt = owned / "hidden-input"
    secret_command = ("/bin/sh -c 'stty -echo; printf \"AETHER_%s\\n\" PRIVATE_READY; "
                      "IFS= read -r input; stty echo; printf \"%s\" \"$input\" > "
                      + str(secret_receipt) + "; printf \"AETHER_%s\\n\" PRIVATE_DONE'")
    send(secret_command)
    visible("AETHER_PRIVATE_READY")
    driver.click("#command-block-toggle")
    driver.type(recall, "\ue007")
    wait_for(lambda: "Wait for the integrated shell prompt" in driver.execute(
        "return document.querySelector('#command-block-status').textContent"), "command recall reached foreground read")
    driver.click("#command-block-return")
    sentinel = "native-hidden-input"
    driver.type("#ime-input", sentinel + "\ue007")
    wait_for(lambda: secret_receipt.is_file() and secret_receipt.read_text() == sentinel,
             "the live shell did not receive the hidden input")
    driver.click("#command-block-toggle")
    private = refresh_for(secret_command)
    assert private["output"] == "AETHER_PRIVATE_READY\nAETHER_PRIVATE_DONE\n", private
    assert sentinel not in private["body"]
    print(json.dumps({"native_jsh_recall": ["editable line", "explicit Enter", "foreground input isolation"]}), flush=True)
    driver.click("#command-block-return")

    # Exercise live alternate-screen rendering and keyboard return in the same
    # integrated PTY. The fullscreen screen contents do not become a block.
    editor_file = owned / "vim-fixture.txt"
    editor_file.write_text("AETHER_NATIVE_VIM\n")
    editor = f"vim -Nu NONE -i NONE -n -X {shlex.quote(str(editor_file))}"
    send(editor)
    visible("AETHER_NATIVE_VIM")
    driver.type("#ime-input", "\ue00c:q!\ue007")
    wait_for(ready_prompt, "Vim did not return to the live jsh prompt")
    driver.click("#command-block-toggle")
    fullscreen = refresh_for(editor)
    assert fullscreen["empty_output"] and not fullscreen["output_surface"], fullscreen
    assert "Exit 0" in fullscreen["body"] and "AETHER_NATIVE_VIM" not in fullscreen["body"], fullscreen
    driver.click("#command-block-return")

    driver.click("#new-local-tab")
    wait_for(lambda: state()["current_tab"] != jsh_tab and state()["writable"], "plain peer tab did not open")
    plain_tab = state()["current_tab"]
    driver.click("#split-local")
    driver.click("#command-block-toggle")
    wait_for(lambda: "sessions opened with jsh" in driver.execute("""
        return document.querySelector('#command-block-status').textContent;
    """), "peer inherited the integrated tab's block state")
    assert not blocks()
    click_tab(driver, jsh_tab)
    driver.click("#command-block-toggle")
    assert refresh_for(command)["output"] == first["output"]
    assert state()["split"] and birth(identity[0]) == identity[1], "block view changed the live PTY"
    if output_review:
        exercise_frozen_output_review(driver, owned, send, refresh_for, blocks, command)
        assert re.fullmatch(r"PTY bytes \[\d+, \d+\) · marker events \d+–\d+ · grid \d+×\d+",
                            first["source"] or ""), first
        driver.click("#command-block-return")
        over_limit = r"head -c 17408 /dev/zero | tr '\000' Q"
        send(over_limit)
        driver.click("#command-block-toggle")
        unavailable = refresh_for(over_limit)
        assert unavailable["output"] is None and "command block size limit" in unavailable["body"], unavailable
        source = re.fullmatch(r"PTY bytes \[(\d+), (\d+)\) · marker events \d+–\d+ · grid \d+×\d+",
                              unavailable["source"] or "")
        assert source and int(source[2]) - int(source[1]) == 17408, unavailable
        print(json.dumps({"native_command_card_shows_pty_source_with_unavailable_output": True}), flush=True)
    driver.click("#close")
    wait_for(lambda: birth(identity[0]) != identity[1], "closed integrated shell was not reaped")
    click_tab(driver, plain_tab)
    driver.click("#close")
    wait_for(lambda: state()["status"].startswith("Terminal closed"), "plain peer did not close")
    print(json.dumps({"native_jsh_markers_from_owned_pty": True,
                      "native_command_block_plain_output_and_status": True,
                      "native_block_return_preserves_pty_cwd_and_input": True,
                      "native_blocks_exclude_hidden_input_and_vim_screen": True,
                      "native_blocks_isolated_between_split_tabs": True}), flush=True)


def exercise_terminal_chat(driver, owned):
    driver.command("POST", "/window/rect", {"width": 1380, "height": 1000})
    toggle = "#terminal-chat-toggle"
    terminal = "#ime-input"

    def view():
        return driver.execute("""
            const pane = document.querySelector('#active-pane');
            const send = document.querySelector('#terminal-chat-send');
            const transcript = document.querySelector('#command-blocks');
            return {
              surface: pane.dataset.chatSurface,
              legacy: !document.querySelector('#command').hidden,
              send: !send.hidden && !send.disabled,
              transcript: !transcript.hidden && pane.contains(transcript),
              chrome: !document.querySelector('#terminal-chat-dock').hidden,
              cards: Array.from(transcript.querySelectorAll('details'), card => ({
                command: card.querySelector('code').textContent,
                body: card.textContent,
                output: card.querySelector('textarea')?.value ?? null,
              })),
            };
        """)

    def phase(surface):
        def observed():
            current = view()
            return current if current['surface'] == surface else None
        return wait_for(observed, f"Terminal Chat did not enter {surface}")

    def card_for(command):
        return wait_for(lambda: next((card for card in view()['cards'] if card['command'] == command), None),
                        f"Terminal Chat did not automatically show {command!r}")

    driver.click("#open-integrated")
    wait_for(lambda: not driver.execute(f"return document.querySelector('{toggle}').disabled;"),
             "the integrated terminal did not acquire a local view")
    live_input = driver.element_path(terminal)
    live_canvas = driver.element_path("#active-pane canvas")
    driver.click(toggle)
    assert phase("prompt")['transcript']
    pid = owned / "terminal-chat.pid"
    receipt = owned / "terminal-chat-send"
    prefix = f"cd {shlex.quote(str(owned))}; printf '%s' \"$$\" > {shlex.quote(str(pid))}; "
    suffix = f"printf x >> {shlex.quote(str(receipt))}; printf 'TERMINAL_CHAT_%s\\n' ONCE"
    driver.type(terminal, prefix)
    driver.click(toggle)
    assert phase('')['legacy'] and not receipt.exists()
    driver.click(toggle)
    assert not phase('prompt')['legacy']
    driver.type(terminal, suffix)
    assert not receipt.exists(), "switching presentation submitted live shell input"
    wait_for(lambda: view()['send'], "send was unavailable at the native prompt")
    driver.click("#terminal-chat-send")
    first = card_for(prefix + suffix)
    assert receipt.read_text() == "x" and first['output'] == "TERMINAL_CHAT_ONCE\n", first
    identity = shell_identity(pid)
    assert driver.element_path(terminal) == live_input
    assert driver.element_path("#active-pane canvas") == live_canvas
    driver.type(terminal, "pwd\ue007")
    assert card_for("pwd")['output'] == str(owned) + "\n"

    hidden_receipt = owned / "terminal-chat-hidden"
    hidden_command = "/bin/sh -c " + shlex.quote(
        f"stty -echo; printf 'CHAT_READ_READY\\n'; IFS= read -r reply; stty echo; printf '%s' \"$reply\" > {hidden_receipt}")
    driver.type(terminal, hidden_command + "\ue007")
    running = phase("running")
    assert running['transcript'] and not running['send'] and not running['legacy']
    driver.click(toggle)
    assert phase('')['legacy']
    driver.click(toggle)
    assert phase("running")['transcript']
    hidden = "terminal-chat-hidden-value"
    driver.type(terminal, hidden + "\ue007")
    wait_for(lambda: hidden_receipt.is_file() and hidden_receipt.read_text() == hidden,
             "foreground read did not receive the actual terminal input")
    assert hidden not in card_for(hidden_command)['body']
    assert phase("prompt")['send']

    editor_file = owned / "terminal-chat-vim.txt"
    editor_file.write_text("TERMINAL_CHAT_VIM\n")
    editor = f"vim -Nu NONE -i NONE -n -X {shlex.quote(str(editor_file))}"
    driver.type(terminal, editor + "\ue007")
    fullscreen = phase("fullscreen")
    assert not fullscreen['transcript'] and not fullscreen['chrome'] and not fullscreen['legacy']
    driver.type(terminal, "\ue00c:q!\ue007")
    assert phase("prompt")['transcript']
    assert card_for(editor)['output'] == ''
    assert birth(identity[0]) == identity[1] and receipt.read_text() == "x"
    driver.click("#close")
    wait_for(lambda: birth(identity[0]) != identity[1], "Terminal Chat leaked the original PTY child")
    wait_for(lambda: driver.execute(
        "return document.querySelector('#status').textContent.startsWith('Terminal closed') && "
        "!document.querySelector('#open').disabled"
    ), "the close receipt did not enable a new shell")

    driver.click("#open")
    def plain_ready():
        state = driver.execute(READ)
        return (state["transport"] == "channel" and state["writable"]
                and not driver.execute(f"return document.querySelector('{toggle}').disabled;"))

    wait_for(plain_ready, "the ordinary shell did not acquire a writable Channel view")
    driver.click(toggle)
    fallback = phase("passthrough")
    assert not fallback['transcript'] and not fallback['send'] and not fallback['legacy']
    plain = owned / "terminal-chat-plain"
    driver.type(terminal, f"printf plain > {shlex.quote(str(plain))}\ue007")
    wait_for(lambda: plain.is_file() and plain.read_text() == "plain", "Chat fallback lost shell input")
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)['status'].startswith("Terminal closed"), "ordinary shell did not close")
    print(json.dumps({"native_terminal_chat_same_live_renderer_and_pty": True,
                      "native_terminal_chat_one_enter_and_automatic_cards": True,
                      "native_terminal_chat_hidden_foreground_input": True,
                      "native_terminal_chat_vim_fullscreen_and_passthrough": True}), flush=True)


def prepare_workflows(owned, env):
    config = Path(env["XDG_CONFIG_HOME"]) / "aether" / "workflows"
    override = owned / "workflow-override"
    config.mkdir(parents=True)
    override.mkdir()
    env["AETHER_WORKFLOW_DIR"] = str(override)
    command = "printf '%s\\n' '{value}-{suffix}' >> " + shlex.quote(str(owned / "workflow-result"))
    (config / "report.toml").write_text(
        'name = "Local report"\n'
        'description = "Write the report fixture"\n'
        'tags = ["native-report-fixture"]\n'
        f'command = {json.dumps(command)}\n'
        '\n[[args]]\nname = "value"\n'
        '\n[[args]]\nname = "suffix"\ndefault = "default"\n',
        encoding="utf-8",
    )
    (override / "duplicate.yaml").write_text(
        'name: Local report\ncommand: printf wrong\n', encoding="utf-8")
    (override / "additional.yaml").write_text(
        'name: YAML check\ncommand: printf yaml\n', encoding="utf-8")


def exercise_terminal_workflows(driver, owned, *, verify_bookmarks=True):
    terminal = "#ime-input"
    receipt = owned / "workflow-result"

    def view():
        return driver.execute("""
            return {
              available: !document.querySelector('#workflows').disabled,
              open: !document.querySelector('#workflow-picker').hidden,
              rows: Array.from(document.querySelectorAll('#workflow-results button'), el => el.textContent),
              fields: !document.querySelector('#workflow-fields').hidden,
              command: document.querySelector('#workflow-command').textContent,
              defaults: Array.from(document.querySelectorAll('#workflow-args input'), el => el.value),
              message: document.querySelector('#workflow-status').textContent,
              insert: document.querySelector('#workflow-insert').textContent,
            };
        """)

    driver.click("#open-integrated")
    wait_for(lambda: view()["available"], "the integrated prompt did not enable workflows")
    original = driver.execute(READ)["current_tab"]
    driver.click("#terminal-chat-toggle")
    driver.click("#workflows")
    wait_for(lambda: view()["rows"].count("Local report") == 1 and "YAML check" in view()["rows"],
             "the workflow library did not use config precedence and the additive override")
    driver.type("#workflow-search", "native-report-fixture")
    wait_for(lambda: view()["rows"] == ["Local report"], "workflow tag search did not narrow the results")
    driver.click("#workflow-results li:first-child button")
    wait_for(lambda: view()["fields"], "workflow arguments did not open")
    assert view()["defaults"] == ["", "default"] and "{value}-{suffix}" in view()["command"]
    driver.click("#workflow-insert")
    wait_for(lambda: "missing values" in view()["message"], "a required workflow value was accepted blank")
    assert not receipt.exists()

    driver.type("#workflow-args label:first-child input", "alpha beta")
    driver.click("#workflow-insert")
    wait_for(lambda: not view()["open"] and
             driver.execute(READ)["status"].startswith("Workflow queued at the shell cursor."),
             "the selected workflow did not reach the live shell")
    assert not receipt.exists(), "workflow insertion executed before Enter"
    driver.type(terminal, "; printf 'edited\\n' >> " + shlex.quote(str(receipt)) + "\ue007")
    expected = "alpha beta-default\nedited\n"
    wait_for(lambda: receipt.is_file() and receipt.read_text() == expected,
             "the workflow did not stay editable in the selected jsh prompt")
    if verify_bookmarks:
        exercise_completed_block_search(driver)
        bookmarked = bookmark_selected_command(driver)

    wait_for(lambda: view()["available"], "workflows did not follow the next shell prompt")
    driver.click("#workflows")
    wait_for(lambda: view()["rows"][:1] == ["Local report"], "the next prompt did not open its own catalog")
    driver.click("#workflow-results li:first-child button")
    wait_for(lambda: view()["fields"], "the workflow form did not open before switching tabs")
    driver.type("#workflow-args label:first-child input", "quoted value")
    driver.click("#new-local-tab")
    peer = wait_for(lambda: (tab if (tab := driver.execute(READ)["current_tab"]) != original else None),
                    "the other shell tab did not activate")
    wait_for(lambda: view()["open"] and view()["fields"] and view()["insert"] == f"Insert in shell {original}",
             "the selected workflow lost its original shell target on focus change")
    assert view()["defaults"] == ["quoted value", "default"]
    assert not driver.execute("return document.querySelector('#command-block-matches').children.length;")
    assert receipt.read_text() == expected, "changing focus executed the workflow"
    driver.click("#workflow-insert")
    wait_for(lambda: driver.execute(READ)["current_tab"] == original and not view()["open"] and
             driver.execute(READ)["status"].startswith("Workflow queued at the shell cursor."),
             "workflow was not inserted at the original terminal after switching focus")
    assert receipt.read_text() == expected, "insert executed without explicit Enter"
    driver.type(terminal, "\ue007")
    expected += "quoted value-default\n"
    wait_for(lambda: receipt.read_text() == expected, "quoted/default workflow was not inserted exactly once")
    click_tab(driver, peer)
    peer_receipt = owned / "workflow-peer-receipt"
    driver.type(terminal, "\ue007")
    driver.type(terminal, f"printf peer > {shlex.quote(str(peer_receipt))}\ue007")
    wait_for(lambda: peer_receipt.is_file() and peer_receipt.read_text() == "peer", "peer shell did not process its own input")
    assert receipt.read_text() == expected, "input also reached the peer shell"
    click_tab(driver, original)
    wait_for(lambda: view()["available"], "workflows did not return with the integrated shell")
    assert not view()["open"] and receipt.read_text() == expected
    if verify_bookmarks:
        exercise_bookmark_retention(driver, bookmarked)
    driver.click("#close")
    click_tab(driver, peer)
    driver.click("#close")
    wait_for(lambda: driver.execute(READ)["status"].startswith("Terminal closed"),
             "the workflow fixture shells did not close")
    witnesses = {
        "native_workflow_shared_discovery_and_precedence": True,
        "native_workflow_required_and_default_arguments": True,
        "native_workflow_inserts_editable_text_with_explicit_enter": True,
        "native_workflow_selected_form_keeps_original_pane_across_focus_switch": True,
        "native_workflow_inserts_into_original_pane_once_and_peer_is_untouched": True,
    }
    if verify_bookmarks:
        witnesses.update(native_completed_block_search_selects_owned_command_and_output_ranges=True,
                         native_bookmark_follows_exact_owned_command_until_eviction=True)
    return witnesses


def exercise_completed_block_search(driver):
    command = r"printf '\360\237\220\207 %s%s\n' alpha suffix"
    driver.type("#ime-input", command + "\ue007")
    wait_for(lambda: driver.execute(r"""
        return Array.from(document.querySelectorAll('#command-block-list details'),
                          card => card.querySelector('textarea')?.value).includes('🐇 alphasuffix\n');
    """), "the search fixture output did not appear in a retained command card")
    driver.type("#command-block-query", "alphasuffix\ue007")
    wait_for(lambda: driver.execute("""
        const results = document.querySelectorAll('#command-block-matches button');
        return results.length === 1 && results[0].textContent.startsWith('Output');
    """), "retained command search did not isolate the output hit")
    driver.click("#command-block-matches button")
    assert driver.execute(r"""
        const output = document.activeElement;
        return output.matches('#command-block-list textarea') &&
            output.value === '🐇 alphasuffix\n' && output.selectionStart === 3 &&
            output.value.slice(output.selectionStart, output.selectionEnd) === 'alphasuffix';
    """), "an output hit did not select the exact UTF-16 range in its retained record"
    driver.click("#command-block-query-clear")
    driver.type("#command-block-query", "%s%s\ue007")
    wait_for(lambda: driver.execute("""
        const results = document.querySelectorAll('#command-block-matches button');
        return results.length === 1 && results[0].textContent.startsWith('Command');
    """), "retained command search did not isolate the command hit")
    driver.click("#command-block-matches button")
    assert driver.execute("""
        const selected = document.getSelection();
        return selected.toString() === '%s%s' &&
               selected.anchorNode.parentElement.matches('#command-block-list summary code');
    """), "a command hit did not select the exact text of the shell command"


def bookmark_selected_command(driver):
    record = driver.execute("""
        const card = document.getSelection().anchorNode.parentElement.closest('details');
        return {sequence: card.dataset.sequence, revision: card.dataset.revision};
    """)
    assert re.fullmatch(r"[0-9]+", record["sequence"])
    assert re.fullmatch(r"[0-9a-f]{64}", record["revision"])
    card = f"#command-block-list details[data-sequence='{record['sequence']}'][data-revision='{record['revision']}']"
    driver.click(card + " .command-block-bookmark")
    wait_for(lambda: driver.execute(f"return !!document.querySelector({json.dumps(card)} + '[data-bookmarked=true]');"),
             "the exact command card was not bookmarked")
    driver.click("#command-block-bookmarked")
    assert driver.execute("""
        return Array.from(document.querySelectorAll('#command-block-list details:not([hidden])'), card => card.dataset.sequence);
    """) == [record["sequence"]]
    return card


def exercise_bookmark_retention(driver, card):
    def bookmarked():
        return driver.execute(f"return !!document.querySelector({json.dumps(card)} + '[data-bookmarked=true]');")

    def empty():
        return driver.execute("return !document.querySelector('#command-block-no-bookmarks').hidden;")

    wait_for(bookmarked, "switching tabs lost the original command bookmark")
    filter_button = "#command-block-bookmarked"
    pin = card + " .command-block-bookmark"
    driver.click(filter_button)
    driver.click(card + " summary")
    driver.click(pin)
    wait_for(empty, "removing the bookmark left the record in the filtered list")
    driver.click(filter_button)
    driver.click(pin)
    wait_for(bookmarked, "the original record could not be bookmarked again")
    driver.click(filter_button)
    for index in range(8):
        command = f"printf 'BOOKMARK_EVICT_{index}\\n'"
        driver.type("#ime-input", command + "\ue007")
        wait_for(lambda: driver.execute(f"""
            return Array.from(document.querySelectorAll('#command-block-list details summary code'))
                .some(command => command.textContent === {json.dumps(command)});
        """), "the replacement command did not reach the retained transcript")
    wait_for(empty, "evicting the command did not clear the filtered list")
    assert not driver.execute(f"return !!document.querySelector({json.dumps(card)});")


def exercise_frozen_output_review(driver, owned, send, refresh_for, blocks, before):
    def select_revision(field, command):
        selector = f"#command-review-{field}"
        choice = driver.execute(f"""
            const id=Array.from(document.querySelectorAll('#command-block-list details'))
                .find(card=>card.querySelector('code').textContent==={json.dumps(command)})?.dataset.sequence;
            const select=document.querySelector({json.dumps(selector)});
            const index=Array.from(select.options).findIndex(option=>option.textContent.startsWith('#'+id+' · '));
            return index<0 ? null : {{ index, revision:select.options[index].value, selected:index===select.selectedIndex }};
        """)
        assert choice and re.fullmatch(r"[0-9a-f]{64}", choice["revision"])
        if not choice["selected"]:
            driver.type(selector, "\ue011" + "\ue015" * choice["index"])
        wait_for(lambda: driver.execute(f"return document.querySelector({json.dumps(selector)}).value;") ==
                 choice["revision"], f"{field} output revision was not selected")
        return choice["revision"]

    before_revision = select_revision("before", before)
    after_revision = select_revision("after", "pwd")
    driver.click("#command-review-compare")

    def current():
        return driver.execute("""
            const view=document.querySelector('#command-review');
            return !view.hidden && {id:view.dataset.reviewId,
                diff:view.querySelector('pre').textContent};
        """)

    frozen = wait_for(current, "frozen output comparison did not render")
    assert "-AETHER_BLOCK_ONE\n+/tmp\n" in frozen["diff"], frozen
    for index in range(8):
        replacement = f"printf 'REVIEW_REPLACEMENT_{index}\\n'"
        send(replacement)
        refresh_for(replacement)
    assert not any(record["command"] in (before, "pwd") for record in blocks()), "old records were not evicted"
    assert current() == frozen, "live updates changed the frozen comparison"

    for extension, button in (("md", "markdown"), ("json", "json")):
        driver.click("#command-review-" + button)
        name = f"aether-output-{frozen['id']}.{extension}"

        def downloaded():
            return next((path for path in (owned / name, owned / "Downloads" / name)
                         if path.is_file() and path.stat().st_size), None)

        evidence = wait_for(downloaded, f"native WebKit did not save {name}")
        content = evidence.read_text()
        if extension == "json":
            exported = json.loads(content)
            assert exported["schema"] == "aether.command-output-review.v1"
            assert exported["review"]["reviewId"] == frozen["id"]
            assert exported["review"]["comparison"] == frozen["diff"]
            assert exported["before"]["revision"] == before_revision
            assert exported["after"]["revision"] == after_revision
            assert exported["before"]["output"] == {"kind": "text", "text": "AETHER_BLOCK_ONE\n"}
            assert exported["after"]["output"] == {"kind": "text", "text": "/tmp\n"}
            previous, latest = (exported[side]["output_source"] for side in ("before", "after"))
            for side, source in (("before", previous), ("after", latest)):
                assert 0 < int(source["start_event"]) < int(source["end_event"]), source
                assert int(source["byte_end"]) - int(source["byte_start"]) >= len(
                    exported[side]["output"]["text"].encode()
                ), source
                assert all(value > 0 for size in ("start_size", "end_size") for value in source[size])
            assert int(previous["byte_end"]) < int(latest["byte_start"])
            assert int(previous["end_event"]) < int(latest["start_event"])
        else:
            assert content.startswith("# Frozen command output review\n")
            assert before_revision in content and after_revision in content
            assert frozen["diff"] in content
            assert content.count("PTY bytes `[") == 2
        assert "REVIEW_REPLACEMENT" not in content
    print(json.dumps({"native_output_review_exact_owned_records": True,
                      "native_output_review_stays_frozen_after_retention_eviction": True,
                      "native_output_review_preserves_pty_ranges_markers_and_geometry": True,
                      "native_output_review_downloads_markdown_and_json": True}), flush=True)


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


WORKSPACE_ID = "11111111-1111-4111-8111-111111111111"


def prepare_workspace(owned, env, launch):
    profile = owned / "profile with spaces"
    project = owned / "cli-project"
    project.mkdir()
    transcripts = {}
    for account in (profile, owned / ".codex"):
        source = account / "sessions" / "2026" / "11" / "22" / "rollout.jsonl"
        source.parent.mkdir(parents=True)
        records = [
            {"type": "session_meta", "payload": {"id": WORKSPACE_ID, "cwd": str(project)}},
            {"type": "event_msg", "payload": {"type": "user_message", "message": "Workspace fixture"}},
            {"type": "event_msg", "payload": {"type": "task_complete"}},
        ]
        source.write_text("".join(json.dumps(record) + "\n" for record in records))
        transcripts[str(account)] = source
    executable = launch / "codex"
    executable.write_text("""#!/bin/sh
set -eu
test "$1" = resume
printf '%s|%s|%s|%s\\n' "$$" "$CODEX_HOME" "$PWD" "$2" >> "$AETHER_WORKSPACE_TRACE"
printf 'WORKSPACE_READY\\n'
query_once() {
    expected=$(printf '\\033[12;%sR' "$1")
    saved=$(stty -g)
    stty raw -echo min 0 time 50
    printf '\\0337\\033[12;%sH\\033[' "$1"
    if [ "$#" -eq 2 ]; then
        while [ ! -e "$2" ]; do sleep 0.02; done
    fi
    printf '6n'
    reply=$(dd bs=1 count="${#expected}" status=none)
    stty min 0 time 2
    extra=$(dd bs=1 count=1 status=none)
    stty "$saved"
    test "$reply" = "$expected"
    test -z "$extra"
    printf '\\0338'
}
while IFS= read -r line; do
    printf '%s\\n' "$line" >> "$AETHER_WORKSPACE_INPUT"
    if [ "$line" = exit-fixture ]; then
        printf '\\033[2K\\rWORKSPACE_LAST_OUTPUT\\n'
        exit 7
    fi
    if [ "$line" = query-fixture ]; then
        query_once 7
        printf '\\033[2K\\rWORKSPACE_QUERY_REPLY_OK\\n'
        continue
    fi
    if [ "$line" = legacy-query-fixture ]; then
        query_once 17
        printf '\\033[2K\\rWORKSPACE_LEGACY_REPLY_OK\\n'
        continue
    fi
    if [ "$line" = detached-query-fixture ]; then
        printf '\\033[2K\\rWORKSPACE_DETACHED_QUERY_ARMED\\n'
        query_once 300 "$AETHER_WORKSPACE_QUERY_GATE"
        printf '\\033[2K\\rWORKSPACE_DETACHED_REPLY_OK\\n'
        : > "$AETHER_WORKSPACE_QUERY_RESULT"
        continue
    fi
    if [ "$line" = paste-fixture ]; then
        saved=$(stty -g)
        stty raw -echo min 0 time 60
        printf '\\033[?2004h\\033[2K\\rWORKSPACE_PASTE_ARMED\\r\\n'
        paste=$(dd bs=1 count=22 status=none)
        stty "$saved"
        test "$paste" = "$(printf '\\033[200~left\\rright\\033[201~')"
        printf '\\033[?2004l\\033[2K\\rWORKSPACE_PASTE_OK\\n'
        continue
    fi
    printf '\\033[2K\\rWORKSPACE_REPLY:%s\\n' "$line"
done
""")
    executable.chmod(0o700)
    env.update({
        "CODEX_HOME": str(profile),
        "JTERM_WORKSPACE_STATE_DIR": str(owned / "workspace"),
        "AETHER_WORKSPACE_TRACE": str(owned / "provider-starts"),
        "AETHER_WORKSPACE_INPUT": str(owned / "provider-input"),
        "AETHER_WORKSPACE_QUERY_GATE": str(owned / "provider-query-gate"),
        "AETHER_WORKSPACE_QUERY_RESULT": str(owned / "provider-query-result"),
    })
    return transcripts


def exercise_workspace(driver, owned, binary, env, transcripts):
    state = Path(env["JTERM_WORKSPACE_STATE_DIR"])
    socket_file = state / "run" / "host.sock"
    log = owned / "workspace-host.log"
    with log.open("wb") as output:
        host = subprocess.Popen([binary, "--jterm-session-host"], env=env, cwd=owned,
                                stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                start_new_session=True)
    helper = None
    helper_output = bytearray()
    master = None
    identity = None
    trace = Path(env["AETHER_WORKSPACE_TRACE"])
    received = Path(env["AETHER_WORKSPACE_INPUT"])
    profile = env["CODEX_HOME"]
    saved_files = {path: path.read_bytes() for path in transcripts.values()}

    def read_state():
        return driver.execute("""
            return {
              status: document.querySelector('#workspace-status').textContent,
              writable: !document.querySelector('#workspace-input').disabled,
              choices: Array.from(document.querySelector('#workspace-conversation').options, option =>
                ({id:option.value,title:option.textContent})),
              selected: document.querySelector('#workspace-conversation').value,
              visible: !document.querySelector('#workspace-view').hidden,
              output: document.querySelector('#workspace-output').value,
            };
        """)

    def select_conversation(conversation):
        choices = read_state()["choices"]
        position = next(index for index, row in enumerate(choices, 1) if row["id"] == conversation)
        driver.click(f"#workspace-conversation option:nth-child({position})")
        assert read_state()["selected"] == conversation

    def select_fixture():
        driver.click("#agent-workspace > summary")
        wait_for(lambda: len(read_state()["choices"]) == 2, "native Workspace catalog did not show both sources")
        choices = read_state()["choices"]
        assert len({choice["id"] for choice in choices}) == 2, "both profiles collapsed into one native UUID"
        assert all(choice["title"].startswith("Workspace fixture · Profile ") for choice in choices), choices
        conversation = next(row["id"] for row in choices
                            if json.loads(row["id"].removeprefix("jterm:cli:v1:"))[2] == profile)
        select_conversation(conversation)
        return conversation

    def shows(text):
        def snapshot():
            if not read_state()["visible"]:
                return False
            driver.click("#workspace-copy-visible")
            return text in read_state()["output"].splitlines()
        wait_for(snapshot, f"Workspace Canvas did not show {text!r}")

    try:
        wait_for(lambda: socket_file.is_socket(), "the embedded Workspace host did not bind")
        conversation = select_fixture()
        assert json.loads(conversation.removeprefix("jterm:cli:v1:"))[2:] == [profile, WORKSPACE_ID]
        database = state / "workspace.sqlite3"
        with contextlib.closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as db:
            assert db.execute("SELECT version FROM schema_version").fetchone()[0] == 6
            row = json.loads(db.execute("SELECT row_json FROM conversations WHERE id=?", (conversation,)).fetchone()[0])
        descriptor = {
            "row": row, "native_id": WORKSPACE_ID, "provider": "codex", "provider_home": profile,
            "source": str(transcripts[profile]), "cwd": row["cwd"], "cols": 80, "rows": 24,
        }
        master, slave = pty.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 384, 0, 0))
            os.set_blocking(master, False)
            helper = subprocess.Popen([binary, "--jterm-session-attach", json.dumps(descriptor)],
                                      cwd=owned, env=env, stdin=slave, stdout=slave, stderr=slave,
                                      start_new_session=True)
        finally:
            os.close(slave)
        def helper_saw(text):
            try:
                helper_output.extend(os.read(master, 65536))
            except BlockingIOError:
                pass
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
            assert len(helper_output) < 1024 * 1024
            if text in helper_output:
                return True
            code = helper.poll()
            if code is not None:
                raise AssertionError(f"embedded Workspace terminal exited before {text!r}: {code}")
            return False

        wait_for(lambda: helper_saw(b"WORKSPACE_READY"), "the existing embedded terminal did not attach")
        starts = trace.read_text().splitlines()
        assert len(starts) == 1
        pid, home, cwd, native_id = starts[0].split("|")
        identity = int(pid), birth(int(pid))
        assert identity[1] and (home, cwd, native_id) == (profile, row["cwd"], WORKSPACE_ID)
        os.write(master, b"from-existing-terminal\n")
        wait_for(lambda: helper_saw(b"WORKSPACE_REPLY:from-existing-terminal"), "existing lease did not deliver input")
        os.write(master, b"legacy-query-fixture\n")
        wait_for(lambda: helper_saw(b"\x1b[12;17H\x1b[6n"), "legacy attachment did not receive the query")
        os.write(master, b"\x1b[12;17R")
        wait_for(lambda: helper_saw(b"WORKSPACE_LEGACY_REPLY_OK"), "legacy query was lost or answered twice")

        driver.click("#workspace-attach")
        wait_for(lambda: "attached to another terminal" in read_state()["status"], "Workspace host failed to reject a competing lease")
        assert not read_state()["writable"] and not read_state()["visible"]
        os.write(master, b"detached-query-fixture\n")
        wait_for(lambda: helper_saw(b"WORKSPACE_DETACHED_QUERY_ARMED"), "detached query fixture was not armed")
        wait_for(lambda: helper_saw(b"\x1b[12;300H\x1b["),
                 "the query prefix did not reach the legacy viewport before detach")
        os.write(master, b"\x1d")
        assert helper.wait(timeout=5) == 0
        assert birth(identity[0]) == identity[1]
        def detached():
            reply = workspace_command(socket_file, "Runtime", {"id": conversation})
            return reply.get("Ok", {}).get("Runtime", {}).get("snapshot", {}).get("state") == "Running"

        wait_for(detached, "the Session Host did not release the legacy attachment")
        Path(env["AETHER_WORKSPACE_QUERY_GATE"]).touch()
        wait_for(Path(env["AETHER_WORKSPACE_QUERY_RESULT"]).exists,
                 "the detached provider did not receive exactly one wide-viewport reply")

        driver.click("#workspace-attach")
        wait_for(lambda: read_state()["writable"], "native Workspace view did not adopt the host lease")
        shows("WORKSPACE_REPLY:from-existing-terminal")
        shows("WORKSPACE_DETACHED_REPLY_OK")
        driver.type("#workspace-input", "from-aether" + "\ue007")
        shows("WORKSPACE_REPLY:from-aether")
        driver.type("#workspace-input", "query-fixture" + "\ue007")
        shows("WORKSPACE_QUERY_REPLY_OK")
        driver.type("#workspace-input", "paste-fixture" + "\ue007")
        shows("WORKSPACE_PASTE_ARMED")
        driver.execute("""
            const input = document.querySelector('#workspace-input');
            const event = new Event('paste', {bubbles: true, cancelable: true});
            Object.defineProperty(event, 'clipboardData', {value: {getData: () => 'left\\r\\nright'}});
            input.dispatchEvent(event);
        """)
        assert driver.execute("return !document.querySelector('#workspace-paste-review').hidden;")
        assert driver.execute("return document.querySelector('#workspace-paste-preview').value;") == "left\nright"
        driver.click("#workspace-paste-confirm")
        shows("WORKSPACE_PASTE_OK")
        driver.click("#workspace-detach")
        wait_for(lambda: "Terminal detached" in read_state()["status"], "native Workspace view did not release its lease")
        assert birth(identity[0]) == identity[1]

        driver.click("#workspace-attach")
        wait_for(lambda: read_state()["writable"], "detached Workspace did not reattach")
        shows("WORKSPACE_REPLY:from-aether")
        driver.close()
        assert birth(identity[0]) == identity[1], "closing the attached app stopped the Workspace provider"
        driver.open(binary)
        wait_for(lambda: opened_page(driver), "embedded application did not reopen")
        assert select_fixture() == conversation
        assert driver.execute("return document.querySelector('#workspace-remove').disabled;")
        other = next(row["id"] for row in read_state()["choices"] if row["id"] != conversation)
        select_conversation(other)
        driver.click("#workspace-remove")
        assert driver.execute("return document.querySelector('#workspace-remove-selection').textContent.includes('Profile ');")
        driver.click("#workspace-remove-cancel")
        assert {row["id"] for row in read_state()["choices"]} == {conversation, other}
        driver.click("#workspace-remove")
        driver.click("#workspace-remove-confirm")
        wait_for(lambda: read_state()["status"] == "Conversation removed from Workspace.",
                 "Workspace did not confirm removal of the selected source")
        assert [row["id"] for row in read_state()["choices"]] == [conversation]
        driver.click("#workspace-refresh")
        wait_for(lambda: not driver.execute("return document.querySelector('#workspace-refresh').disabled;"),
                 "the CLI catalog did not refresh after removing a source")
        assert [row["id"] for row in read_state()["choices"]] == [conversation]
        assert all(path.read_bytes() == data for path, data in saved_files.items())
        assert birth(identity[0]) == identity[1]
        driver.click("#workspace-attach")
        wait_for(lambda: read_state()["writable"], "reopened app did not recover the same Workspace terminal")
        shows("WORKSPACE_REPLY:from-aether")
        driver.type("#workspace-input", "exit-fixture" + "\ue007")
        wait_for(lambda: "exited with status 7" in read_state()["status"], "native Workspace exit receipt missing")
        assert not read_state()["writable"]
        shows("WORKSPACE_LAST_OUTPUT")
        assert received.read_text().splitlines() == [
            "from-existing-terminal", "legacy-query-fixture", "detached-query-fixture",
            "from-aether", "query-fixture", "paste-fixture", "exit-fixture",
        ]
        assert trace.read_text().splitlines() == starts
        print(json.dumps({
            "native_workspace_shared_schema_6_catalog": True,
            "native_workspace_duplicate_uuid_preserves_source": True,
            "native_workspace_conflicting_cli_lease_refused": True,
            "native_workspace_reopens_existing_provider_pty": True,
            "native_workspace_provider_input_and_exit_once": True,
            "native_workspace_protocol_reply_bytes": True,
            "native_workspace_reviewed_paste_matches_exact_pty_bytes": True,
            "native_workspace_final_output_before_exit": True,
            "native_workspace_detached_host_answers_wide_viewport": True,
            "native_workspace_query_split_across_detach": True,
            "native_workspace_legacy_view_answers_without_duplicates": True,
            "native_workspace_removes_only_the_confirmed_profile": True,
        }), flush=True)
        print(json.dumps({
            "case": "PRODUCT.CLI.001.NATIVE.001",
            "backend": env["GDK_BACKEND"],
            "result": "passed",
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        }), flush=True)
    except Exception:
        print("Workspace fixture:", log.read_text()[-4000:], read_state(), flush=True)
        print("Workspace helper:", helper.poll() if helper is not None else None,
              bytes(helper_output[-2000:]), flush=True)
        raise
    finally:
        if helper is not None and helper.poll() is None:
            stop_driver(helper)
        if master is not None:
            os.close(master)
        if identity and birth(identity[0]) == identity[1]:
            args = Path(f"/proc/{identity[0]}/cmdline")
            if args.is_file() and os.fsencode(owned / "bin" / "codex") in args.read_bytes():
                with contextlib.suppress(ProcessLookupError):
                    os.kill(identity[0], signal.SIGTERM)
        stop_driver(host)


def interrupted(_signum, _frame):
    # Let the outer timeout take the same cleanup path as an ordinary failure.
    raise TimeoutError("native WebKit smoke interrupted")


def workspace_command(socket_file, kind, values):
    request = {"Command": {"version": 2, "command": kind if values is None else {kind: values}}}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(str(socket_file))
        stream.sendall(json.dumps(request).encode() + b"\n")
        with stream.makefile("rb") as reader:
            return json.loads(reader.readline(12 * 1024 * 1024 + 1))["Reply"]


@contextlib.contextmanager
def api_provider(env):
    requests = []
    gate = threading.Event()
    cancel_gate = threading.Event()
    first = b'{"message":{"role":"assistant","content":"First streamed segment. "},"done":false}\n'
    last = b'{"message":{"role":"assistant","content":"Final durable segment."},"done":true}\n'

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            length = int(self.headers["Content-Length"])
            if self.path != "/api/chat" or not 0 < length <= 1024 * 1024:
                self.send_error(400)
                return
            requests.append(json.loads(self.rfile.read(length)))
            release = gate if len(requests) == 1 else cancel_gate
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.send_header("Content-Length", str(len(first) + len(last)))
            self.end_headers()
            try:
                self.wfile.write(first)
                self.wfile.flush()
                if release.wait(timeout=55):
                    self.wfile.write(last)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    env.update({
        "AETHER_AI_PROVIDER": "ollama",
        "AETHER_AI_BASE_URL": f"http://127.0.0.1:{server.server_port}",
        "AETHER_AI_MODEL": "isolated-aether-fixture",
        "AETHER_AI_MAX_TOKENS": "64",
    })
    try:
        yield requests, gate, cancel_gate
    finally:
        gate.set()
        cancel_gate.set()
        server.shutdown()
        server.server_close()
        worker.join(timeout=3)


def exercise_api_chat(driver, owned, binary, env, fixture):
    requests, gate, cancel_gate = fixture
    state = Path(env["JTERM_WORKSPACE_STATE_DIR"])
    with (owned / "api-host.log").open("wb") as output:
        host = subprocess.Popen([binary, "--jterm-session-host"], cwd=owned, env=env,
                                stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                start_new_session=True)
    socket_file = state / "run" / "host.sock"

    def command(kind, values):
        return workspace_command(socket_file, kind, values)

    def read():
        return driver.execute("""
            return {
              status: document.querySelector('#chat-status').textContent,
              job: document.querySelector('#chat-job-status').textContent,
              draft: document.querySelector('#chat-draft').value,
              preview: document.querySelector('#chat-preview').textContent,
              history: Array.from(document.querySelectorAll('#chat-history pre'), el=>el.textContent),
              options: Array.from(document.querySelector('#chat-select').options, el=>el.value),
              canOpen: !document.querySelector('#chat-open').disabled,
              view: !document.querySelector('#chat-view').hidden,
              canCancel: !document.querySelector('#chat-cancel').disabled,
              canSend: !document.querySelector('#chat-send').disabled,
            };
        """)

    def show_chat():
        for selector in ["#agent-workspace", "#workspace-api"]:
            if not driver.execute(f"return document.querySelector('{selector}').open;"):
                driver.click(f"{selector} > summary")
        wait_for(lambda: driver.execute("return !document.querySelector('#chat-new').disabled;"),
                 "API conversation controls did not load")

    def reopen():
        driver.click("#chat-refresh")
        def ready_to_open():
            chat = read()
            return chat["canOpen"] and conversation in chat["options"]

        wait_for(ready_to_open, "the shared API conversation was lost")
        driver.click("#chat-open")
        wait_for(lambda: read()["view"], "the shared API conversation did not reopen")

    first_draft = "first native request\nspanning two lines"
    newer_draft = "newer saved draft\nsurvives background completion"
    try:
        def host_ready():
            if host.poll() is not None:
                raise RuntimeError("the isolated API Workspace host exited")
            try:
                return "Ok" in command("CatalogPage", {"before": None, "limit": 1})
            except (OSError, KeyError, ValueError):
                return False

        wait_for(host_ready, "the API Workspace host did not bind")
        show_chat()
        driver.click("#chat-new")
        wait_for(lambda: read()["view"], "the shared API conversation was not created")
        page = command("CatalogPage", {"before": None, "limit": 32})["Ok"]["CatalogPage"]
        conversation = next(row["id"] for row in page["rows"] if row["kind"] == "Api")
        driver.type("#chat-draft", first_draft.replace("\n", "\ue007"))
        assert read()["draft"] == first_draft, repr(read()["draft"])
        driver.click("#chat-save")
        wait_for(lambda: read()["status"] == "Draft saved.", "initial draft was not committed")
        initial = command("LoadChat", {"id": conversation})["Ok"]["Chat"]["chat"]
        driver.click("#chat-send")
        wait_for(lambda: "First streamed segment." in read()["preview"], "initial API preview did not stream")
        assert len(requests) == 1 and requests[0]["stream"] is True and read()["canCancel"]
        assert requests[0]["messages"][-1]["content"] == first_draft, requests[0]["messages"]
        assert read()["draft"] == "" and not read()["canSend"]
        assert "another live view" in command("AttachApi", {
            "id": conversation, "epoch": initial["epoch"] + 1, "view": "competing-native-fixture",
        })["Err"].lower()

        driver.type("#chat-draft", newer_draft.replace("\n", "\ue007"))
        assert read()["draft"] == newer_draft, repr(read()["draft"])
        driver.click("#chat-save")
        wait_for(lambda: read()["status"] == "Draft saved.", "newer draft was not committed")
        active = command("LoadChat", {"id": conversation})["Ok"]["Chat"]["chat"]
        assert active["draft_revision"] > initial["draft_revision"]
        assert "revision conflict" in command("SetDraft", {
            "id": conversation, "expected_revision": initial["draft_revision"], "text": "STALE",
        })["Err"]
        assert "already has a request in flight" in command("BeginSubmission", {
            "id": conversation, "expected_revision": active["revision"],
            "draft_revision": active["draft_revision"], "text": newer_draft,
        })["Err"]
        assert len(requests) == 1 and not read()["canSend"]

        driver.click("#chat-detach")
        wait_for(lambda: not read()["view"] and read()["status"] == "Conversation detached.",
                 "API view detach was not acknowledged")
        reopen()
        wait_for(lambda: "First streamed segment." in read()["preview"] and read()["canCancel"],
                 "detached API execution did not reattach")
        assert read()["draft"] == newer_draft
        driver.close()
        driver.open(binary)
        wait_for(lambda: opened_page(driver), "Aether did not reopen during API streaming")
        show_chat()
        reopen()
        wait_for(lambda: "First streamed segment." in read()["preview"] and read()["canCancel"],
                 "reopened native API view did not recover the same stream")
        assert read()["draft"] == newer_draft and len(requests) == 1
        gate.set()
        answer = "First streamed segment. Final durable segment."
        wait_for(lambda: read()["history"] == [first_draft, answer], "completed API turn was not loaded from SQLite")
        assert read()["draft"] == newer_draft and len(requests) == 1 and not read()["canCancel"]
        driver.click("#chat-detach")
        wait_for(lambda: not read()["view"], "completed API conversation did not detach")
        reopen()
        assert read()["history"] == [first_draft, answer] and read()["draft"] == newer_draft
        driver.click("#chat-send")
        wait_for(lambda: len(requests) == 2 and "First streamed segment." in read()["preview"],
                 "second explicit submission did not reach the host")
        assert requests[1]["messages"][-1]["content"] == newer_draft and read()["canCancel"]
        driver.click("#chat-cancel")
        wait_for(lambda: read()["job"] == "Request cancelled." and read()["draft"] == newer_draft,
                 "cancelled API request did not restore its draft after cleanup")
        assert read()["history"] == [first_draft, answer] and len(requests) == 2
        print(json.dumps({
            "native_workspace_api_shared_draft_and_durable_history": True,
            "native_workspace_api_preview_survives_app_close": True,
            "native_workspace_api_single_submission_and_control_conflict": True,
            "native_workspace_api_newer_draft_survives_completion": True,
            "native_workspace_api_explicit_cancel_restores_draft": True,
        }), flush=True)
    except Exception:
        print("API fixture:", (owned / "api-host.log").read_text()[-4000:], read())
        raise
    finally:
        gate.set()
        cancel_gate.set()
        stop_driver(host)


def prepare_native_task(owned, env, launch):
    node = shutil.which("node", path=os.defpath) or shutil.which("node")
    if node is None:
        raise RuntimeError("the native Codex fixture requires Node.js")
    # Give the fixture one interpreter in its private PATH. The native host
    # still admits the canonical interpreter and the installed system Git.
    (launch / "node").symlink_to(Path(node).resolve(strict=True))
    profile = Path(env.setdefault("CODEX_HOME", str(owned / "native-profile")))
    profile.mkdir(mode=0o700, exist_ok=True)
    profile.chmod(0o700)
    auth = profile / "auth.json"
    auth.write_text(json.dumps({
        "auth_mode": "chatgpt",
        "tokens": {"access_token": "native-fixture-placeholder", "account_id": "isolated-fixture"},
    }))
    auth.chmod(0o600)
    executable = launch / "codex"
    if executable.exists():
        executable.rename(launch / "codex-cli-fixture")
    fixture = launch / "native-codex-fixture.py"
    shutil.copyfile(Path(__file__).with_name("native-codex-fixture.py"), fixture)
    executable.write_text("""#!/usr/bin/env node
const {spawnSync}=require('node:child_process');
const {join}=require('node:path');
const child=spawnSync('/usr/bin/python3',[join(__dirname,'native-codex-fixture.py'),...process.argv.slice(2)],{stdio:'inherit'});
process.exit(child.status ?? 1);
""")
    executable.chmod(0o700)
    repository = owned / "native-task-repository"
    repository.mkdir()
    git_env = {"HOME": str(owned), "PATH": "/usr/bin:/bin", "LC_ALL": "C.UTF-8", "GIT_CONFIG_NOSYSTEM": "1"}

    def git(*args):
        return subprocess.check_output(["/usr/bin/git", *args], cwd=repository, env=git_env, timeout=10, text=True)

    git("init", "--quiet")
    (repository / "tracked.txt").write_text("original native fixture baseline\n")
    git("add", "tracked.txt")
    git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture")
    env.setdefault("JTERM_WORKSPACE_STATE_DIR", str(owned / "workspace"))
    return repository, git("rev-parse", "HEAD").strip()


def exercise_native_tools(driver, command, task, snapshot, starts_file):
    marker = "AETHER_VERIFICATION_PASSED"
    validation = "validation:" + snapshot["validation"]["run_id"]
    assert re.fullmatch(r"validation:validation-[0-9a-f-]+", validation)
    # Later indexed imports give the real validation record an older page and
    # a replaceable hit for the revision fence. They are separate evidence.
    imported = [{
        "conversation": task,
        "event_id": f"external:aether-tools-{index}",
        "revision": "r1",
        "source": "ImportedHistory",
        "message_sequence": None,
        "text": f"Imported reader fixture {index}, search term {marker}",
        "truncated": False,
    } for index in range(32)]
    assert command("IndexText", {"documents": imported}) == {"Ok": "Ack"}
    starts = starts_file.read_text().splitlines()

    def view():
        return driver.execute("""
            const record = document.querySelector('#tools-document-identity');
            return {
              status: document.querySelector('#tools-status').textContent,
              hits: Array.from(document.querySelectorAll('#tools-results button'), button=>button.dataset.eventId),
              older: !document.querySelector('#tools-more').disabled,
              scope: !document.querySelector('#tools-scope').disabled,
              opened: !document.querySelector('#tools-document').hidden,
              text: document.querySelector('#tools-document-text').textContent,
              source: document.querySelector('#tools-document-source').textContent,
              anchor: [record.dataset.conversation, record.dataset.eventId, record.dataset.revision],
            };
        """)

    driver.click("#workspace-tools > summary")
    wait_for(lambda: view()["scope"], "Tools did not recognize the current native task")
    driver.click("#tools-scope")
    driver.type("#tools-query", marker)
    driver.click("#tools-submit")
    wait_for(lambda: (page := view())["older"] and len(page["hits"]) == 30, "Tools did not open the bounded evidence index")
    assert validation not in view()["hits"]
    changed = {**imported[-1], "revision": "r2", "text": f"Updated imported record, search term {marker}"}
    assert command("IndexText", {"documents": [changed]}) == {"Ok": "Ack"}
    driver.click('#tools-results button[data-event-id="external:aether-tools-31"]')
    wait_for(lambda: "Search again" in view()["status"], "Tools opened a replaced evidence revision")
    assert not view()["opened"]

    driver.click("#tools-more")
    wait_for(lambda: validation in view()["hits"], "Tools did not page to the real native validation")
    driver.click(f'#tools-results button[data-event-id="{validation}"]')
    wait_for(lambda: (record := view())["opened"] and marker in record["text"], "actual validation evidence was not opened")
    displayed = view()
    conversation, event_id, revision = displayed["anchor"]
    assert conversation == task and event_id == validation and revision
    exact = command("ReadSearchDocument", {
        "conversation": conversation, "event_id": event_id, "revision": revision,
    })["Ok"]["SearchDocument"]["document"]
    assert displayed["source"] == "Validation evidence" and exact["source"] == "Validation"
    assert displayed["text"] == exact["text"] and "Status: Passed" in displayed["text"]
    driver.clear("#tools-query")
    wait_for(lambda: not view()["opened"] and not view()["hits"], "editing the query did not retire old evidence")
    assert starts_file.read_text().splitlines() == starts


def exercise_native_mission(driver, owned, command, reopen_task, task, snapshot, starts_file):
    def view():
        return driver.execute("""
            return {
              status: document.querySelector('#mission-status').textContent,
              plan: document.querySelector('#mission-repositories').textContent,
              goal: document.querySelector('#mission-edit-goal').value,
              evidence: document.querySelector('#mission-evidence').textContent,
              phase: document.querySelector('#mission-evidence-state').textContent,
              stale: Boolean(document.querySelector('#mission-evidence [data-check="stale"]')),
              open: !document.querySelector('#mission-open').disabled,
              create: !document.querySelector('#mission-new').disabled,
              freeze: !document.querySelector('#mission-freeze').disabled,
              expanded: document.querySelector('#workspace-mission').open,
            };
        """)

    def read():
        return command("ReadMission", {"id": mission_id})["Ok"]["Mission"]

    def show_missions():
        def expand():
            if not view()["expanded"]:
                driver.click("#workspace-mission > summary")
            return view()["expanded"]

        wait_for(expand, "Missions summary did not expand")
        try:
            wait_for(lambda: (current := view())["create"] and current["status"] in (
                "Mission catalog refreshed.", "No saved Missions."
            ), "Missions did not recognize the selected native task")
        except AssertionError:
            print("Missions fixture:", view(), flush=True)
            raise

    show_missions()
    title = "Deliver the isolated fixture"
    goal = "Observe the exact worktree and its current verification receipt."
    name = "example/aether-native-mission-fixture"
    driver.type("#mission-title", title)
    driver.type("#mission-goal", goal)
    driver.type("#mission-repository", name)
    driver.clear("#mission-target")
    # The host's strict remote ref admission keeps this proof offline while
    # still doing real local Git and task validation inspection.
    driver.type("#mission-target", "refs/heads/-offline-fixture")
    driver.click("#mission-new")
    wait_for(lambda: "Mission saved · revision 1" in view()["status"], "the Mission plan was not saved")
    saved, = command("MissionList", None)["Ok"]["Missions"]["missions"]
    mission_id = saved["id"]
    repository, = saved["repositories"]
    assert repository["path"] == snapshot["task"]["worktree_path"]
    assert repository["baseline"] == snapshot["task"]["base_commit"]
    link, = repository["tasks"]
    assert link["conversation"] == task and link["task_id"] == snapshot["task"]["id"]
    assert link["run_id"] == snapshot["incarnation"] and link["source_sequence"] >= snapshot["sequence"]
    assert snapshot["incarnation"] in view()["plan"]
    assert read()["report"] is None

    replacement = "Changed shared acceptance goal."
    changed = {**saved, "goal": replacement}
    external = command("SaveMission", {"expected_revision": 1, "mission": changed})
    assert external["Ok"]["Mission"]["mission"]["revision"] == 2
    driver.type("#mission-edit-goal", " Keep this attempted edit.")
    driver.click("#mission-save")
    wait_for(lambda: (current := view())["open"] and any(
        detail in current["status"] for detail in ("changed in another", "Saved revision reloaded")
    ), "the conflicting Mission save did not finish with reopening available")
    assert view()["goal"].endswith("Keep this attempted edit.") and read()["mission"]["goal"] == replacement
    driver.click("#mission-open")
    wait_for(lambda: view()["goal"] == replacement and "revision 2" in view()["status"],
             "reopening the Mission did not load the shared acceptance goal")

    driver.click("#mission-inspect")
    wait_for(lambda: "Current inspection" in view()["phase"] and view()["stale"],
             "host did not inspect actual local worktree and stale validation evidence")
    assert "uncommitted changes" in view()["evidence"] and not view()["freeze"]
    historical = read()["report"]["repositories"][0]
    assert historical["observed"]["head"] == repository["baseline"] and historical["observed"]["dirty"]

    worktree = Path(repository["path"])
    git_env = {"HOME": str(owned), "PATH": "/usr/bin:/bin", "LC_ALL": "C.UTF-8", "GIT_CONFIG_NOSYSTEM": "1"}

    def git(*args):
        return subprocess.check_output(["/usr/bin/git", *args], cwd=worktree, env=git_env, timeout=10, text=True)

    git("add", "--", "tracked.txt")
    git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "Mission candidate")
    candidate = git("rev-parse", "HEAD").strip()
    assert candidate != repository["baseline"]
    driver.click("#mission-inspect")
    wait_for(lambda: view()["freeze"] and candidate in view()["evidence"],
             "the host did not observe the committed fixture candidate")
    driver.click("#mission-freeze")
    wait_for(lambda: "Candidates frozen · revision 3" in view()["status"], "the observed candidate was not frozen")
    assert read()["mission"]["repositories"][0]["candidate"] == candidate
    driver.click("#mission-inspect")
    wait_for(lambda: "Current inspection" in view()["phase"] and "Commit verified" in view()["evidence"],
             "Mission candidate commit was not independently verified")
    assert view()["stale"] and "Push unverified" in view()["evidence"]
    assert len(starts_file.read_text().splitlines()) == 1

    reopen_task(task)
    show_missions()
    wait_for(lambda: view()["open"], "the saved Mission was absent after restart")
    driver.click("#mission-open")
    wait_for(lambda: "Saved observation" in view()["phase"], "Mission evidence did not reopen as historical")
    assert not view()["freeze"] and "Commit pending" in view()["evidence"] and candidate in view()["plan"]
    assert len(starts_file.read_text().splitlines()) == 1


def exercise_native_context(driver, owned, command, reopen_task, task, starts_file, repository):
    def view():
        return driver.execute("""
            return {
              contextStatus: document.querySelector('#native-context-status').textContent,
              canReloadContext: !document.querySelector('#native-context-reload').disabled,
              taskStatus: document.querySelector('#native-status').textContent,
              goal: document.querySelector('#native-context-goal').value,
              completed: document.querySelector('#native-context-completed').value,
              remaining: document.querySelector('#native-context-remaining').value,
              constraints: document.querySelector('#native-context-constraints').value,
              materials: document.querySelector('#native-context-materials').textContent,
              canEdit: !document.querySelector('#native-context-goal').disabled,
              canUse: !document.querySelector('#native-context-use').disabled,
              packet: document.querySelector('#native-context-body').textContent,
              draft: document.querySelector('#native-goal').value,
              repository: document.querySelector('#native-repository').value,
              consent: document.querySelector('#native-consent').checked,
              canStart: !document.querySelector('#native-start').disabled,
              hasApproval: Boolean(document.querySelector('#native-approvals button:not(:disabled)')),
              canInspect: !document.querySelector('#native-inspect').disabled,
            };
        """)

    def shared_context():
        return command("ReadContext", {"id": task})["Ok"]["Context"]["context"]

    def edit_shared(**fields):
        context = shared_context()
        context.update(fields)
        return command("SaveContext", {
            "id": task, "expected_revision": context["revision"], "context": context,
        })["Ok"]["Context"]["context"]

    def show_context():
        driver.click("#native-context > summary")
        wait_for(lambda: view()["canEdit"], "the selected native task context did not load")

    show_context()
    context_goal = "Review the fixture approval and pass the reviewed findings to a follow-up."
    local_fixture_secret = "sk-artificialAetherContextPacketFixture000000001"
    driver.type("#native-context-goal", context_goal)
    driver.type("#native-context-completed", "Finished the original verification.")
    driver.type("#native-context-remaining", "Check the final scope.")
    driver.type("#native-context-constraints", "Keep the current repository.")
    driver.type("#native-context-note", f"Reviewed evidence with synthetic key {local_fixture_secret}")
    driver.click("#native-context-save")
    wait_for(lambda: "saved at revision 1" in view()["contextStatus"], "authored context was not saved")
    original = shared_context()
    assert original["goal"] == context_goal and original["revision"] == 1
    assert original["materials"][0]["kind"] == "Note"
    assert local_fixture_secret not in view()["materials"] and "[REDACTED:openai-key]" in view()["materials"]
    assert local_fixture_secret not in original["materials"][0]["text"]

    reopen_task(task)
    wait_for(lambda: view()["canInspect"], "source native task did not reopen for context")
    show_context()
    assert view()["goal"] == context_goal and view()["completed"] == original["completed"]
    assert edit_shared(constraints="New constraint from a second client.")["revision"] == 2
    driver.type("#native-context-remaining", " Keep this local unsaved edit.")
    driver.click("#native-context-save")
    wait_for(lambda: "Shared context changed" in view()["contextStatus"], "stale desktop context write was accepted")
    assert view()["remaining"].endswith("Keep this local unsaved edit.")
    assert shared_context()["remaining"] == original["remaining"]
    driver.click("#native-context-reload")
    wait_for(lambda: view()["constraints"] == "New constraint from a second client.",
             "explicit reload did not read the second client's saved context")
    driver.click("#native-context-preview")
    wait_for(lambda: view()["canUse"], "the host did not build a reviewed Codex handoff")
    assert context_goal in view()["packet"] and "Source materials revision: 2" in view()["packet"]
    assert edit_shared(remaining="Latest remaining work from the shared host.")["revision"] == 3
    driver.click("#native-context-use")
    wait_for(lambda: "Materials changed" in view()["contextStatus"], "changed packet opened a stale task draft")
    assert view()["draft"] == "" and view()["canInspect"]
    assert len(starts_file.read_text().splitlines()) == 1

    wait_for(lambda: view()["canReloadContext"], "the saved context reload is still busy")
    # Activate this button by keyboard to exercise it after the preview folds
    # and moves the control on screen.
    driver.type("#native-context-reload", "\ue007")
    wait_for(lambda: (current := view())["remaining"] == "Latest remaining work from the shared host."
             and "revision 3" in current["contextStatus"], "latest context revision did not reload")
    driver.click("#native-context-preview")
    wait_for(lambda: view()["canUse"], "fresh Codex handoff did not become available")
    packet = view()["packet"]
    assert context_goal in packet and "Source materials revision: 3" in packet
    assert "Latest remaining work from the shared host." in packet and "Material: Task note (Note)" in packet
    assert "[REDACTED:openai-key]" in packet and local_fixture_secret not in packet
    driver.click("#native-context-use")
    wait_for(lambda: "New task draft prepared" in view()["taskStatus"], "handoff did not open the native task composer")
    draft = view()
    assert draft["draft"] == packet and draft["repository"] == str(repository)
    assert not draft["consent"] and not draft["canStart"] and not draft["hasApproval"]
    entries = command("NativeCatalog", {"after": None, "limit": 4})["Ok"]["NativeCatalog"]["entries"]
    assert [entry["id"] for entry in entries] == [task]
    assert len(starts_file.read_text().splitlines()) == 1

    driver.clear("#native-title")
    driver.type("#native-title", "Explicit context follow-up")
    driver.click("#native-consent")
    driver.click("#native-start")
    wait_for(lambda: view()["hasApproval"], "explicit handoff draft did not start a new native task", timeout=20)
    entries = command("NativeCatalog", {"after": None, "limit": 4})["Ok"]["NativeCatalog"]["entries"]
    assert len(entries) == 2
    followup = next(entry["id"] for entry in entries if entry["id"] != task)
    original_task = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]
    new_task = command("InspectNative", {"id": followup})["Ok"]["Native"]["snapshot"]
    first, second = [json.loads(line) for line in starts_file.read_text().splitlines()]
    assert first["pid"] != second["pid"] and first["thread"] != second["thread"]
    assert second["cwd"] == new_task["task"]["worktree_path"] != first["cwd"]
    assert new_task["task"]["brief"]["goal"] == packet.strip() and new_task["task"]["brief"]["validation"] is None
    last_prompt = json.loads((owned / "native-task-prompts").read_text().splitlines()[-1])
    assert last_prompt["pid"] == second["pid"] and last_prompt["brief"]["goal"] == packet.strip()
    assert original_task["task"]["status"] == "completed"
    provider_birth = birth(second["pid"])
    assert provider_birth is not None
    return followup, Path(second["cwd"]), second["pid"], provider_birth


def exercise_native_manual_acceptance(driver, command, reopen_task, followup, starts_file):
    task, worktree, pid, provider_birth = followup
    starts = starts_file.read_text().splitlines()

    def view():
        return driver.execute("""
            return {
              status: document.querySelector('#native-status').textContent,
              lifecycle: document.querySelector('#native-lifecycle').textContent,
              canFinish: !document.querySelector('#native-finish').disabled,
              canInspect: !document.querySelector('#native-inspect').disabled,
              canReview: !document.querySelector('#native-accept-reviewed').disabled,
              canAccept: !document.querySelector('#native-accept-verified').disabled,
              result: !document.querySelector('#native-result').hidden,
              diff: document.querySelector('#native-diff').textContent,
            };
        """)

    assert not view()["canReview"] and not view()["canAccept"]
    driver.click("#native-approvals button")
    wait_for(lambda: view()["canFinish"], "the no-recipe provider did not reach review")
    driver.click("#native-finish")
    wait_for(lambda: (state := view())["canInspect"] and state["lifecycle"] == "Ready for review"
             and birth(pid) != provider_birth, "the manual-review provider did not finish")
    assert not view()["canReview"]
    driver.click("#native-inspect")
    wait_for(lambda: view()["canReview"], "inspection did not enable exact human review")
    assert not view()["canAccept"] and "edited inside the native fixture worktree" in view()["diff"]

    with (worktree / "tracked.txt").open("a") as tracked:
        tracked.write("Changed after human inspection\n")
    driver.click("#native-accept-reviewed")
    wait_for(lambda: (state := view())["status"].startswith("Worktree changed after inspection")
             and not state["result"], "manual acceptance did not reject the changed worktree")
    snapshot = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]
    assert snapshot["task"]["status"] == "ready_for_review" and not view()["canReview"]

    driver.click("#native-inspect")
    wait_for(lambda: (state := view())["canReview"] and "Changed after human inspection" in state["diff"],
             "freshly inspected diff was not offered for human acceptance")
    driver.click("#native-accept-reviewed")
    wait_for(lambda: view()["lifecycle"] == "Completed", "explicit human acceptance was not stored")
    accepted = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]
    assert accepted["incarnation"] == snapshot["incarnation"] and accepted["task"]["status"] == "completed"
    assert accepted["task"]["validation"]["status"] == "not_run" and accepted["validation"] is None
    reopen_task(task)
    wait_for(lambda: view()["lifecycle"] == "Completed", "manual acceptance did not reopen")
    assert starts_file.read_text().splitlines() == starts


def exercise_native_queue(driver, command, reopen_task, accepted_task, repository, starts_file):
    starts = starts_file.read_text().splitlines()

    def catalog():
        return command("NativeCatalog", {"after": None, "limit": 16})["Ok"]["NativeCatalog"]["entries"]

    def snapshot(task):
        return command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]

    def saved_queue():
        return command("QueueSnapshot", None)["Ok"]["Queue"]["snapshot"]

    def view():
        return driver.execute("""
            const deps=document.querySelector('#queue-dependencies');
            return {
              draft: document.querySelector('#native-draft-goal').textContent,
              status: document.querySelector('#queue-status').textContent,
              rows: Object.fromEntries(Array.from(document.querySelectorAll('#queue-entries li'),
                item=>[item.dataset.task,item.textContent])),
              editing: !deps.disabled,
              options: Array.from(deps.options,option=>option.value),
              selected: Array.from(deps.selectedOptions,option=>option.value),
              start: !document.querySelector('#queue-launch').disabled,
              confirm: !document.querySelector('#queue-confirm').disabled,
              pending: Boolean(document.querySelector('#native-approvals button:not(:disabled)')),
              create: !document.querySelector('#native-save-draft').disabled,
            };
        """)

    def new_draft(title, goal):
        previous = {entry["id"] for entry in catalog()}
        driver.click("#native-detach")
        wait_for(lambda: view()["create"], "native draft composer did not open")
        driver.type("#native-repository", str(repository))
        driver.type("#native-title", title)
        driver.type("#native-goal", goal)
        driver.click("#native-save-draft")
        wait_for(lambda: view()["draft"] == goal, "the saved draft was not attached")
        created, = {entry["id"] for entry in catalog()} - previous
        saved = snapshot(created)
        assert saved["saved_draft"] and not saved["live"] and not saved["preparing"]
        assert saved["task"]["brief"]["goal"] == goal and not view()["pending"]
        assert starts_file.read_text().splitlines() == starts
        return created

    def show_queue():
        driver.click("#workspace-queue > summary")
        wait_for(lambda: view()["editing"], "the shared queue did not open for the exact draft")

    def choose_dependency(task):
        choices = view()["options"]
        assert task in choices
        driver.type("#queue-dependencies", "\ue011" + "\ue015" * choices.index(task))
        wait_for(lambda: view()["selected"] == [task], "queue dependency selection did not settle")

    blocker = new_draft("Unstarted queue dependency", "Review the fixture approval at a later stage.")
    goal = "Review the fixture approval only after the queued dependency is accepted."
    queued = new_draft("Durable queued fixture", goal)
    show_queue()
    choose_dependency(blocker)
    driver.click("#queue-save")
    wait_for(lambda: "Waiting for accepted dependencies" in view()["rows"].get(queued, ""),
             "the host queue did not block the saved draft on its dependency")
    assert not view()["start"] and not view()["confirm"]
    first = saved_queue()
    assert first["entries"][0]["intent"]["dependencies"] == [blocker]

    other = command("QueueUpdate", {"update": {
        "expected_revision": first["revision"],
        "change": {"Upsert": {"intent": {"id": blocker, "dependencies": [], "location": {"kind": "local"}}}},
    }})["Ok"]["Queue"]["snapshot"]
    choose_dependency(accepted_task)
    driver.click("#queue-save")
    wait_for(lambda: "changed in another view" in view()["status"], "a stale queue revision overwrote another view")
    assert view()["selected"] == [accepted_task] and saved_queue()["revision"] == other["revision"]
    assert starts_file.read_text().splitlines() == starts

    reopen_task(queued)
    wait_for(lambda: view()["draft"] == goal, "the exact unstarted draft did not persist")
    show_queue()
    assert view()["selected"] == [blocker]
    choose_dependency(accepted_task)
    driver.click("#queue-save")
    wait_for(lambda: (state := view())["confirm"]
             and "Ready to start explicitly" in state["rows"].get(queued, ""),
             "accepted predecessor did not make the draft ready")
    before = snapshot(queued)
    assert before["saved_draft"] and not before["live"] and not view()["start"]
    assert starts_file.read_text().splitlines() == starts
    driver.click("#queue-confirm")
    driver.click("#queue-launch")
    wait_for(lambda: view()["pending"], "explicit queued launch did not reach the scoped provider", timeout=20)
    after = snapshot(queued)
    assert after["id"] == before["id"] and after["incarnation"] == before["incarnation"]
    assert not after["saved_draft"] and after["live"] and not view()["start"]
    receipts = starts_file.read_text().splitlines()
    assert len(receipts) == len(starts) + 1 and receipts[:-1] == starts
    active = json.loads(receipts[-1])
    assert active["cwd"] == after["task"]["worktree_path"]
    assert snapshot(blocker)["saved_draft"]
    started = birth(active["pid"])
    driver.click("#native-stop")
    wait_for(lambda: birth(active["pid"]) != started, "queued task provider was not reaped")


def exercise_native_task(driver, owned, binary, env, fixture):
    repository, baseline = fixture
    runtime = Path("/run/user") / str(os.getuid())
    bus = runtime / "bus"
    if not bus.is_socket():
        raise RuntimeError("native approval smoke needs a real user systemd bus")
    # The desktop remains on its private display/session bus. Only the native
    # host connects to the user's systemd manager for its transient scope.
    host_env = {**env, "XDG_RUNTIME_DIR": str(runtime), "DBUS_SESSION_BUS_ADDRESS": f"unix:path={bus}"}
    log = owned / "native-task-host.log"
    with log.open("wb") as output:
        host = subprocess.Popen([binary, "--jterm-session-host"], cwd=owned, env=host_env,
                                stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                start_new_session=True)
    socket_file = Path(env["JTERM_WORKSPACE_STATE_DIR"]) / "run" / "host.sock"
    starts_file = owned / "native-provider-starts"
    receipts = owned / "native-approval-receipts"
    verification_count = owned / "native-verification-count"
    verification_gate = owned / "native-verification-gate"
    gate_path = shlex.quote(str(verification_gate))
    recipe = (
        f"printf 'run\\n' >> {shlex.quote(str(verification_count))}; "
        "printf 'AETHER_VERIFICATION_STARTED\\n'; "
        f"for step in $(seq 1 360); do test -f {gate_path} && break; sleep .05; done; "
        f"test -f {gate_path} && grep -q 'edited inside the native fixture worktree' tracked.txt "
        "&& printf 'AETHER_VERIFICATION_PASSED\\n'"
    )
    provider_identity = None

    def status():
        return driver.execute("""
            return {
              status: document.querySelector('#native-status').textContent,
              detail: document.querySelector('#native-detail').textContent,
              lifecycle: document.querySelector('#native-lifecycle').textContent,
              worktree: document.querySelector('#native-worktree').textContent,
              output: document.querySelector('#native-output').textContent,
              approval: document.querySelector('#native-approvals').textContent,
              hasApproval: Boolean(document.querySelector('#native-approvals button:not(:disabled)')),
              finish: !document.querySelector('#native-finish').disabled,
              canOpen: !document.querySelector('#native-open').disabled,
              canInspect: !document.querySelector('#native-inspect').disabled,
              canVerify: !document.querySelector('#native-verify').disabled,
              canAccept: !document.querySelector('#native-accept-verified').disabled,
              verification: document.querySelector('#native-verification-status').textContent,
              verificationOutput: document.querySelector('#native-verification-output').textContent,
              inspectionValidation: document.querySelector('#native-validation').textContent,
              options: Array.from(document.querySelector('#native-select').options, option=>option.value),
              diff: document.querySelector('#native-diff').textContent,
            };
        """)

    def show_tasks():
        for selector in ["#agent-workspace", "#workspace-native"]:
            if not driver.execute(f"return document.querySelector('{selector}').open;"):
                driver.click(f"{selector} > summary")
        wait_for(lambda: driver.execute("return !document.querySelector('#native-refresh').disabled;"),
                 "native task catalog did not load")

    def command(kind, values):
        return workspace_command(socket_file, kind, values)

    def reopen_task(task):
        driver.close()
        driver.open(binary)
        wait_for(lambda: opened_page(driver), "Aether did not reopen for the native task")
        show_tasks()
        wait_for(lambda: (view := status())["canOpen"] and task in view["options"],
                 "shared native task was absent after reopening")
        selected = driver.execute("return document.querySelector('#native-select').value;")
        if selected != task:
            row = status()["options"].index(task)
            driver.type("#native-select", "\ue011" + "\ue015" * row)
            wait_for(lambda: driver.execute("return document.querySelector('#native-select').value;") == task,
                     "could not select the exact saved native task")
        driver.click("#native-open")

    def host_ready():
        if host.poll() is not None:
            raise RuntimeError("the isolated native task host exited")
        try:
            return "Ok" in command("NativeCatalog", {"after": None, "limit": 1})
        except (OSError, KeyError, ValueError):
            return False

    try:
        wait_for(host_ready, "native task host did not bind")
        show_tasks()
        assert driver.execute("return document.querySelector('#native-start').disabled;")
        driver.type("#native-repository", str(repository))
        driver.type("#native-title", "Isolated native approval fixture")
        driver.type("#native-goal", "Review the fixture approval")
        driver.type("#native-verification-recipe", recipe)
        driver.click("#native-consent")
        driver.click("#native-start")
        wait_for(lambda: status()["hasApproval"], "the scoped Codex approval did not reach Aether", timeout=20)
        presented = status()
        assert "printf fixture-only" in presented["approval"] and "pending request" in presented["output"]
        assert presented["lifecycle"] == "Waiting for approval"
        assert not presented["canVerify"] and not verification_count.exists()
        entries = command("NativeCatalog", {"after": None, "limit": 4})["Ok"]["NativeCatalog"]["entries"]
        assert len(entries) == 1
        task = entries[0]["id"]
        snapshot = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]
        worktree = Path(snapshot["task"]["worktree_path"])
        assert snapshot["task"]["base_commit"] == baseline and worktree.parent.name == "native-worktrees"
        assert worktree != repository and "edited inside" in (worktree / "tracked.txt").read_text()
        assert (repository / "tracked.txt").read_text() == "original native fixture baseline\n"
        starts = starts_file.read_text().splitlines()
        assert len(starts) == 1
        launch = json.loads(starts[0])
        assert Path(launch["cwd"]) == worktree and "-codex-" in launch["cgroup"] and ".scope" in launch["cgroup"]
        provider_identity = launch["pid"], birth(launch["pid"])
        assert provider_identity[1] and not receipts.exists()
        conflict = command("AttachNative", {"id": task, "incarnation": snapshot["incarnation"], "view": "competing-native-task"})
        assert "another" in conflict["Err"].lower(), conflict

        assert birth(provider_identity[0]) == provider_identity[1]
        reopen_task(task)
        assert birth(provider_identity[0]) == provider_identity[1]
        wait_for(lambda: status()["hasApproval"], "reopened Aether did not regain the pending native approval")
        assert status()["approval"] == presented["approval"] and not receipts.exists()
        driver.click("#native-approvals button")
        wait_for(lambda: status()["finish"] and "Decline observed" in status()["output"],
                 "native provider did not receive the explicit decline")
        assert [json.loads(line) for line in receipts.read_text().splitlines()] == [{"decision": "decline"}]
        assert starts_file.read_text().splitlines() == starts
        driver.click("#native-finish")
        wait_for(lambda: status()["canInspect"] and birth(provider_identity[0]) != provider_identity[1],
                 "native Finish did not release the real provider scope")
        driver.click("#native-inspect")
        wait_for(lambda: "edited inside the native fixture worktree" in status()["diff"], "task diff was not inspected")
        assert status()["canVerify"] and not status()["canAccept"] and not verification_count.exists()
        assert driver.execute("return document.querySelector('#native-verification-command').textContent;") == recipe
        assert driver.execute("return document.querySelector('#native-verification-shell').textContent;") == "Shell /bin/sh"
        driver.click("#native-verify")
        wait_for(lambda: (view := status())["verification"] == "Running"
                 and "AETHER_VERIFICATION_STARTED" in view["verificationOutput"],
                 "the host verification PTY did not start")
        assert verification_count.read_text().splitlines() == ["run"]
        assert not status()["canAccept"] and not status()["canInspect"]
        reopen_task(task)
        wait_for(lambda: (view := status())["verification"] == "Running"
                 and "AETHER_VERIFICATION_STARTED" in view["verificationOutput"],
                 "reopened Aether did not recover the live verification PTY")
        assert verification_count.read_text().splitlines() == ["run"]
        verification_gate.touch()
        wait_for(lambda: (view := status())["verification"] == "Passed"
                 and "AETHER_VERIFICATION_PASSED" in view["verificationOutput"],
                 "host verification did not pass")
        first_check = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]["validation"]
        assert first_check["evidence"]["cwd"] == str(worktree)
        assert first_check["evidence"]["shell"] == "/bin/sh"
        assert first_check["evidence"]["command"] == recipe
        assert first_check["evidence"]["unchanged_after_run"] and not status()["canAccept"]

        driver.click("#native-inspect")
        wait_for(lambda: status()["canAccept"], "current verified evidence was not eligible for acceptance")
        with (worktree / "tracked.txt").open("a") as tracked:
            tracked.write("external edit after inspection\n")
        driver.click("#native-accept-verified")
        wait_for(lambda: "Validation evidence is stale" in status()["status"],
                 "host did not reject stale worktree evidence")
        assert not status()["canAccept"] and status()["lifecycle"] != "Completed"
        wait_for(lambda: (view := status())["canVerify"] and view["verification"] == "Needs review",
                 "fresh verification was not available after stale evidence")
        driver.click("#native-verify")
        wait_for(lambda: status()["verification"] == "Passed", "explicit verification rerun did not pass")
        assert verification_count.read_text().splitlines() == ["run", "run"]
        driver.click("#native-inspect")
        wait_for(lambda: (view := status())["canAccept"] and "external edit after inspection" in view["diff"],
                 "fresh reviewed diff did not enable acceptance")
        driver.click("#native-accept-verified")
        wait_for(lambda: status()["lifecycle"] == "Completed", "verified work was not accepted by the host")
        reopened = command("InspectNative", {"id": task})["Ok"]["Native"]["snapshot"]
        assert reopened["incarnation"] == snapshot["incarnation"] and reopened["task"]["status"] == "completed"
        assert reopened["validation"]["run_id"] != first_check["run_id"]
        assert reopened["validation"]["evidence"]["fingerprint"] != first_check["evidence"]["fingerprint"]
        reopen_task(task)
        wait_for(lambda: status()["lifecycle"] == "Completed", "accepted task could not reopen")
        assert not status()["canAccept"] and not status()["canVerify"]
        assert verification_count.read_text().splitlines() == ["run", "run"]
        assert (repository / "tracked.txt").read_text() == "original native fixture baseline\n"
        assert starts_file.read_text().splitlines() == starts
        exercise_native_tools(driver, command, task, reopened, starts_file)
        exercise_native_mission(driver, owned, command, reopen_task, task, reopened, starts_file)
        followup = exercise_native_context(driver, owned, command, reopen_task, task, starts_file, repository)
        exercise_native_manual_acceptance(driver, command, reopen_task, followup, starts_file)
        exercise_native_queue(driver, command, reopen_task, task, repository, starts_file)
        assert verification_count.read_text().splitlines() == ["run", "run"]
        assert (repository / "tracked.txt").read_text() == "original native fixture baseline\n"
        print(json.dumps({
            "native_task_shared_host_isolated_worktree": True,
            "native_task_codex_systemd_scope": True,
            "native_task_conflicting_view_refused": True,
            "native_task_reopens_exact_pending_approval": True,
            "native_task_decline_receipt_once": True,
            "native_task_finish_and_read_only_diff": True,
            "native_verification_explicit_and_owned": True,
            "native_verification_reopens_running_without_replay": True,
            "native_acceptance_rejects_stale_worktree_evidence": True,
            "native_verified_acceptance_persists_after_reopen": True,
            "native_context_persists_redacted_material_and_rejects_stale_save": True,
            "native_context_preview_rejects_changed_shared_revision": True,
            "native_context_opens_unsent_draft_then_separate_native_task": True,
            "native_mission_links_exact_worktree_and_run": True,
            "native_mission_cas_keeps_local_edit_and_reopens_saved_plan": True,
            "native_mission_freezes_observed_commit_and_keeps_stale_checks_pending": True,
            "native_tools_pages_to_exact_native_validation": True,
            "native_tools_refuses_replaced_revision_without_launch": True,
            "native_manual_acceptance_rejects_unreviewed_worktree_edit": True,
            "native_manual_acceptance_persists_without_validation": True,
            "native_queue_persists_drafts_dependencies_and_rejects_stale_update": True,
            "native_queue_ready_does_not_launch_until_explicit_consent": True,
        }), flush=True)
    except Exception:
        print("Native task fixture:", log.read_text()[-4000:], status())
        raise
    finally:
        verification_gate.touch()
        if starts_file.is_file():
            for entry in starts_file.read_text().splitlines():
                with contextlib.suppress(ValueError, KeyError, FileNotFoundError, ProcessLookupError):
                    pid = json.loads(entry)["pid"]
                    args = Path(f"/proc/{pid}/cmdline").read_bytes()
                    if os.fsencode(owned / "bin" / "native-codex-fixture.py") in args.split(b"\0"):
                        os.kill(pid, signal.SIGTERM)
        stop_driver(host)


def exercise_notebook(driver, owned):
    from hashlib import sha256

    source = owned / "native-cells.jtnb.md"
    ran = owned / "notebook-explicit-run"
    late = owned / "notebook-child-escaped-stop"
    document = (
        "# Native cells\n\n"
        "<script>document.body.dataset.notebookExecuted = 'yes'</script>\n\n"
        "```shell\n"
        "printf 'NOTE_OUT\\n'\nprintf 'NOTE_ERR\\n' >&2\n"
        f"pwd > {shlex.quote(str(ran))}\nexit 17\n"
        "```\n\n"
        "```sh\n"
        f"(sleep 3; printf child > {shlex.quote(str(late))}) &\n"
        "printf 'PARTIAL_OUT\\n'\nprintf 'PARTIAL_ERR\\n' >&2\nwait\n"
        "```\n\n"
        "```javascript\nthrow Error('read-only fence')\n```\n"
    ).encode()
    source.write_bytes(document)
    fingerprint = sha256(document).hexdigest()
    driver.click("#local-notebook > summary")
    driver.type("#notebook-file", str(source))
    driver.click("#notebook-open")
    wait_for(lambda: driver.execute(
        f"return document.querySelector('#notebook-source-id').textContent.includes('{fingerprint}')"),
        "exact native notebook source did not open")
    assert not ran.exists() and not late.exists()
    assert driver.execute("""
        return !document.body.dataset.notebookExecuted &&
          !document.querySelector('[data-notebook-cell="5"] button') &&
          document.querySelectorAll('[data-notebook-run]').length === 2 &&
          document.querySelector('#notebook-run-result').hidden;
    """)

    def result():
        return driver.execute("""
            return {
                title: document.querySelector('#notebook-run-title').textContent,
                stdout: document.querySelector('#notebook-stdout').textContent,
                stderr: document.querySelector('#notebook-stderr').textContent,
                locked: document.querySelector('#notebook-close').disabled,
            };
        """)

    driver.click('[data-notebook-run="1"]')
    wait_for(lambda: result()["title"].endswith("Exited (exit 17)"), "cell exit receipt not rendered")
    assert result() == {"title": "Cell 1 · shell · Exited (exit 17)",
                        "stdout": "NOTE_OUT\n", "stderr": "NOTE_ERR\n", "locked": False}
    assert ran.is_file()
    execution_directory = Path(ran.read_text().strip())
    assert execution_directory != owned and execution_directory.name.startswith("jterm-notebook-")

    driver.click('[data-notebook-run="3"]')
    def partial():
        current = result()
        return current["stdout"] == "PARTIAL_OUT\n" and current["stderr"] == "PARTIAL_ERR\n" and current["locked"]
    wait_for(partial, "live partial stdout/stderr did not arrive")
    driver.click('[data-notebook-stop="3"]')
    wait_for(lambda: result()["title"].endswith("Stopped (signal 9)"), "cell stop receipt not rendered")
    assert result() == {"title": "Cell 2 · sh · Stopped (signal 9)",
                        "stdout": "PARTIAL_OUT\n", "stderr": "PARTIAL_ERR\n", "locked": False}
    time.sleep(3.1)
    assert not late.exists(), "stopped cell left a live child"

    driver.click("#notebook-close")
    wait_for(lambda: not execution_directory.exists(), "closed notebook working directory was not removed")
    ran.unlink()
    driver.type("#notebook-file", str(source))
    driver.click("#notebook-open")
    wait_for(lambda: driver.execute(
        f"return !document.querySelector('#notebook-document').hidden && document.querySelector('#notebook-run-result').hidden && document.querySelector('#notebook-source-id').textContent.includes('{fingerprint}')"),
        "notebook did not reopen with unchanged source and no run")
    assert not ran.exists() and not late.exists()
    driver.click("#notebook-close")
    print(json.dumps({"notebook_native_file_open_and_reopen": True,
                      "notebook_exit_code_and_split_streams": True,
                      "notebook_stop_kept_partial_output_and_no_group_child_write": True}), flush=True)


def exercise_vim9_project(driver, source):
    def view():
        return driver.execute("""
            const el=id=>document.querySelector('#vproject-'+id), project=document.querySelector('#vim9-project');
            return {labels:Array.from(el('choices').children,b=>b.textContent),
                status:el('status').textContent,file:document.querySelector('#vim9-status').textContent,
                root:el('root').textContent, jobs:el('jobs').textContent, output:el('output').textContent,
                diagnostics:el('diagnostics').textContent,line:project.dataset.line,col:project.dataset.byteColumn,windows:project.dataset.windowCount};
        """)

    def pick(title):
        wait_for(lambda: title in view()["labels"], f"missing Vim project choice {title}")
        index = view()["labels"].index(title) + 1
        driver.click(f"#vproject-choices button:nth-child({index})")

    def find(text):
        driver.clear("#vproject-query")
        driver.type("#vproject-query", text)
        driver.click("#vproject-lookup button")

    driver.click("#vim9-project > summary")
    try:
        pick("src/")
        wait_for(lambda: view()["root"].endswith("/src"), "Vim project tree did not enter directory")
        pick("note.txt")
        wait_for(lambda: "note.txt" in view()["file"], "Vim did not open the chosen project file")
        find("src/note.txt:2:4")
        pick("src/note.txt")
        wait_for(lambda: (view()["line"], view()["col"]) == ("2", "4"), "Vim file:line:byte-column navigation")
        find(">split vertically")
        pick("Split editor vertically")
        wait_for(lambda: view()["windows"] == "2", "Vim command palette did not create a split")
        driver.type("#vproject-search-query", "PROJECT_EVIDENCE")
        driver.click("#vproject-search button")
        wait_for(lambda: "1 matches" in view()["jobs"] and "note.txt" in view()["diagnostics"], "Vim project search with quickfix evidence")
        driver.type("#vproject-build-command", "printf 'AETHER_BUILD_OUTPUT\\n'; exit 7")
        driver.click("#vproject-build button")
        wait_for(lambda: "Build exit 7" in view()["jobs"] and "AETHER_BUILD_OUTPUT" in view()["output"], "Vim build output and exit code")
        driver.clear("#vproject-build-command")
        driver.type("#vproject-build-command", "sleep 15")
        driver.click("#vproject-build button")
        wait_for(lambda: "Build running" in view()["jobs"], "Vim build did not start")
        driver.click("#vproject-stop-build")
        wait_for(lambda: "Build stopped" in view()["jobs"], "Vim build did not stop")
        # Close the split with Vim's own action and return to the original file.
        driver.type("#vim9-input", ":close\ue007")
        wait_for(lambda: view()["windows"] == "1", "Vim did not close its split")
        find(source.name)
        pick(source.name)
        wait_for(lambda: source.name in view()["file"], "Vim project picker did not return to original buffer")
    except Exception:
        print("Vim project:", view(), flush=True)
        raise
    driver.click("#vim9-project > summary")
    print(json.dumps({"native_vim9_project": ["tree", "file:line:byte-column", "palette", "quickfix search", "build exit/output", "cancel"]}), flush=True)


def exercise_vim9_overlays(driver, owned):
    setup = owned / "native-vim9-overlays.vim"
    accepted = owned / "accepted-completion"
    selection = owned / "vim-menu-selection"
    setup.write_text(
        "set completeopt=menuone,noinsert,noselect\n"
        "new\nsetlocal buftype=nofile bufhidden=wipe noswapfile\n"
        "highlight AetherPopup guifg=#ffffff guibg=#963c2d gui=NONE\n"
        "highlight Pmenu guifg=#ffffff guibg=#335577 gui=NONE\n"
        "highlight PmenuSel guifg=#ffffff guibg=#448866 gui=NONE\n"
        "highlight! link PmenuMatch Pmenu | highlight! link PmenuMatchSel PmenuSel\n"
        f"autocmd CompleteChanged * call writefile([string(complete_info(['selected']).selected)], '{selection}')\n"
        f"command! ProbeChoice call writefile([getline(1)], '{accepted}')\n"
        "inoremap <F6> <C-R>=[complete(col('.'), ['chooseAlpha','chooseBeta']), ''][1]<CR>\n"
        "let g:aether_popup = popup_create(['Native popup'], {'line':4,'col':6,'minwidth':16,'highlight':'AetherPopup'})\n"
        "call setwinvar(g:aether_popup, 'jterm_native_popup', 1)\n")

    def has_color(red, green, blue):
        return driver.execute(f"""
          const screen=document.querySelector('#vim9-screen'), canvas=document.querySelector('#vim9-render');
          if(screen.dataset.source!=='vim9'||canvas.hidden)return false;
          const data=canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
          let count=0;
          for(let i=0;i<data.length;i+=4)
            if(data[i]==={red}&&data[i+1]==={green}&&data[i+2]==={blue}&&data[i+3]===255&&++count>150)return true;
          return false;
        """)
    driver.type("#vim9-input", f":source {setup}\ue007")
    wait_for(lambda: has_color(0x96, 0x3c, 0x2d), "authenticated Vim popup pixels did not reach the native Canvas")
    driver.type("#vim9-input", ":call popup_close(g:aether_popup)\ue007")
    wait_for(lambda: not has_color(0x96, 0x3c, 0x2d), "closed Vim popup remained painted")
    driver.type("#vim9-input", "A\ue036")
    wait_for(lambda: has_color(0x33, 0x55, 0x77), "Vim completion menu was not painted on the native Canvas")
    def control_key(key):
        driver.command("POST", "/actions", {"actions": [{"type":"key","id":"editor-keyboard","actions":[
            {"type":"keyDown","value":"\ue009"}, {"type":"keyDown","value":key},
            {"type":"keyUp","value":key}, {"type":"keyUp","value":"\ue009"}]}]})
    assert driver.execute("return document.activeElement.id") == "vim9-input"
    control_key("n")
    wait_for(lambda: selection.is_file() and selection.read_text().strip() == "0", "Vim did not select the first completion")
    control_key("y")
    driver.type("#vim9-input", "\ue00c")
    wait_for(lambda: not has_color(0x33, 0x55, 0x77), "accepted completion remained visible")
    driver.type("#vim9-input", ":ProbeChoice\ue007")
    def completed_choice():
        if accepted.is_file():
            text = accepted.read_text()
            if text.endswith("\n"):
                return text

    choice = wait_for(completed_choice, "Vim did not finish writing its completion selection")
    assert choice == "chooseAlpha\n", repr(choice)
    driver.type("#vim9-input", ":close!\ue007")
    print(json.dumps({"native_vim9_overlays": ["popup pixels and dismissal", "completion pixels", "selection in Vim's own buffer"]}), flush=True)


def vim9_file_chooser(atspi, pid):
    desktop = atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(index)
        if app is None or app.get_process_id() != pid:
            continue
        for child in range(app.get_child_count()):
            window = app.get_child_at_index(child)
            if (window is not None and window.get_name() == "Open local file in Vim9"
                    and window.get_state_set().contains(atspi.StateType.SHOWING)):
                return window
    return None


def accept_vim9_file(atspi, dialog, path):
    def descendants(node):
        for index in range(node.get_child_count()):
            child = node.get_child_at_index(index)
            if child is not None:
                yield child
                yield from descendants(child)

    location = wait_for(lambda: next((node for node in descendants(dialog)
                        if node.get_state_set().contains(atspi.StateType.FOCUSED)
                        and node.get_state_set().contains(atspi.StateType.EDITABLE)), None),
                        "GTK file chooser did not focus its location field")
    assert location.get_editable_text_iface().set_text_contents(str(path))
    wait_for(lambda: atspi.Text.get_text(location.get_text_iface(), 0, -1) == str(path),
             "GTK location differs from the fixture")
    accept = wait_for(lambda: next((node for node in descendants(dialog)
                      if node.get_role() == atspi.Role.PUSH_BUTTON and node.get_name() == "Open"
                      and node.get_state_set().contains(atspi.StateType.SENSITIVE)), None),
                      "GTK file chooser did not enable Open")
    assert accept.get_action_iface().do_action(0)


def exercise_vim9_workbench(driver, owned):
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi
    Atspi.init()
    project = owned / "vim-project"
    (project / "src").mkdir(parents=True)
    (project / "Makefile").touch()
    (project / "src" / "note.txt").write_text("First line\n中PROJECT_EVIDENCE\n")
    source = project / "native Vim9 edit.txt"
    source.write_text("AETHER_VIM9_ORIGINAL\n")
    driver.click("#vim9-workbench > summary")

    def choose(button="#vim9-open", path=source):
        driver.click(button)
        chooser = subprocess.check_output(
            ["xdotool", "search", "--sync", "--onlyvisible", "--name", "^Open local file in Vim9$"],
            timeout=10, text=True).splitlines()[-1]
        pid = int(subprocess.check_output(["xdotool", "getwindowpid", chooser], text=True, timeout=5))
        dialog = wait_for(lambda: vim9_file_chooser(Atspi, pid),
                          "GTK file chooser did not expose its accessible window")
        subprocess.run(["xdotool", "windowfocus", "--sync", chooser], check=True, timeout=5)
        subprocess.run(["xdotool", "key", "ctrl+l"], check=True, timeout=5)
        accept_vim9_file(Atspi, dialog, path)
        wait_for(lambda: not vim9_file_chooser(Atspi, pid), "GTK file chooser did not accept the selected file")

    def editor():
        return driver.execute("""
            const canvas=document.querySelector('#vim9-render');
            const pixels=canvas.getContext('2d').getImageData(0,0,32,32).data;
            return {source:document.querySelector('#vim9-screen').dataset.source,
                status:document.querySelector('#vim9-status').textContent,
                error:document.querySelector('#vim9-error').textContent,
                painted:pixels.some((channel,index)=>index%4===3 && channel>0),
                manualFallback:document.querySelector('#vim9-fallback').checked,
                save:!document.querySelector('#vim9-save').disabled,
                released:!document.querySelector('#vim9-release').disabled};
        """)

    def renderer_ready():
        view = editor()
        return view["source"] == "vim9" and view["painted"] and view["save"] and source.name in view["status"]

    try:
        choose()
        wait_for(renderer_ready, "authenticated Vim9 buffer was not painted over its live PTY", timeout=20)
        driver.type("#vim9-input", "GoAETHER_VIM9_GUI_中\ue00c")
        wait_for(lambda: "Modified" in editor()["status"], "Vim did not own the GUI text edit")
        assert source.read_text() == "AETHER_VIM9_ORIGINAL\n"
        driver.click("#vim9-save")
        first_edit = "AETHER_VIM9_ORIGINAL\nAETHER_VIM9_GUI_中\n"
        wait_for(lambda: source.read_text() == first_edit, "Vim9 save did not persist exact GUI text")
        driver.click("#vim9-fallback")
        wait_for(lambda: editor()["source"] == "terminal", "live compatibility Vim was not shown")
        driver.type("#vim9-input", "GoAETHER_VIM9_PTY_你\ue00c")
        wait_for(lambda: "Modified" in editor()["status"], "compatibility view did not edit the same Vim buffer")
        driver.click("#vim9-fallback")
        wait_for(renderer_ready, "native paint did not resume for the same editor")
        assert source.read_text() == first_edit
        driver.click("#vim9-save")
        final = first_edit + "AETHER_VIM9_PTY_你\n"
        wait_for(lambda: source.read_text() == final, "Vim did not save both edits to the chosen local file")
        exercise_vim9_project(driver, source)
        exercise_vim9_overlays(driver, owned)
        wait_for(renderer_ready, "Vim did not restore the chosen file after closing the scratch window")
        driver.click("#vim9-quit")
        wait_for(lambda: editor()["released"], "workbench did not receive the Vim/PTy exit")
        driver.click("#vim9-release")
        wait_for(lambda: driver.execute("return document.querySelector('#vim9-viewport').hidden;"),
                 "closed workbench retained the departed Vim frame")

        driver.type("#preview-single", str(source))
        driver.click("#preview-show")
        wait_for(lambda: driver.execute("return !document.querySelector('#preview-vim9').hidden;"),
                 "the text preview did not offer a separate Vim9 chooser")
        other = owned / source.name
        other.write_text("Different file with the same basename\n")
        choose("#preview-vim9", other)
        wait_for(lambda: driver.execute("return document.querySelector('#preview-notice').textContent.includes('differs from the preview');"),
                 "Vim opened a different local file with the same name")
        assert driver.execute("return document.querySelector('#vim9-viewport').hidden;")
        choose("#preview-vim9")
        wait_for(renderer_ready, "verified local snapshot did not open in Vim9", timeout=20)
        disconnect = owned / "disconnect-owned-vim.vim"
        disconnect.write_text(
            "let sid = getscriptinfo({'name': 'vim9-ide-bridge.vim'})[0].sid\n"
            "call ch_close(getscriptinfo({'sid': sid})[0].variables.bridge)\n")
        driver.type("#vim9-input", f":source {disconnect}\ue007")

        def automatic_fallback():
            view = editor()
            return view["source"] == "terminal" and not view["manualFallback"] and not view["save"]

        wait_for(automatic_fallback, "disconnecting the Vim GUI bridge did not expose the live terminal")
        driver.type("#vim9-input", "GoAETHER_VIM9_AFTER_LOSS_文\ue00c:wq\ue007")
        wait_for(lambda: source.read_text() == final + "AETHER_VIM9_AFTER_LOSS_文\n",
                 "Vim could not save the same buffer after losing its GUI bridge")
        wait_for(lambda: editor()["released"], "compatibility Vim exit did not drain its terminal")
        driver.click("#vim9-release")
    except Exception:
        print("Vim9 editor:", editor(), "chosen file:", source.read_text())
        raise
    print(json.dumps({"native_vim9_exact_chooser_open_and_canvas": True,
                      "native_vim9_preview_refuses_same_name_different_bytes": True,
                      "native_vim9_gui_and_compatibility_edit_same_buffer": True,
                      "native_vim9_saved_exact_unicode_and_graceful_exit": True,
                      "native_vim9_actual_gui_disconnect_preserves_live_pty_input": True}), flush=True)


def private_accessibility_address():
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    address = bus.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", None,
                            GLib.VariantType.new("(s)"), Gio.DBusCallFlags.NONE, 5000, None)
    return address.unpack()[0]


def run(binary, jsh, workspace, api, native, notebook=False, output_review=False, vim9=False,
        inline_graphics_only=False, history_graphics_only=False, workspace_only=False, backend="x11",
        vim_binary=None, workflow_only=False):
    executable = shutil.which("WebKitWebDriver")
    if not executable:
        raise RuntimeError("run with official WebKitWebDriver")
    if backend == "wayland":
        display = os.environ.get("WAYLAND_DISPLAY")
        if not display:
            raise RuntimeError("run with a private Wayland compositor")
        socket_path = Path(os.environ.get("XDG_RUNTIME_DIR", "")) / display
        if not socket_path.is_absolute() or not socket_path.is_socket():
            raise RuntimeError("WAYLAND_DISPLAY must identify the private compositor socket")
        connection = {"WAYLAND_DISPLAY": str(socket_path)}
    else:
        if not os.environ.get("DISPLAY") or not os.environ.get("XAUTHORITY"):
            raise RuntimeError("run with authenticated Xvfb")
        connection = {name: os.environ[name] for name in ("DISPLAY", "XAUTHORITY")}
    with tempfile.TemporaryDirectory(prefix="aether-native-two-") as scratch:
        owned = Path(scratch)
        env = {name: os.environ[name] for name in ("DBUS_SESSION_BUS_ADDRESS", "GDK_SCALE")
               if name in os.environ}
        env.update(connection)
        env.update({"HOME": scratch, "PATH": "/usr/bin:/bin", "LC_ALL": "C.UTF-8",
                    "GDK_BACKEND": backend, "LIBGL_ALWAYS_SOFTWARE": "1",
                    "TAURI_WEBVIEW_AUTOMATION": "true"})
        if jsh or workspace or workspace_only or native or vim9:
            launch = owned / "bin"
            launch.mkdir()
            env["PATH"] = f"{launch}:/usr/bin:/bin"
        if jsh:
            (launch / "jsh").symlink_to(jsh)
        if jsh or vim9:
            (launch / "vim").symlink_to(vim_binary or Path(shutil.which("vim")).resolve())
        if vim9:
            (launch / "rg").symlink_to(Path(shutil.which("rg")).resolve())
        transcripts = prepare_workspace(owned, env, launch) if workspace or workspace_only else None
        native_fixture = prepare_native_task(owned, env, launch) if native else None
        if api and not workspace:
            env["JTERM_WORKSPACE_STATE_DIR"] = str(owned / "workspace")
        for name in ("CACHE", "CONFIG", "DATA", "RUNTIME", "STATE"):
            path = owned / name.lower()
            path.mkdir(mode=0o700)
            env["XDG_RUNTIME_DIR" if name == "RUNTIME" else f"XDG_{name}_HOME"] = str(path)
        if jsh:
            prepare_workflows(owned, env)
        if vim9:
            address = private_accessibility_address()
            os.environ["AT_SPI_BUS_ADDRESS"] = env["AT_SPI_BUS_ADDRESS"] = address
        if output_review:
            (owned / "Downloads").mkdir()
            (Path(env["XDG_CONFIG_HOME"]) / "user-dirs.dirs").write_text(
                'XDG_DOWNLOAD_DIR="$HOME/Downloads"\n')
        prepare_hyperlink_handler(owned, env)
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        driver = Driver(port)
        with (owned / "driver.log").open("wb") as log, (
            api_provider(env) if api else contextlib.nullcontext()
        ) as fixture:
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
                if workspace_only:
                    exercise_workspace(driver, owned, binary, env, transcripts)
                    return
                if workflow_only:
                    results = exercise_terminal_workflows(driver, owned, verify_bookmarks=False)
                    binary_hash = hashlib.sha256()
                    with binary.open("rb") as artifact:
                        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
                            binary_hash.update(chunk)
                    print(json.dumps({"case": "PRODUCT.WORKFLOW.001.NATIVE.001", "result": "passed",
                                      "backend": backend, "binary_sha256": binary_hash.hexdigest(), **results}), flush=True)
                    return
                if inline_graphics_only:
                    exercise_inline_graphics(driver, owned)
                    return
                if history_graphics_only:
                    exercise_history_graphics(driver, owned)
                    return
                exercise(driver, owned)
                exercise_hyperlinks(driver, owned)
                exercise_keyboard(driver, owned)
                exercise_height_shrink(driver)
                exercise_renderer_reload(driver, owned)
                exercise_restart(driver, owned, binary, integrated=jsh is not None)
                if jsh:
                    exercise_command_blocks(driver, owned, output_review)
                    exercise_terminal_chat(driver, owned)
                    print(json.dumps(exercise_terminal_workflows(driver, owned)), flush=True)
                if workspace:
                    exercise_workspace(driver, owned, binary, env, transcripts)
                if api:
                    exercise_api_chat(driver, owned, binary, env, fixture)
                if native:
                    exercise_native_task(driver, owned, binary, env, native_fixture)
                if notebook:
                    exercise_notebook(driver, owned)
                if vim9:
                    exercise_vim9_workbench(driver, owned)
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
    parser.add_argument("--jsh", type=Path, help="installed jsh for the command-block, Terminal Chat and workflow journeys")
    parser.add_argument("--workspace", action="store_true", help="include the isolated shared Workspace host and CLI PTY journey")
    parser.add_argument("--workspace-only", action="store_true", help="run the isolated Workspace CLI journey by itself")
    parser.add_argument("--workflow-only", action="store_true", help="run the isolated Workflow journey by itself")
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11", help="display backend for standalone journeys")
    parser.add_argument("--api-chat", action="store_true", help="include the isolated local streaming API conversation")
    parser.add_argument("--native-task", action="store_true", help="include the isolated scoped native Codex task and approval")
    parser.add_argument("--notebook", action="store_true", help="include native file selection and a supervised notebook run/stop")
    parser.add_argument("--output-review", action="store_true", help="compare and export actual local jsh outputs after eviction")
    parser.add_argument("--vim9", action="store_true", help="open an actual local workbench and edit/save through Vim and its fallback PTY")
    parser.add_argument("--vim-binary", type=Path, help="exact Vim executable for an isolated native fixture")
    parser.add_argument("--inline-graphics-only", action="store_true", help="exercise real PTY image rendering and reconnect")
    parser.add_argument("--history-graphics-only", action="store_true", help="exercise projected history rendering and selection")
    args = parser.parse_args()
    if not args.binary.is_absolute() or args.binary.is_symlink() or not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("an absolute regular executable is required")
    if args.jsh and (not args.jsh.is_absolute() or not args.jsh.is_file() or not os.access(args.jsh, os.X_OK)
                     or not (args.vim_binary or shutil.which("vim"))):
        parser.error("--jsh requires an absolute installed jsh executable and Vim")
    if args.output_review and not args.jsh:
        parser.error("--output-review requires --jsh")
    if args.workflow_only and (not args.jsh or args.workspace_only):
        parser.error("--workflow-only requires --jsh and a standalone run")
    if args.vim_binary and (not args.vim_binary.is_absolute() or not args.vim_binary.is_file()
                            or not os.access(args.vim_binary, os.X_OK)):
        parser.error("--vim-binary requires an absolute executable path")
    if args.vim9 and (not (args.vim_binary or shutil.which("vim"))
                      or any(not shutil.which(tool) for tool in ("rg", "xdotool"))):
        parser.error("--vim9 requires Vim 9, ripgrep and xdotool")
    if args.backend == "wayland" and not (args.workspace_only or args.workflow_only):
        parser.error("--backend wayland requires a standalone journey")
    signal.signal(signal.SIGTERM, interrupted)
    run(args.binary, args.jsh, args.workspace, args.api_chat, args.native_task, args.notebook,
        args.output_review, args.vim9, args.inline_graphics_only, args.history_graphics_only,
        workspace_only=args.workspace_only, backend=args.backend,
        vim_binary=args.vim_binary.resolve() if args.vim_binary else None, workflow_only=args.workflow_only)
