#!/usr/bin/env python3
"""Exercise SSH Files and read-only Workspace over a private loopback OpenSSH server."""
import argparse
import contextlib
import getpass
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import socket
import subprocess
import tempfile

native = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))
wait = native["wait_for"]


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def exercise_files(driver, owned):
    project = owned / "remote-project"
    project.mkdir()
    local = owned / "local-project"
    local.mkdir()
    name = "same.md"
    remote_text = "Remote project 中文\n<script>literal</script>\n"
    (local / name).write_text("Local snapshot only\n")
    (project / name).write_text(remote_text)
    (project / "hidden-link").symlink_to(local / name)
    payload = bytes(range(256))
    binary = local / "sample.bin"
    binary.write_bytes(payload)

    def text(selector):
        return driver.execute(f"return document.querySelector({json.dumps(selector)}).textContent")
    def exists(selector):
        return driver.execute(f"return !!document.querySelector({json.dumps(selector)})")
    def choose(entry):
        selector = f'#files-entries option[value="{entry}"]'
        wait(lambda: exists(selector), f"missing remote entry {entry}")
        driver.click(selector)
    def status_contains(value):
        return wait(lambda: value in text("#files-status"), f"SSH Files status {value!r}, observed {text('#files-status')!r}")

    # Same basename in a user-selected local preview and in the remote project.
    driver.type("#preview-single", str(local / name))
    driver.click("#preview-show")
    wait(lambda: text("#preview-source") == "Local snapshot only\n", "local snapshot not opened")
    driver.click("#preview-close")
    driver.click("#ssh-files > summary")
    driver.type("#files-host", "aether-qa")
    driver.type("#files-root", str(project))
    driver.click("#files-open")
    choose(name)
    assert not exists('#files-entries option[value="hidden-link"]')
    driver.click("#files-preview")
    wait(lambda: text("#files-text") == remote_text, "wrong remote source")
    version = hashlib.sha256(remote_text.encode()).hexdigest()
    assert version in text("#files-version") and "aether-qa" in text("#files-version")
    assert not exists("#files-text script")
    (project / name).write_text("New revision\n")
    driver.click("#files-download")
    status_contains("changed since preview")
    assert not (owned / "downloads" / name).exists()
    driver.click("#files-preview")
    wait(lambda: text("#files-text") == "New revision\n", "new SSH file revision")
    driver.click("#files-download")
    downloaded = owned / "downloads" / name
    wait(lambda: downloaded.is_file(), "missing downloaded SSH snapshot")
    assert downloaded.read_bytes() == b"New revision\n"

    driver.type("#files-name", "new-folder")
    driver.click("#files-mkdir")
    choose("new-folder")
    driver.click("#files-enter")
    wait(lambda: text("#files-source").endswith("/new-folder"), "folder navigation")
    driver.type("#files-local", str(binary))
    driver.click("#files-upload")
    choose(binary.name)
    assert (project / "new-folder" / binary.name).read_bytes() == payload
    binary.write_bytes(b"replacement")
    driver.type("#files-local", str(binary))
    driver.click("#files-upload")
    status_contains("Could not create new SSH file")
    assert (project / "new-folder" / binary.name).read_bytes() == payload
    driver.click("#files-download")
    wait(lambda: (owned / "downloads" / binary.name).is_file(), "binary download")
    assert (owned / "downloads" / binary.name).read_bytes() == payload
    driver.click("#files-parent")
    choose(name)
    driver.click("#files-close")
    wait(lambda: text("#files-source") == "" and text("#files-text") == "", "retired file source")
    driver.click("#preview-show")
    wait(lambda: text("#preview-source") == "Local snapshot only\n", "local selection changed after SSH")
    driver.click("#preview-close")
    print(json.dumps({"ssh_files_native": ["local/remote source isolation", "versioned preview and download",
          "mkdir", "binary upload and download", "existing file preserved", "close"]}), flush=True)


