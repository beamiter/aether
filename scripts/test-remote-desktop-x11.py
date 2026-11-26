"""Exercise Aether and JWM through two private X11 displays."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time

from PIL import Image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('aether', type=lambda path: Path(path).resolve())
    parser.add_argument('jwm_remote', type=lambda path: Path(path).resolve())
    args = parser.parse_args()
    folder = Path(tempfile.mkdtemp(prefix='jwm-aether-embed.'))
    print(f'Artifacts in {folder}', flush=True)
    children = []
    handles = []
    source = None

    def wait(check, description, seconds=16):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if value := check():
                return value
            time.sleep(0.06)
        raise RuntimeError(f'Timed out waiting for {description} in {folder}')

    def log(name):
        handle = (folder / name).open('wb')
        handles.append(handle)
        return handle

    def display(name):
        number = log(f'{name}.display')
        child = subprocess.Popen(
            ['Xvfb', '-displayfd', str(number.fileno()), '-screen', '0',
             '1440x1080x24', '-nolisten', 'tcp', '-ac', '-noreset'],
            pass_fds=(number.fileno(),), stdout=log(f'{name}.xvfb.log'),
            stderr=subprocess.STDOUT,
        )
        children.append(child)
        return ':' + wait(lambda: (folder / f'{name}.display').read_text().strip(), name)

    def run(*argv, check=True):
        return subprocess.run(argv, env=env, text=True, capture_output=True, check=check).stdout

    def launch(argv, name):
        process = subprocess.Popen(argv, env=env, stdout=log(name), stderr=subprocess.STDOUT)
        children.append(process)
        return process

    def find(*query):
        matches = run('xdotool', 'search', '--onlyvisible', *query, check=False).split()
        return int(matches[-1]) if matches else None

    def xt(*argv):
        return run('xdotool', *map(str, argv))

    def click(window, x, y):
        xt('mousemove', '--window', window, x, y, 'click', 1)

    def tree(window):
        return run('xwininfo', '-id', hex(window), '-tree')

    def window_info(window):
        return run('xwininfo', '-id', hex(window))

    def bounds(window):
        info = window_info(window)
        labels = ['Absolute upper-left X', 'Absolute upper-left Y', 'Width', 'Height']
        return tuple(int(re.search(rf'{label}:\s*(\d+)', info)[1]) for label in labels)

    def screenshot(name):
        path = folder / f'{name}.png'
        run('import', '-window', 'root', path)
        return Image.open(path).convert('RGB')

    def start_host(name, control=False):
        argv = [args.jwm_remote, 'host', '--listen', '127.0.0.1:0', '--once',
                '--key-file', key, '--capture-source', 'root', '--display', source_display,
                '--region', '640x480+0+0', '--share-region', '300x200+50+60', '--stats-json']
        if control:
            argv.append('--allow-input')
        path = folder / f'{name}.log'
        host = launch(argv, path.name)

        def address():
            match = re.search(r'listening on (127\.0\.0\.1:\d+)', path.read_text())
            return match[1] if match else None

        return host, wait(address, f'{name} listen'), path

    def frame(log_path, epoch):
        for line in log_path.read_text().splitlines(keepends=True):
            if line.startswith('{') and line.endswith('\n'):
                row = json.loads(line)
                sent = row.get('last_sent_frame')
                if sent and sent['source_epoch'] == epoch and row['drawn_acks']:
                    return row
        return None

    def connect(address):
        xt('windowactivate', '--sync', app)
        click(app, 220, 160)
        xt('key', 'ctrl+a', 'BackSpace')
        xt('type', '--clearmodifiers', address)
        xt('key', 'Return')
        return wait(lambda: find('--class', 'JwmRemote'), 'native video')

    def prove_parent(viewer):
        parent = int(re.search(r'Parent window id: (0x[0-9a-f]+)', tree(viewer))[1], 16)
        assert hex(viewer) in tree(parent)
        assert hex(parent) in tree(app)
        top_levels = run('xprop', '-root', '_NET_CLIENT_LIST')
        assert hex(app) in top_levels and hex(viewer) not in top_levels
        info = run('xprop', '-id', hex(viewer), '-f', '_XEMBED_INFO', '32c', '_XEMBED_INFO')
        assert '_XEMBED_INFO(_XEMBED_INFO) = 0, 1' in info, info
        return parent

    try:
        source_display = display('source')
        viewer_display = display('viewer')
        env = os.environ | {
            'DISPLAY': viewer_display,
            'GDK_BACKEND': 'x11',
            'LIBGL_ALWAYS_SOFTWARE': '1',
            'PATH': str(args.jwm_remote.parent) + ':' + os.environ['PATH'],
        }
        for kind in ['CONFIG', 'CACHE', 'RUNTIME', 'DATA', 'STATE']:
            path = folder / kind.lower()
            path.mkdir(mode=0o700)
            env[f'XDG_{kind}_DIR' if kind == 'RUNTIME' else f'XDG_{kind}_HOME'] = str(path)
        key = folder / 'config/jwm/remote.key'
        key.parent.mkdir(mode=0o700)
        run(args.jwm_remote, 'keygen', '--output', key)
        subprocess.run(['xsetroot', '-display', source_display, '-solid', '#d41427'], check=True)

        xlib = ctypes.CDLL('libX11.so.6')
        pointer = ctypes.c_void_p
        xid = ctypes.c_ulong
        xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        xlib.XOpenDisplay.restype = pointer
        xlib.XDefaultRootWindow.argtypes = [pointer]
        xlib.XDefaultRootWindow.restype = xid
        xlib.XCreateSimpleWindow.argtypes = [pointer, xid, ctypes.c_int, ctypes.c_int,
                                            ctypes.c_uint, ctypes.c_uint, ctypes.c_uint, xid, xid]
        xlib.XCreateSimpleWindow.restype = xid
        xlib.XMapWindow.argtypes = [pointer, xid]
        xlib.XSync.argtypes = [pointer, ctypes.c_int]
        xlib.XKeysymToKeycode.argtypes = [pointer, xid]
        xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
        xlib.XQueryKeymap.argtypes = [pointer, pointer]
        xlib.XCloseDisplay.argtypes = [pointer]
        source = xlib.XOpenDisplay(source_display.encode())
        assert source
        region = xlib.XCreateSimpleWindow(source, xlib.XDefaultRootWindow(source),
                                          50, 60, 300, 200, 0, 0, 0x19c742)
        xlib.XMapWindow(source, region)
        xlib.XSync(source, 0)
        shift = xlib.XKeysymToKeycode(source, 0xffe1)

        def shift_held():
            bitmap = ctypes.create_string_buffer(32)
            xlib.XQueryKeymap(source, bitmap)
            return bool(bitmap.raw[shift // 8] & (1 << (shift % 8)))

        launch(['metacity', '--sm-disable'], 'metacity.log')
        wait(lambda: '_NET_SUPPORTING_WM_CHECK(WINDOW)' in
             run('xprop', '-root', '_NET_SUPPORTING_WM_CHECK'), 'window manager')
        desktop = launch([args.aether], 'aether.log')
        app = wait(lambda: find('--pid', str(desktop.pid)), 'Aether main window')
        xt('windowactivate', '--sync', app)
        time.sleep(1.3)
        click(app, 183, 35)
        time.sleep(.5)
        screenshot('remote-tab')

        host, address, host_log = start_host('view-only')
        viewer = connect(address)
        parent = prove_parent(viewer)
        first = wait(lambda: frame(host_log, 1), 'initial draw acknowledgement')
        original_size = bounds(viewer)
        shot = screenshot('initial-embedded')
        x, y, w, h = original_size
        red = shot.getpixel((x + w // 2, y + 3 * h // 4))
        assert red[0] > 170 and red[1] < 60 and red[2] < 70, red

        click(app, 283, 159)
        xt('key', 'Down', 'Return')
        second = wait(lambda: frame(host_log, 2), 'selected source draw acknowledgement')
        assert second['last_sent_frame']['source_dimensions'] == [300, 200]
        assert find('--class', 'JwmRemote') == viewer and prove_parent(viewer) == parent

        xt('windowsize', app, 1180, 760)
        resized = wait(lambda: box if (box := bounds(viewer))[3] > h + 120 else None, 'pane resize')
        x, y, w, h = resized

        def green_pixel():
            color = screenshot('resized-embedded').getpixel((x + w // 2, y + h // 2))
            return color if color[1] > 150 and color[0] < 75 and color[2] < 105 else None

        green = wait(green_pixel, 'green pixels inside resized pane')
        # The web controls center in the wider window.
        click(app, 136, 35)

        def hidden():
            info = window_info(viewer)
            (folder / 'hidden-pane-info').write_text(info)
            return 'Map State: IsViewable' not in info

        try:
            wait(hidden, 'native pane hidden', 3)
        finally:
            screenshot('terminal-tab')
        assert host.poll() is None
        click(app, 263, 35)
        wait(lambda: find('--class', 'JwmRemote') == viewer, 'same pane restored')
        wait(green_pixel, 'same source visible on return')
        click(app, 1035, 159)
        wait(lambda: host.poll() is not None and not find('--class', 'JwmRemote'), 'disconnect')
        assert host.returncode == 0 and desktop.poll() is None
        wait(lambda: hex(parent) not in tree(app), 'native socket removed')

        host, address, host_log = start_host('control', control=True)
        xt('windowsize', app, 960, 420)
        time.sleep(.4)
        click(app, 32, 253)  # Allow keyboard and pointer, above clipboard consent.
        viewer = connect(address)
        prove_parent(viewer)
        wait(lambda: 'authenticated session started (screen + pointer + keyboard)'
             in host_log.read_text(), 'keyboard and pointer negotiation')
        wait(lambda: frame(host_log, 1), 'controlled video')
        _, _, w, h = bounds(viewer)
        assert not shift_held()
        click(viewer, w // 2, h // 2)
        xt('keydown', 'Shift_L')
        wait(shift_held, 'Shift held on host')
        run('wmctrl', '-i', '-c', hex(app))
        wait(lambda: not shift_held(), 'Shift released on desktop close')
        xt('keyup', 'Shift_L')
        wait(lambda: host.poll() is not None and not find('--class', 'JwmRemote'), 'child exited')
        assert host.returncode == desktop.wait(10) == 0

        print(json.dumps({
            'first_source': first['last_sent_frame'],
            'second_source': second['last_sent_frame'],
            'initial_size': original_size[2:],
            'resized_size': resized[2:],
            'initial_visible_rgb': red,
            'resized_visible_rgb': green,
            'embedded_xembed_child_not_in_wm_client_list': True,
            'same_child_after_source_change_and_tab_switch': True,
            'disconnect_removed_native_pane': True,
            'host_shift_released_on_app_close': True,
        }), flush=True)
    finally:
        if source:
            xlib.XCloseDisplay(source)
        for process in reversed(children):
            if process.poll() is None:
                process.terminate()
        for process in reversed(children):
            try:
                process.wait(3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for handle in handles:
            handle.close()
        if 'key' in locals():
            key.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
