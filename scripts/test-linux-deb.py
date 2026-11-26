#!/usr/bin/env python3
"""Install, upgrade and roll back two Aether packages in a private dpkg root."""
import argparse
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile


PACKAGE = "aether-terminal"
DESKTOP = Path("usr/bin/tauri-terminal-lab")
HOST = Path("usr/bin/aether-terminal-host")
TERMINFO = Path("usr/share/terminfo/j/jterm-256color")


def run(*command):
    return subprocess.run(command, check=True, capture_output=True, text=True, timeout=45).stdout.strip()


def package_field(archive, field):
    return run("dpkg-deb", "--field", archive, field)


def status_record(*alternatives):
    for package in alternatives:
        result = subprocess.run(["dpkg-query", "--status", package], capture_output=True, text=True, timeout=10)
        if result.returncode == 0 and "Status: install ok installed" in result.stdout.splitlines():
            return result.stdout.strip()
    raise ValueError(f"Install one of the required libraries: {', '.join(alternatives)}")


def test_packages(previous, candidate):
    if any(package_field(archive, "Package") != PACKAGE for archive in (previous, candidate)):
        raise ValueError(f"Both archives must contain {PACKAGE}")
    old_version, new_version = [package_field(archive, "Version") for archive in (previous, candidate)]
    run("dpkg", "--compare-versions", new_version, "gt", old_version)

    rollback = runpy.run_path(str(Path(__file__).with_name("test-workspace-rollback.py")))
    digest = rollback["digest"]
    with tempfile.TemporaryDirectory(prefix="aether-deb-") as directory:
        private = Path(directory)
        root = private / "system"
        database = root / "var/lib/dpkg"
        database.mkdir(parents=True)
        # The package is installed in this root. Its processes run against the
        # runner's GTK/WebKit libraries, whose installed records resolve Depends.
        dependencies = [status_record("libwebkit2gtk-4.1-0"), status_record("libgtk-3-0", "libgtk-3-0t64")]
        (database / "status").write_text("\n\n".join(dependencies) + "\n")
        dpkg = ("fakeroot", "--", "dpkg", "--force-not-root", f"--root={root}",
                f"--admindir={database}", f"--log={private / 'dpkg.log'}")

        def install(archive, expected_version):
            run(*dpkg, "--install", archive)
            status = run("dpkg-query", f"--admindir={database}", "--show",
                         "--showformat=${Status}\t${Version}", PACKAGE)
            assert status == f"install ok installed\t{expected_version}", status
            assert not run(*dpkg, "--verify", PACKAGE), "installed package differs from its file checksums"
            desktop_entry = root / "usr/share/applications/Aether Terminal.desktop"
            assert "Exec=tauri-terminal-lab" in desktop_entry.read_text().splitlines()
            return {path.name: digest(root / path) for path in (DESKTOP, HOST)}

        old_build = install(previous, old_version)
        old_terminfo = (root / TERMINFO).read_bytes() if (root / TERMINFO).exists() else None
        old_desktop = private / DESKTOP.name
        shutil.copy2(root / DESKTOP, old_desktop)
        new_build = install(candidate, new_version)
        assert old_build[DESKTOP.name] != new_build[DESKTOP.name], "rehearsal requires distinct desktop builds"
        terminfo = root / TERMINFO
        compiled = subprocess.run([root / HOST, "terminfo"], check=True,
                                  capture_output=True, timeout=10).stdout
        assert terminfo.read_bytes() == compiled, "installed terminfo differs from the runtime"
        assert run("env", "TERM=jterm-256color", f"TERMINFO={terminfo.parent.parent}",
                   "tput", "colors") == "256"
        terminfo_sha256 = digest(terminfo)
        rollback["exercise"](old_desktop, root / DESKTOP)
        assert install(previous, old_version) == old_build, "downgrade did not restore the original binaries"
        restored_terminfo = terminfo.read_bytes() if terminfo.exists() else None
        assert restored_terminfo == old_terminfo, "downgrade did not restore the original terminfo installation"

    print(json.dumps({"case": "INSTALL.LINUX.DEB.001",
                      "previous_version": old_version, "candidate_version": new_version,
                      "previous_package_sha256": digest(previous), "candidate_package_sha256": digest(candidate),
                      "previous_binaries_sha256": old_build, "candidate_binaries_sha256": new_build,
                      "terminfo_sha256": terminfo_sha256, "terminfo_restored": True,
                      "clean_install": True, "upgraded": True, "rolled_back": True}, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", type=Path)
    parser.add_argument("candidate", type=Path)
    args = parser.parse_args()
    try:
        test_packages(args.previous.resolve(strict=True), args.candidate.resolve(strict=True))
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"{error}\n{error.stderr}")
