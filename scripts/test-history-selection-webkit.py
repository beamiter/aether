#!/usr/bin/env python3
"""Use the native archive, live PTY and OS clipboard to check anchored copy."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import runpy
import shlex
import socket
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
NATIVE = runpy.run_path(str(HERE / 'test-two-local-webkit.py'))
ROWS = [f'aether-archive-{number:04} 中' for number in range(152, 155)]
EXPECTED = '\n'.join(ROWS)
POISON = b'aether-unrelated-clipboard-before-copy'

PRODUCER = r'''
import fcntl, json, os, select, struct, sys, termios, time, tty
from pathlib import Path

root = Path(sys.argv[1])
device = os.ttyname(0)
pid = os.getpid()
original = termios.tcgetattr(0)
birth = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()[19]

def publish(replies):
    window = fcntl.ioctl(0, termios.TIOCGWINSZ, bytes(8))
    rows, columns, _, _ = struct.unpack('HHHH', window)
    update = root / 'producer.pending'
    update.write_text(json.dumps(dict(pid=pid, birth=birth, tty=device, rows=rows, columns=columns, replies=replies)))
    update.replace(root / 'producer.json')

try:
    tty.setraw(0)
    os.write(1, b'\x1b[3J\x1b[2J\x1b[H')
    for number in range(278):
        line = f'aether-archive-{number:04} 中'
        if number % 23 == 0:
            line += ' ' + 'wrap-geometry-' * 9
        os.write(1, (line + '\r\n').encode())
    publish(0)
    bound = time.monotonic() + 85
    while not (root / 'start-live').exists():
        if time.monotonic() > bound:
            raise TimeoutError('start-live')
        time.sleep(.03)
    replies = 0
    while not (root / 'finish-live').exists():
        if time.monotonic() > bound:
            raise TimeoutError('finish-live')
        os.write(1, f'\x1b[?2026hAETHER_LIVE_UPDATE_{replies:04}\r\n\x1b[5n\x1b[?2026l'.encode())
        reply = b''
        until = time.monotonic() + 3
        while len(reply) < 4:
            readable, _, _ = select.select([0], [], [], max(0, until - time.monotonic()))
            if not readable:
                raise TimeoutError('terminal status query')
            reply += os.read(0, 4 - len(reply))
        if reply != b'\x1b[0n':
            raise ValueError(f'wrong DSR: {reply!r}')
        replies += 1
        publish(replies)
        time.sleep(.06)
    os.write(1, b'\r\nAETHER_LIVE_TAIL_AFTER_HISTORY\r\n')
    (root / 'tail-written').touch()
except Exception as error:
    (root / 'producer-error').write_text(repr(error))
    raise
finally:
    termios.tcsetattr(0, termios.TCSANOW, original)
'''

READ = """
const output=document.querySelector('#history-output'), canvas=document.querySelector('#history-canvas');
return {text:output.value,selected:output.value.slice(output.selectionStart,output.selectionEnd),
  label:document.querySelector('#history-label').textContent,
  current:document.querySelector('#cursor-status').textContent,
  view:!document.querySelector('#history-viewport').hidden,
  width:parseFloat(canvas.style.width), first:Number(canvas.dataset.firstRow)};