def exercise_workspace(driver, binary, host_id, store):
    def command(kind, value):
        return native["workspace_command"](store, kind, value)["Ok"]
    chat = command("CreateChat", {"title": "Loopback conversation"})["Chat"]["chat"]["row"]["id"]
    record = dict(conversation=chat, event_id="external:qa", revision="r1", source="ImportedHistory",
                  message_sequence=None, text="Loopback evidence version one", truncated=False)
    def index(doc):
        assert command("IndexText", {"documents": [doc]}) == "Ack"
    index(record)
    for number in range(100):
        command("CreateChat", {"title": f"Catalog page fixture {number:03}"})
    for number in range(31):
        index({**record, "event_id": f"external:page{number:02}", "text": f"Paged fixture document {number:02}"})

    def state():
        return driver.execute("""
            const el=id=>document.querySelector('#rws-'+id);
            return {ready:!el('submit').disabled, status:el('status').textContent,
                    options:Array.from(el('conversation').options,o=>o.textContent),
                    hits:el('hits').childElementCount, text:el('text').textContent,
                    source:el('source').textContent, anchor:el('anchor').textContent,
                    open:!el('document').hidden, busy:el('refresh').disabled,
                    next:!el('next').disabled, more:!el('more').hidden};
        """)
    driver.click("#ssh-workspace > summary")
    driver.type("#rws-alias", "aether-qa")
    driver.type("#rws-executable", str(binary))
    driver.type("#rws-host-id", "wrong-fixture-id")
    driver.click("#rws-open")
    wait(lambda: "identity" in state()["status"].lower() and not state()["busy"], "mismatched Workspace identity not rejected")
    assert "Loopback conversation" not in state()["options"]
    driver.click("#rws-disconnect")
    driver.clear("#rws-host-id")
    driver.type("#rws-host-id", host_id)
    driver.click("#rws-open")
    wait(lambda: state()["next"] and len(state()["options"]) == 101, "first remote catalog page")
    first_titles = set(state()["options"][1:])
    driver.click("#rws-next")
    wait(lambda: not state()["busy"] and not state()["next"] and len(state()["options"]) == 2, "next remote catalog page")
    last_title = state()["options"][1]
    assert last_title not in first_titles and "Loopback conversation" in first_titles | {last_title}
    driver.type("#rws-query", "Paged fixture document")
    driver.click("#rws-submit")
    wait(lambda: state()["hits"] == 30 and state()["more"] and not state()["busy"], "first remote evidence page")
    driver.execute("window.retiredHit = document.querySelector('#rws-hits button');")
    driver.click("#rws-more")
    wait(lambda: state()["hits"] == 1 and not state()["more"] and not state()["busy"], "next remote evidence page")
    driver.execute("window.retiredHit.click();")
    driver.click("#rws-hits button")
    wait(lambda: state()["text"].startswith("Paged fixture document "), "document on second remote page")
    driver.clear("#rws-query")
    driver.type("#rws-query", "Loopback evidence")
    driver.click("#rws-submit")
    wait(lambda: state()["hits"] == 1 and not state()["busy"], "remote search")
    new = {**record, "revision": "r2", "text": "Loopback evidence version two"}
    index(new)
    driver.click("#rws-hits button")
    wait(lambda: "revision changed" in state()["status"].lower(), "remote stale revision was not rejected")
    assert not state()["open"]
    driver.click("#rws-submit")
    wait(lambda: state()["hits"] == 1 and not state()["busy"], "updated remote search")
    driver.click("#rws-hits button")
    wait(lambda: state()["text"] == new["text"], "remote evidence not opened")
    assert host_id in state()["source"] and "r2" in state()["anchor"]
    driver.click("#rws-disconnect")
    wait(lambda: state()["text"] == "" and not state()["open"], "remote evidence not retired")
    print(json.dumps({"ssh_workspace_native": ["wrong identity rejected", "paged read-only catalog",
          "paged evidence search", "replaced revision refused", "exact document", "disconnect"]}), flush=True)


