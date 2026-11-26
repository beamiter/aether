#!/usr/bin/env python3
"""Verify loss of a native API admission receipt and late cross-view replies."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import runpy
import socket
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
NATIVE = runpy.run_path(str(HERE / 'test-two-local-webkit.py'))
CASE = 'PRODUCT.WORKSPACE.001.NATIVE.001'
FIRST_DRAFT = 'start exactly once\nfrom the initial composer'
NEXT_DRAFT = 'saved during the stream\nkeep these exact bytes'
OTHER_DRAFT = 'another conversation\nkeep it independent'
ANSWER = 'First streamed segment. Final durable segment.'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


READ = """
const node=id=>document.getElementById(id);
return {
  status:node('chat-status').textContent,
  job:node('chat-job-status').textContent,
  draft:node('chat-draft').value,
  preview:node('chat-preview').textContent,
  history:[...node('chat-history').querySelectorAll('pre')].map(el=>el.textContent),
  options:[...node('chat-select').options].map(el=>el.value),
  canOpen:!node('chat-open').disabled,
  view:!node('chat-view').hidden,
  canSend:!node('chat-send').disabled,
  canCancel:!node('chat-cancel').disabled,
};
"""


def hold_next_job(driver, conversation):
    # Interpose only at the real Tauri callback delivery boundary. The embedded
    # JS, native command, Workspace host and provider are otherwise unmodified.
    assert driver.execute("""
        const callbacks=window.__TAURI_INTERNALS__.callbacks;
        if (!(callbacks instanceof Map) || window.heldWorkspaceReceipt) return false;
        const fixture={conversation:%s,receipt:null,deliver:null};
        const register=callbacks.set;
        callbacks.set=function(id, handler) {
          return register.call(this,id,reply=>{
            const data=reply?.value;
            if (!fixture.receipt && Number.isInteger(reply?.request) && data?.kind==='job' &&
                data.identity?.conversation===fixture.conversation && data.job?.user_text===%s) {
              fixture.receipt=reply;
              fixture.deliver=()=>handler(reply);
              delete callbacks.set;
              return;
            }
            return handler(reply);
          });
        };
        window.heldWorkspaceReceipt=fixture;
        return callbacks.set!==register;
    """ % (json.dumps(conversation), json.dumps(FIRST_DRAFT)))


def receipt(driver):
    return driver.execute('return window.heldWorkspaceReceipt?.receipt ?? null;')


def run(args):
    binary = args.binary.absolute()
    assert binary.is_file() and not binary.is_symlink() and os.access(binary, os.X_OK)
    assert digest(binary) == args.sha256
    sources = [Path(__file__).resolve(), HERE / 'test-two-local-webkit.py', HERE.parent / 'web/workspace-chat.js']
    source_hashes = {str(source.relative_to(HERE.parent)): digest(source) for source in sources}
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    witnesses = {}
    last_state = None
    with tempfile.TemporaryDirectory(prefix='aether-workspace-recovery-') as scratch, contextlib.ExitStack() as cleanup:
        owned = Path(scratch)
        env = {name: os.environ[name] for name in ('PATH', 'DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS')
               if name in os.environ}
        env.update(GDK_BACKEND=args.backend, LC_ALL='C.UTF-8', LIBGL_ALWAYS_SOFTWARE='1',
                   TAURI_WEBVIEW_AUTOMATION='true', JTERM_WORKSPACE_STATE_DIR=str(owned / 'workspace'))
        for name in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR'):
            folder = owned / name
            folder.mkdir(mode=0o700)
            env[name] = str(folder)

        def spawn(command, log_name, environment=env):
            log = cleanup.enter_context((output / log_name).open('wb'))
            child = subprocess.Popen(command, cwd=owned, env=environment, stdin=subprocess.DEVNULL,
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(NATIVE['stop_driver'], child)
            return child

        def wait(predicate, diagnostic):
            return NATIVE['wait_for'](predicate, diagnostic, timeout=18)

        if args.backend == 'wayland':
            env.pop('DISPLAY', None)
            env.pop('XAUTHORITY', None)
            config = owned / 'sway.conf'
            config.write_text('xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n')
            compositor = spawn(['sway', '--unsupported-gpu', '-c', str(config)], 'sway.log', dict(
                env, WLR_BACKENDS='headless', WLR_RENDERER='pixman', WLR_LIBINPUT_NO_DEVICES='1'))

            def display():
                assert compositor.poll() is None
                return next((entry.name for entry in Path(env['XDG_RUNTIME_DIR']).glob('wayland-*') if entry.is_socket()), None)

            env['WAYLAND_DISPLAY'] = wait(display, 'no private Wayland compositor socket')
            spawn(['wtype', '-s', '600000', '-k', 'F24'], 'seat.log')
        else:
            assert env.get('DISPLAY') and env.get('XAUTHORITY'), 'run in authenticated private Xvfb'

        requests, finish, _ = cleanup.enter_context(NATIVE['api_provider'](env))
        host = spawn([binary, '--jterm-session-host'], 'workspace-host.log')
        endpoint = owned / 'workspace' / 'run' / 'host.sock'

        def command(kind, values):
            return NATIVE['workspace_command'](endpoint, kind, values)['Ok']

        def catalog():
            return {row['id'] for row in command('CatalogPage', {'before': None, 'limit': 32})['CatalogPage']['rows']
                    if row['kind'] == 'Api'}

        def persisted(conversation):
            chat = command('LoadChat', {'id': conversation})['Chat']['chat']
            return {key: chat[key] for key in ('revision', 'draft_revision', 'snapshot_json', 'api_job')}

        wait(endpoint.is_socket, 'independent Workspace host did not bind')
        with socket.socket() as slot:
            slot.bind(('127.0.0.1', 0))
            port = slot.getsockname()[1]
        web = spawn(['WebKitWebDriver', '--host=127.0.0.1', f'--port={port}'], 'webdriver.log')
        driver = NATIVE['Driver'](port)
        cleanup.callback(driver.close)

        def driver_ready():
            assert web.poll() is None
            try:
                return driver.request('GET', '/status', timeout=1).get('ready')
            except OSError:
                return False

        def read():
            nonlocal last_state
            last_state = driver.execute(READ)
            return last_state

        def showing(field, expected):
            view = read()
            return view if view[field] == expected else None

        def show():
            driver.open(binary)
            wait(lambda: NATIVE['opened_page'](driver), 'native Aether page did not load')
            driver.command('POST', '/window/rect', {'width': 1380, 'height': 1000})
            for selector in ('#agent-workspace', '#workspace-api'):
                if not driver.execute(f"return document.querySelector('{selector}').open;"):
                    driver.click(f'{selector} > summary')
            wait(lambda: driver.execute("return !document.querySelector('#chat-new').disabled;"), 'API Workspace controls did not load')

        def save(text):
            driver.type('#chat-draft', text.replace('\n', '\ue007'))
            assert read()['draft'] == text
            driver.click('#chat-save')
            wait(lambda: read()['status'] == 'Draft saved.', 'exact API draft was not durably acknowledged')

        def create(text):
            before = catalog()
            driver.click('#chat-new')
            wait(lambda: read()['view'], 'new API conversation did not open')
            conversation, = catalog() - before
            save(text)
            return conversation

        def detach():
            driver.click('#chat-detach')
            wait(lambda: not read()['view'] and read()['status'] == 'Conversation detached.', 'API view was not detached')

        def open_chat(conversation):
            driver.click('#chat-refresh')
            def catalog_ready():
                value = read()
                return value if value['canOpen'] and conversation in value['options'] else None

            position = wait(catalog_ready, 'saved API conversation not listed')['options'].index(conversation) + 1
            driver.click(f'#chat-select option:nth-child({position})')
            assert driver.execute("return document.querySelector('#chat-select').value;") == conversation
            driver.click('#chat-open')
            wait(lambda: read()['view'], 'selected API conversation did not open')

        try:
            wait(driver_ready, 'private WebKitWebDriver did not start')
            show()
            other = create(OTHER_DRAFT)
            other_initial = persisted(other)
            detach()
            first = create(FIRST_DRAFT)
            assert first != other
            hold_next_job(driver, first)
            driver.click('#chat-send')
            held = wait(lambda: receipt(driver), 'native admission response was not withheld at the UI boundary')
            assert held['value']['identity']['conversation'] == first
            assert held['value']['job']['state'] in ('Starting', 'Running')
            epoch = held['value']['job']['epoch']
            wait(lambda: len(requests) == 1, 'the native host did not make exactly one provider request')
            assert requests[0]['stream'] is True and requests[0]['messages'][-1]['content'] == FIRST_DRAFT
            before_close = read()
            assert before_close['status'] == 'Waiting for Workspace host admission…'
            assert before_close['draft'] == FIRST_DRAFT and not before_close['preview'] and not before_close['canSend']
            assert persisted(first)['api_job']['epoch'] == int(epoch)
            driver.close()
            assert host.poll() is None

            show()
            open_chat(first)
            resumed = wait(lambda: showing('preview', 'First streamed segment. '),
                           'reopened WebView did not observe the existing API stream')
            assert resumed['draft'] == '' and resumed['canCancel'] and not resumed['canSend']
            assert persisted(first)['api_job']['epoch'] == int(epoch) and len(requests) == 1
            save(NEXT_DRAFT)
            hold_next_job(driver, first)
            late = wait(lambda: receipt(driver), 'no real background observation was available for delayed delivery')
            assert late['value']['job']['epoch'] == epoch
            detach()

            open_chat(other)
            before_late = read()
            assert before_late['draft'] == OTHER_DRAFT and before_late['history'] == [] and not before_late['job']
            assert driver.execute("""
                const fixture=window.heldWorkspaceReceipt;
                if (!fixture?.deliver) return false;
                fixture.deliver();
                delete window.heldWorkspaceReceipt;
                return true;
            """)
            after_late = read()
            assert after_late == before_late and persisted(other) == other_initial
            finish.set()
            wait(lambda: persisted(first)['api_job']['state'] == 'Completed', 'the original provider execution did not complete')
            assert read() == before_late and persisted(other) == other_initial and len(requests) == 1
            detach()

            open_chat(first)
            final = wait(lambda: showing('history', [FIRST_DRAFT, ANSWER]),
                         'exact committed user and assistant turns were not recovered')
            assert final['draft'] == NEXT_DRAFT and not final['canCancel']
            assert persisted(first)['api_job']['epoch'] == int(epoch) and persisted(other) == other_initial
            assert len(requests) == 1
            witnesses.update(first=first, other=other, receipt_epoch=epoch, initial_native_request=held['request'],
                             delayed_observe_request=late['request'], before_close=before_close, resumed=resumed,
                             before_late=before_late, after_late=after_late, final=final)
        except Exception:
            (output / 'last-ui-state.json').write_text(json.dumps(last_state, indent=2) + '\n')
            raise

    assert digest(binary) == args.sha256
    assert source_hashes == {str(source.relative_to(HERE.parent)): digest(source) for source in sources}
    result = {'case_id': CASE, 'result': 'passed', 'backend': args.backend, 'binary_sha256': args.sha256,
              'source_sha256': source_hashes, 'receipt_lost_before_native_close': True,
              'same_host_epoch_after_reopen': True, 'provider_request_count': 1,
              'unrelated_draft_and_history_preserved_after_late_native_receipt': True,
              'newer_draft_and_single_durable_turn_restored': True, 'witnesses': witnesses}
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: value for key, value in result.items() if key != 'witnesses'}, sort_keys=True))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--backend', required=True, choices=('x11', 'wayland'))
    parser.add_argument('--output', required=True, type=Path)
    run(parser.parse_args())