"""


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(args):
    binary = args.binary.absolute()
    assert binary.is_file() and not binary.is_symlink() and digest(binary) == args.sha256
    out = args.output.absolute()
    out.mkdir(parents=True, exist_ok=False)
    (out / 'producer.py').write_text(PRODUCER)
    with tempfile.TemporaryDirectory(prefix='aether-history-selection-') as scratch, contextlib.ExitStack() as cleanup:
        owned = Path(scratch)
        env = {name: os.environ[name] for name in ('DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS', 'PATH') if name in os.environ}
        env.update(GDK_BACKEND=args.backend, LC_ALL='C.UTF-8', LIBGL_ALWAYS_SOFTWARE='1', TAURI_WEBVIEW_AUTOMATION='true')
        for name in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR'):
            directory = owned / name
            directory.mkdir(mode=0o700)
            env[name] = str(directory)

        def spawn(command, name, environment=env, stdin=None):
            log = cleanup.enter_context((out / name).open('wb'))
            child = subprocess.Popen(command, cwd=owned, env=environment, stdin=stdin or subprocess.DEVNULL,
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(NATIVE['stop_driver'], child)
            return child

        def wait(predicate, label):
            return NATIVE['wait_for'](predicate, label, timeout=18)

        if args.backend == 'wayland':
            env.pop('DISPLAY', None)
            env.pop('XAUTHORITY', None)
            configuration = owned / 'sway.conf'
            configuration.write_text('xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n')
            sway = spawn(['sway', '--unsupported-gpu', '-c', str(configuration)], 'compositor.log', dict(
                env, WLR_BACKENDS='headless', WLR_RENDERER='pixman', WLR_LIBINPUT_NO_DEVICES='1'))

            def display():
                assert sway.poll() is None
                return next((item.name for item in Path(env['XDG_RUNTIME_DIR']).glob('wayland-*') if item.is_socket()), None)

            env['WAYLAND_DISPLAY'] = wait(display, 'private Wayland socket unavailable')
            spawn(['wtype', '-s', '600000', '-k', 'F24'], 'seat.log')
            clipboard_read = ['wl-paste', '--no-newline']
            clipboard_write = ['wl-copy', '--foreground', '--type', 'text/plain;charset=utf-8']
        else:
            assert env.get('DISPLAY') and env.get('XAUTHORITY'), 'authenticated private Xvfb required'
            clipboard_read = ['xclip', '-selection', 'clipboard', '-o']
            clipboard_write = ['xclip', '-selection', 'clipboard', '-i', '-quiet']

        def clipboard():
            result = subprocess.run(clipboard_read, env=env, capture_output=True, timeout=3)
            return result.stdout if result.returncode == 0 else None

        with socket.socket() as slot:
            slot.bind(('127.0.0.1', 0))
            port = slot.getsockname()[1]
        web = spawn(['WebKitWebDriver', '--host=127.0.0.1', f'--port={port}'], 'webdriver.log')
        driver = NATIVE['Driver'](port)
        cleanup.callback(driver.close)

        def webdriver_ready():
            assert web.poll() is None
            try:
                return driver.request('GET', '/status', timeout=1).get('ready')
            except OSError:
                return False

        def read():
            return driver.execute(READ)

        def producer():
            failure = owned / 'producer-error'
            if failure.is_file():
                raise AssertionError(failure.read_text())
            record = owned / 'producer.json'
            return json.loads(record.read_text()) if record.is_file() else None

        def select_rows():
            first = driver.execute("""
                const page=window.historyFixtureProjection;
                const start=page.text.indexOf(%s), viewport=document.querySelector('#history-viewport');
                const first=page.rows.findIndex(row=>row.text_offsets[0]===start);
                viewport.scrollIntoView({block:'center'}); viewport.scrollTop=Math.max(0,(first-1)*24);
                return first;
            """ % json.dumps(ROWS[0]))
            assert first >= 0

            def points():
                return driver.execute("""
                    const canvas=document.querySelector('#history-canvas'), rect=canvas.getBoundingClientRect();
                    const local=%d-Number(canvas.dataset.firstRow);
                    const top={x:Math.floor(rect.left+6),y:Math.floor(rect.top+24*local+12)};
                    const bottom={x:Math.floor(rect.left+21*12+6),y:top.y+48};
                    if([top,bottom].some(point=>document.elementFromPoint(point.x,point.y)!==canvas))return null;
                    return {top,bottom};
                """ % first)

            views = wait(points, 'archive canvas did not expose both selection endpoints')
            driver.command('POST', '/actions', {'actions': [{'type': 'pointer', 'id': 'history-pointer',
                'parameters': {'pointerType': 'mouse'}, 'actions': [
                    {'type': 'pointerMove', 'origin': 'viewport', 'duration': 0, **views['top']},
                    {'type': 'pointerDown', 'button': 0},
                    {'type': 'pointerMove', 'origin': 'viewport', 'duration': 140, **views['bottom']},
                    {'type': 'pointerUp', 'button': 0}]}]})
            driver.command('DELETE', '/actions')
            return wait(lambda: (view := read())['selected'] == EXPECTED and view,
                        'archive drag did not produce exact UTF-8 selection')

        try:
            wait(webdriver_ready, 'WebKitWebDriver did not listen')
            driver.open(binary)
            wait(lambda: NATIVE['opened_page'](driver), 'embedded Aether page did not load')
            driver.command('POST', '/window/rect', {'width': 1380, 'height': 1000})
            driver.click('#open')
            wait(lambda: (state := driver.execute(NATIVE['READ']))['writable'] and state['transport'] == 'channel',
                 'local shell did not obtain a writable native Channel')
            driver.type('#input', f'{shlex.quote("/usr/bin/python3")} {shlex.quote(str(out / "producer.py"))} {shlex.quote(str(owned))}')
            driver.click('#send')
            initial = wait(producer, 'real PTY producer did not finish archive setup')
            assert initial['tty'].startswith('/dev/pts/')
            # Observe the real projection once to place the native drag on its
            # visual row, which can differ from the logical text line after wraps.
            driver.execute("""
                const callbacks=window.__TAURI_INTERNALS__.callbacks, register=callbacks.set;
                callbacks.set=function(id, handler){
                  return register.call(this,id,reply=>{
                    const value=typeof reply==='string'&&reply.startsWith('{')?JSON.parse(reply):null;
                    if(value?.rows&&value.text?.includes('aether-archive-0152')){
                      window.historyFixtureProjection=value;
                      delete callbacks.set;
                    }
                    return handler(reply);
                  });
                };
            """)
            driver.click('#history-refresh')
            wait(lambda: ROWS[0] in read()['text'] and ROWS[-1] in read()['text'] and
                 driver.execute('return Boolean(window.historyFixtureProjection);'), 'seeded rows not in archive window')
            selected = select_rows()

            (owned / 'start-live').touch()
            before = wait(lambda: (live := producer()) and live['replies'] >= 3 and live, 'terminal did not answer synchronized producer')
            for selector, value in [('#cols', '52'), ('#rows', '17')]:
                driver.clear(selector)
                driver.type(selector, value)
            driver.click('#resize')
            after = wait(lambda: (live := producer()) and [live['columns'], live['rows']] == [52, 17] and live,
                         'actual PTY dimensions did not resize while output was streaming')
            after_resize = wait(lambda: (view := read())['view'] and view['width'] == 52 * 12 and view,
                                'retained archive did not reflow to the new width')
            assert after_resize['selected'] == EXPECTED, f"resize changed the selected bytes: {after_resize['selected']!r}"
            assert after_resize['label'] != selected['label']
            driver.click('#history-older')
            after_page = wait(lambda: (view := read())['label'] != after_resize['label'] and view,
                              'older page did not change the viewed archive range')
            assert after_page['selected'] == EXPECTED, f"paging changed the selected bytes: {after_page['selected']!r}"
            assert after_page['view'] and after_page['first'] == 0 and after_page['text'].splitlines()[0] != after_resize['text'].splitlines()[0]
            assert all(row in after_page['text'] for row in ROWS)

            def poison(log_name):
                selection_owner = spawn(clipboard_write, log_name, stdin=subprocess.PIPE)
                selection_owner.stdin.write(POISON)
                selection_owner.stdin.close()
                wait(lambda: clipboard() == POISON, 'private OS clipboard poison was not installed')

            poison('clipboard-poison.log')
            driver.execute("document.querySelector('#history-output').focus({preventScroll:true});")
            if args.backend == 'wayland':
                control = next(Path(env['XDG_RUNTIME_DIR']).glob('sway-ipc.*.sock'))
                with (out / 'focused-window.json').open('wb') as tree:
                    subprocess.run(['swaymsg', '-s', control, '-t', 'get_tree', '-r'], env=env, stdout=tree, check=True, timeout=5)
                driver.execute("""
                    window.historyClipEvents=[];
                    for(const name of ['keydown','copy'])document.addEventListener(name,event=>{
                      window.historyClipEvents.push({name, key:event.key, ctrl:event.ctrlKey,
                        trusted:event.isTrusted, target:event.target.id, focused:document.hasFocus()});
                    },true);
                """)
                subprocess.run(['wtype', '-M', 'ctrl', '-k', 'c', '-m', 'ctrl'], env=env, check=True, timeout=5)
            else:
                driver.command('POST', '/actions', {'actions': [{'type': 'key', 'id': 'copy-keys', 'actions': [
                    {'type': 'keyDown', 'value': '\ue009'}, {'type': 'keyDown', 'value': 'c'},
                    {'type': 'keyUp', 'value': 'c'}, {'type': 'keyUp', 'value': '\ue009'}]}]})
                driver.command('DELETE', '/actions')
            wait(lambda: clipboard() == EXPECTED.encode(), 'native clipboard did not contain the selected archive bytes')
            poison('clipboard-button-poison.log')
            if args.backend == 'wayland':
                assert driver.execute("""
                    const button=document.querySelector('#history-copy');
                    if(button.disabled)return false;
                    button.focus();
                    return document.activeElement===button;
                """)
                subprocess.run(['wtype', '-k', 'Return'], env=env, check=True, timeout=5)
            else:
                driver.click('#history-copy')
            wait(lambda: clipboard() == EXPECTED.encode(), 'explicit Copy selection did not replace the OS clipboard')
            latest = wait(lambda: (live := producer()) and live['replies'] > after['replies'] and live, 'PTY queries did not continue after paging')
            assert all(before[key] == after[key] == latest[key] for key in ('pid', 'birth', 'tty'))
            assert after['replies'] > before['replies']
            assert read()['label'] == after_page['label'] and read()['selected'] == EXPECTED

            (owned / 'finish-live').touch()
            wait((owned / 'tail-written').is_file, 'producer did not write the tail')

            def live_tail():
                driver.click('#copy-visible')
                return 'AETHER_LIVE_TAIL_AFTER_HISTORY' in read()['text']

            wait(live_tail, 'same live terminal did not paint final producer output')
            driver.click('#close')
            wait(lambda: driver.execute(NATIVE['READ'])['status'].startswith('Terminal closed'), 'terminal did not close')
        except Exception:
            (owned / 'finish-live').touch()
            with contextlib.suppress(RuntimeError, OSError):
                (out / 'last-state.json').write_text(json.dumps(read(), indent=2) + '\n')
                events = driver.execute('return window.historyClipEvents ?? [];')
                (out / 'clipboard-events.json').write_text(json.dumps(events, indent=2) + '\n')
                if current_clipboard := clipboard():
                    (out / 'last-clipboard').write_bytes(current_clipboard)
            raise

    assert digest(binary) == args.sha256
    result = {'case_id': 'PRODUCT.HISTORY.001.NATIVE.001', 'result': 'passed', 'backend': args.backend,
              'binary_sha256': args.sha256, 'selected_text': EXPECTED, 'clipboard_hex': EXPECTED.encode().hex(),
              'actual_pty_before_resize': before, 'actual_pty_after_resize': after, 'actual_pty_after_paging': latest,
              'anchored_archive_range_after_resize': after_resize['label'], 'older_archive_range': after_page['label'],
              'selection_survived_reflow_and_paging': True, 'native_clipboard_poison_replaced': True,
              'both_copy_shortcut_and_visible_button_write_exact_bytes': True,
              'archive_stayed_off_live_until_explicit_snapshot': True, 'same_pty_final_output_painted': True,
              'fixture_sha256': digest(Path(__file__).resolve())}
    (out / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--backend', required=True, choices=('x11', 'wayland'))
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args())