def exercise(app, helper, backend, compositor_libraries):
    uid = os.getuid()
    runtime = Path(f"/run/user/{uid}")
    if not runtime.is_dir() or runtime.stat().st_uid != uid:
        runtime = Path(tempfile.gettempdir())
    with tempfile.TemporaryDirectory(prefix="aether-remote-", dir=runtime) as temporary, contextlib.ExitStack() as stack:
        owned = Path(temporary)
        def start(argv, log, env=None):
            output = stack.enter_context((owned / log).open("wb"))
            child = subprocess.Popen(argv, cwd=owned, env=env, stdout=output, stderr=output, start_new_session=True)
            stack.callback(native["stop_driver"], child)
            return child

        ssh = owned / "client-ssh"
        ssh.mkdir(mode=0o700)
        host_key, key = owned / "host-key", ssh / "key"
        for filename in [host_key, key]:
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", filename], check=True)
        ssh_port = port()
        (ssh / "known_hosts").write_text(f"[127.0.0.1]:{ssh_port} " + host_key.with_suffix(".pub").read_text())
        (ssh / "config").write_text(f"""Host aether-qa
    HostName 127.0.0.1
    Port {ssh_port}
    User {getpass.getuser()}
    IdentityFile {key}
    IdentitiesOnly yes
    UserKnownHostsFile {ssh}/known_hosts
    GlobalKnownHostsFile /dev/null
    UpdateHostKeys no
""")
        state = owned / "remote-state"
        config = owned / "sshd.conf"
        config.write_text(f"""ListenAddress 127.0.0.1
Port {ssh_port}
HostKey {host_key}
PidFile {owned}/sshd.pid
AuthorizedKeysFile {key}.pub
AllowUsers {getpass.getuser()}
AuthenticationMethods publickey
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
DisableForwarding yes
PermitTTY no
PermitUserRC no
SetEnv JTERM_WORKSPACE_STATE_DIR={state}
""")
        daemon = start(["/usr/sbin/sshd", "-D", "-e", "-f", config], "sshd.log")
        wait(lambda: (owned / "sshd.pid").is_file() or daemon.poll() is not None, "private sshd")
        subprocess.run(["/usr/bin/ssh", "-F", ssh / "config", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes",
                        "aether-qa", "true"], check=True, timeout=10)
        remote_env = os.environ | {"JTERM_WORKSPACE_STATE_DIR": str(state)}
        host = start([app, "--jterm-session-host"], "host.log", remote_env)
        store = state / "run" / "host.sock"
        wait(lambda: store.is_socket() or host.poll() is not None, "remote Workspace daemon")
        host_id = subprocess.check_output([app, "--jterm-workspace-host-id"], env=remote_env, text=True).strip()

        env = os.environ | {"TAURI_WEBVIEW_AUTOMATION": "true", "GDK_BACKEND": backend,
                            "LIBGL_ALWAYS_SOFTWARE": "1", "AETHER_SSH_TERMINAL_HELPER": str(helper),
                            "JTERM_WORKSPACE_STATE_DIR": str(owned / "local-state")}
        for setting in ["CONFIG", "CACHE", "DATA", "STATE", "RUNTIME"]:
            directory = owned / setting.lower()
            directory.mkdir(mode=0o700)
            env["XDG_RUNTIME_DIR" if setting == "RUNTIME" else f"XDG_{setting}_HOME"] = str(directory)
        downloads = owned / "downloads"
        downloads.mkdir()
        (owned / "config" / "user-dirs.dirs").write_text(f'XDG_DOWNLOAD_DIR="{downloads}"\n')
        if backend == "wayland":
            env.pop("DISPLAY", None)
            runtime_dir = Path(env["XDG_RUNTIME_DIR"])
            config = owned / "sway.conf"
            config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n")
            compositor_env = env | {"WLR_BACKENDS": "headless", "WLR_RENDERER": "pixman", "WLR_LIBINPUT_NO_DEVICES": "1"}
            if compositor_libraries:
                compositor_env["LD_LIBRARY_PATH"] = str(compositor_libraries.resolve(strict=True))
            compositor = start(["sway", "--unsupported-gpu", "-c", config], "sway.log", compositor_env)
            def display():
                if compositor.poll() is not None:
                    raise RuntimeError((owned / "sway.log").read_text())
                return next((p.name for p in runtime_dir.glob("wayland-*") if p.is_socket()), None)
            env["WAYLAND_DISPLAY"] = wait(display, "private Wayland compositor")
            start(["wtype", "-s", "600000", "-k", "F24"], "seat.log", env)
        driver_port = port()
        driver = native["Driver"](driver_port)
        # Both native clients run system OpenSSH. Give only this test process
        # a private SSH configuration without editing the user's home.
        wrap = ["bwrap", "--dev-bind", "/", "/", "--ro-bind", ssh / "config", "/etc/ssh/ssh_config"]
        personal = Path.home() / ".ssh"
        if personal.is_dir():
            wrap += ["--ro-bind", ssh, personal]
        webdriver = shutil.which("WebKitWebDriver")
        if not webdriver:
            raise RuntimeError("WebKitWebDriver is required")
        process = start([*wrap, "--", webdriver, "--host=127.0.0.1", f"--port={driver_port}"], "driver.log", env)
        def ready():
            if process.poll() is not None:
                raise RuntimeError((owned / "driver.log").read_text())
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False
        wait(ready, "private WebKitWebDriver")
        driver.open(app)
        stack.callback(driver.close)
        wait(lambda: native["opened_page"](driver), "Aether's embedded page")
        try:
            exercise_files(driver, owned)
            exercise_workspace(driver, app, host_id, store)
            def digest(path):
                with path.open("rb") as content:
                    return hashlib.file_digest(content, "sha256").hexdigest()
            print(json.dumps({"case": "PRODUCT.REMOTE.001.NATIVE.001", "backend": backend, "result": "passed",
                              "desktop_sha256": digest(app), "helper_sha256": digest(helper)}), flush=True)
        except Exception:
            print(driver.execute("return ['files-status','rws-status','rws-source'].map(id=>document.getElementById(id).textContent)"), flush=True)
            print((owned / "sshd.log").read_text()[-2000:], flush=True)
            raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("aether", type=Path)
    parser.add_argument("helper", type=Path)
    parser.add_argument("--backend", choices=["x11", "wayland"], default="x11")
    parser.add_argument("--compositor-libraries", type=Path)
    args = parser.parse_args()
    exercise(args.aether.resolve(strict=True), args.helper.resolve(strict=True), args.backend, args.compositor_libraries)
