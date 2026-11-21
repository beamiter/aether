#!/usr/bin/env python3
"""Drive Aether’s native WebKitGTK terminal over an isolated Xvfb display and SSH."""
import argparse
import contextlib
import json
import os
from pathlib import Path, PurePosixPath
import runpy
import shlex
import signal
import subprocess
import tempfile
import time

def wait(predicate, label, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.08)
    raise AssertionError(label)


def stop(child):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(4)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(4)


def run(env, args, **kwargs):
    return subprocess.run(list(map(str,args)), env=env, check=True, text=True, **kwargs)


def exercise(app, destination, helper):
    probe = runpy.run_path(str(Path(__file__).with_name('test-ssh-terminal-session.py')))
    root = Path(tempfile.mkdtemp(prefix='jwm-aether-ssh-webview.'))
    print(root, flush=True)

    def start(cleanup, args, name, env=None, pass_fds=()):
        with (root / name).open('wb') as log:
            child = subprocess.Popen(
                args, env=env, stdout=log, stderr=subprocess.STDOUT, pass_fds=pass_fds,
            )
        cleanup.callback(stop, child)
        return child

    with contextlib.ExitStack() as cleanup:
        supervisor = probe['Channel'](destination, ['python3', '-c', probe['SUPERVISE'], helper])
        cleanup.callback(supervisor.close)
        socket = supervisor.line().decode()
        owned = PurePosixPath(socket).parent
        display_path = root / 'display'
        with display_path.open('wb') as handle:
            start(cleanup, ['Xvfb','-displayfd',str(handle.fileno()),'-screen','0','1600x1750x24',
                             '-nolisten','tcp','-ac','-noreset'], 'xvfb.log', pass_fds=(handle.fileno(),))
        display = wait(lambda: display_path.read_text().strip(), 'Xvfb display')
        env = os.environ | {'DISPLAY':':'+display,'GDK_BACKEND':'x11','LIBGL_ALWAYS_SOFTWARE':'1',
                            'AETHER_SSH_TERMINAL_HELPER':helper,'AETHER_SSH_TERMINAL_SOCKET':socket}
        for name in ['CONFIG','CACHE','RUNTIME','DATA','STATE']:
            directory = root / name.lower()
            directory.mkdir(mode=0o700)
            env['XDG_RUNTIME_DIR' if name=='RUNTIME' else f'XDG_{name}_HOME'] = str(directory)
        start(cleanup, ['metacity','--sm-disable'], 'wm.log', env)
        time.sleep(.6)
        desktop = start(cleanup, [app], 'aether.log', env)

        def xt(*args):
            return run(env, ['xdotool',*args], capture_output=True).stdout

        def find_window():
            result = subprocess.run(['xdotool','search','--onlyvisible','--pid',str(desktop.pid)],env=env,text=True,capture_output=True)
            return result.stdout.split()[-1] if result.stdout.strip() else None

        window = wait(find_window,'native Aether WebView')
        xt('windowactivate','--sync',window)
        run(env,['wmctrl','-i','-r',hex(int(window)),'-e','0,80,20,1350,1670'])
        time.sleep(.6)

        def snap(name):
            run(env, ['import','-window',window,root/(name+'.png')])

        def click(pos):
            xt('mousemove','--window',window,*pos,'click','1')

        def pids():
            result = subprocess.run(['ps','--ppid',str(desktop.pid),'-o','pid=,comm='],env=env,text=True,capture_output=True)
            return [int(pid) for pid,comm in (line.split(None,1) for line in result.stdout.splitlines()) if comm=='ssh']

        snap('start')
        # These coordinates belong to the fixed 1350×1670 Xvfb window above.
        click((404, 256))
        xt('type', '--clearmodifiers', destination)
        click((652, 256))
        first = wait(pids, 'Aether starts OpenSSH')[0]
        time.sleep(1)
        snap('connected')
        active = {
            'recover': (273, 306), 'detach': (774, 306), 'attach': (906, 306),
            'close': (1025, 306), 'input': (405, 1467), 'send': (1139, 1467),
            'snapshot': (260, 1110), 'output': (396, 1255),
        }

        def submit(command):
            click(active['input'])
            xt('key', 'ctrl+a')
            clipboard_owner = subprocess.Popen(
                ['xclip', '-selection', 'clipboard', '-i', '-quiet'],
                env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            try:
                clipboard_owner.stdin.write(command.encode())
                clipboard_owner.stdin.close()
                time.sleep(.15)
                xt('key', 'ctrl+v')
                time.sleep(.35)
                snap('typed')
            finally:
                stop(clipboard_owner)
            click(active['send'])
            time.sleep(.35)
            snap('sent')

        def visible():
            click(active['snapshot'])
            click(active['output'])
            xt('key', 'ctrl+a', 'ctrl+c')
            out = subprocess.run(['xclip','-o','-selection','clipboard'],env=env,text=True,capture_output=True,timeout=3)
            return [line.strip() for line in out.stdout.splitlines()]

        submit("stty -echo; printf '\\033[2;1H\\033[2K%s\\r\\n' ECHO_DISABLED")
        wait(lambda:'ECHO_DISABLED' in visible(),'initial remote PTY output renders in native WebView')
        quote=shlex.quote
        submit("SENTINEL=WEBVIEW_PTY_SURVIVED; "
               f"printf '%s' \"$$\" > {quote(str(owned/'pty-pid'))}; "
               "printf '\\033[3;1H\\033[2K%s\\r\\n' WEBVIEW_READY; "
               f"(n=0; while [ ! -f {quote(str(owned/'proceed'))} ] && [ \"$n\" -lt 100 ]; "
               "do n=$((n+1)); sleep 0.05; done; "
               f"if [ -f {quote(str(owned/'proceed'))} ]; then "
               "printf '\\033[7;1H\\033[2K%s\\r\\n' WEBVIEW_DETACHED_OUTPUT; "
               f"printf done > {quote(str(owned/'written'))}; fi) &")
        before = wait(lambda:(lines if 'WEBVIEW_READY' in (lines:=visible()) else None),'initial marker in canvas grid')
        assert 'WEBVIEW_DETACHED_OUTPUT' not in before
        snap('before-cut')

        assert pids()==[first]
        argv=Path(f'/proc/{first}/cmdline').read_bytes().split(b'\0')
        assert destination.encode() in argv and any(socket.encode() in arg for arg in argv)
        fd=os.pidfd_open(first)
        try:
            signal.pidfd_send_signal(fd,signal.SIGKILL)
        finally:
            os.close(fd)
        wait(lambda:not pids(),'old OpenSSH child is reaped after loss')
        remote_pid, born=json.loads(probe['remote_python'](destination,probe['RELEASE_DETACHED_OUTPUT'],str(owned)))
        snap('link-lost')

        click(active['recover'])
        second = wait(pids,'recovery starts new OpenSSH child')[0]
        assert first!=second
        recovered = wait(lambda:(lines if 'WEBVIEW_DETACHED_OUTPUT' in (lines:=visible()) else None),'disconnected output renders after recovery')
        assert recovered.count('WEBVIEW_READY')==recovered.count('WEBVIEW_DETACHED_OUTPUT')==1
        snap('recovered')
        submit("printf '\\033[9;1H\\033[2K%s\\r\\n' \"$SENTINEL\"")
        wait(lambda:'WEBVIEW_PTY_SURVIVED' in visible(),'recovered SSH shell accepts new input and preserves its variable')

        click(active['detach'])
        wait(lambda: not pids(), 'explicit detach retires relay')
        snap('detached')
        click(active['attach'])
        third = wait(pids, 'read-only attach creates fresh relay')[0]
        assert third not in (first, second)
        readonly = wait(lambda:(lines if 'WEBVIEW_PTY_SURVIVED' in (lines:=visible()) else None),'read-only reattachment restores grid')
        for marker in ['WEBVIEW_READY','WEBVIEW_DETACHED_OUTPUT','WEBVIEW_PTY_SURVIVED']:
            assert readonly.count(marker)==1
        snap('read-only')
        forbidden = str(owned / 'read-only-write')
        submit(f"printf forbidden > {quote(forbidden)}")
        probe['remote_python'](destination,
            "import pathlib, sys; assert not pathlib.Path(sys.argv[1]).exists(), 'read-only view sent input'",
            forbidden,
        )
        click(active['close'])
        wait(lambda:not pids(),'explicit close retires relay')
        probe['remote_python'](destination,probe['CONFIRM_REAPED'],str(remote_pid),born)
        snap('closed')
        xt('windowclose',window)
        assert desktop.wait(7)==0
        return dict(
            native_webview=True, ssh_children=3, detached_content_recovered=True,
            same_pty_input_resumed=True, read_only_grid=True, read_only_write_blocked=True,
            remote_pty_reaped=True, clean_native_exit=True, artifacts=str(root),
        )


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', help='path to the built Aether Tauri executable')
    parser.add_argument('destination', help='SSH destination with a verified host key')
    parser.add_argument('helper', help='absolute path to aether-terminal-host on the SSH host')
    args = parser.parse_args()
    print(json.dumps(exercise(args.app, args.destination, args.helper)), flush=True)
