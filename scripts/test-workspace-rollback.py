#!/usr/bin/env python3
"""Rehearse an Aether Workspace upgrade and backup rollback with two real builds."""
import argparse
from contextlib import closing, contextmanager
import hashlib
import json
import os
from pathlib import Path
import runpy
import sqlite3
import subprocess
import tempfile

native = runpy.run_path(str(Path(__file__).with_name("test-two-local-webkit.py")))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


@contextmanager
def host(binary, state, environment):
    endpoint = state / "run" / "host.sock"
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen([binary, "--jterm-session-host"], cwd=state, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=log, start_new_session=True,
                                   env=dict(environment, JTERM_WORKSPACE_STATE_DIR=str(state)))
        try:
            def ready():
                if process.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode(errors="replace"))
                try:
                    return "Ok" in native["workspace_command"](endpoint, "MissionList", None)
                except (FileNotFoundError, ConnectionRefusedError):
                    return False
            native["wait_for"](ready, "Workspace host did not accept commands")

            def chat(command, **arguments):
                return native["workspace_command"](endpoint, command, arguments)["Ok"]["Chat"]["chat"]
            yield chat
        finally:
            native["stop_driver"](process)


def backup(source, target):
    target.touch(mode=0o600, exist_ok=False)
    with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as old:
        with closing(sqlite3.connect(target)) as copy:
            old.backup(copy)


def saved_draft(chat):
    snapshot, = json.loads(chat["snapshot_json"])["chats"]
    return chat["row"]["id"], chat["row"]["title"], chat["draft_revision"], snapshot["draft"]


def exercise(previous, candidate):
    previous_hash, candidate_hash = digest(previous), digest(candidate)
    if previous_hash == candidate_hash:
        raise ValueError("Supply two distinct builds for the cross-version rehearsal")
    with tempfile.TemporaryDirectory(prefix="aether-rollback-", dir="/tmp") as directory:
        root = Path(directory)
        current, rollback, future = [root / name for name in ("current", "rollback", "future")]
        environment = os.environ.copy()
        for key in ("HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME",
                    "XDG_RUNTIME_DIR"):
            build_dir = root / key
            build_dir.mkdir(mode=0o700)
            environment[key] = str(build_dir)
        for state in (current, rollback, future):
            state.mkdir(mode=0o700)
        filename = "workspace.sqlite3"
        with host(previous, current, environment) as old:
            chat = old("CreateChat", title="Rollback fixture")
            identity = chat["row"]["id"]
            original = old("SetDraft", id=identity, expected_revision=chat["draft_revision"], text="原始草稿")
            assert saved_draft(original)[-1] == "原始草稿"
            backup(current / filename, rollback / filename)
        with host(candidate, current, environment) as new:
            assert saved_draft(new("LoadChat", id=identity)) == saved_draft(original)
            updated = new("SetDraft", id=identity, expected_revision=original["draft_revision"], text="升级后")
            assert saved_draft(updated)[-1] == "升级后"
            backup(current / filename, future / filename)

        # A future owner may use a different journal mode. Check the exact
        # database bytes as well as the refusal from the real host process.
        with closing(sqlite3.connect(future / filename)) as db:
            assert db.execute("PRAGMA journal_mode=DELETE").fetchone() == ("delete",)
            version = db.execute("SELECT version FROM schema_version").fetchone()[0] + 1
            db.execute("UPDATE schema_version SET version=?", (version,))
            db.commit()
        before = digest(future / filename)
        rejected = subprocess.run([candidate, "--jterm-session-host"], cwd=future,
                                  env=dict(environment, JTERM_WORKSPACE_STATE_DIR=str(future)),
                                  capture_output=True, timeout=10)
        assert rejected.returncode != 0 and f"database version {version}" in rejected.stderr.decode()
        assert digest(future / filename) == before
        with host(previous, rollback, environment) as old:
            assert saved_draft(old("LoadChat", id=identity)) == saved_draft(original)
            restored = old("SetDraft", id=identity, expected_revision=original["draft_revision"], text="回退副本")
            assert saved_draft(restored)[-1] == "回退副本"
        with host(candidate, current, environment) as new:
            assert saved_draft(new("LoadChat", id=identity)) == saved_draft(updated)
    print(json.dumps({"case": "WORKSPACE.ROLLBACK.001", "previous_sha256": previous_hash,
                      "candidate_sha256": candidate_hash, "refused_schema": version,
                      "incompatible_database_unchanged": True, "independent_backup_restored": True}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", type=Path)
    parser.add_argument("candidate", type=Path)
    options = parser.parse_args()
    exercise(options.previous.resolve(strict=True), options.candidate.resolve(strict=True))
