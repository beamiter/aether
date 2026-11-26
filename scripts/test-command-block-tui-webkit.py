#!/usr/bin/env python3
"""Exact native single-command alternate-screen/output-card oracle.

Run under private authenticated Xvfb, or use --backend wayland for private Sway.
The real integrated shell, real Vim, tracked source tree and three executables
are independently hash-checked before and after the native journey.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import runpy
import shlex
import socket
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
L = runpy.run_path(str(HERE / "test-layout-webkit.py"))
F, require, digest = L["F"], L["require"], L["digest"]
def verify_inputs(args):
    import re
    root = HERE.parent
    revision = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    require(revision == args.expected_source_revision, 'unexpected application source revision')
    subprocess.run(['git', '-C', str(root), 'diff', '--quiet', 'HEAD', '--'], check=True)
    paths = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z']).decode().rstrip('\0').split('\0')
    sources = {path: digest(root / path) for path in paths}
    for crate in ['src-tauri', 'bridge', 'session']:
        manifest = (root / crate / 'Cargo.toml').read_text()
        pins = re.findall(r'git = "https://github.com/beamiter/jterm_core(?:\.git)?", rev = "([a-f0-9]{40})"', manifest)
        lock = (root / crate / 'Cargo.lock').read_text()
        locks = re.findall(r'git\+https://github.com/beamiter/jterm_core(?:\.git)?\?rev=([a-f0-9]{40})#([a-f0-9]{40})', lock)
        require(pins and all(pin == args.expected_core_revision for pin in pins), 'application manifest pin changed')
        require(locks and all(a == b == args.expected_core_revision for a, b in locks), 'application lock pin changed')
    binaries = {}
    for path, expected in [(args.binary, args.sha256), (args.jsh, args.jsh_sha256), (args.vim, args.vim_sha256)]:
        require(path.is_file() and not path.is_symlink() and os.access(path, os.X_OK), 'regular executable required')
        require(digest(path) == expected, 'executable input hash changed')
        binaries[str(path.absolute())] = expected
    return {'aether_commit': revision, 'core_commit': args.expected_core_revision,
            'tracked_source_sha256': sources, 'executable_sha256': binaries}



def run(args):
    import time, hashlib
    binary=args.binary.absolute()
    require(digest(binary)==args.sha256,"wrong artifact")
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    driver = None
    with tempfile.TemporaryDirectory(prefix="aether-command-block-", ignore_cleanup_errors=True) as temporary, contextlib.ExitStack() as cleanup:
        owned = Path(temporary)
        env = {key: os.environ[key] for key in ["PATH", "DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "XAUTHORITY"] if key in os.environ}
        env.update(GDK_BACKEND=args.backend, LC_ALL="C.UTF-8", LIBGL_ALWAYS_SOFTWARE="1", TAURI_WEBVIEW_AUTOMATION="true")
        for key in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"]:
            directory = owned / key
            directory.mkdir(mode=0o700)
            env[key] = str(directory)

        launch=owned/'bin';launch.mkdir()
        (launch/'jsh').symlink_to(args.jsh.absolute())
        (launch/'vim').symlink_to(args.vim.absolute())
        env['PATH']=str(launch)+':/usr/bin:/bin'

        def spawn(command, name, environment=env):
            log = cleanup.enter_context((output / name).open("wb"))
            child = subprocess.Popen(command, cwd=owned, env=environment, stdin=subprocess.DEVNULL,
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            cleanup.callback(F["stop_driver"], child)
            return child

        if args.backend == "wayland":
            env.pop("DISPLAY", None)
            env.pop("XAUTHORITY", None)
            config = owned / "sway.conf"
            config.write_text("xwayland disable\noutput * resolution 1440x1100\nseat seat0 fallback true\n")
            sway = spawn(["sway", "--unsupported-gpu", "-c", str(config)], "sway.log",
                         dict(env, WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1"))

            def display():
                require(sway.poll() is None, "private compositor exited")
                return next((p.name for p in Path(env["XDG_RUNTIME_DIR"]).glob("wayland-*") if p.is_socket()), None)

            env["WAYLAND_DISPLAY"] = F["wait_for"](display, "private Wayland display did not start")
            spawn(["wtype", "-s", "600000", "-k", "F24"], "seat.log")
        else:
            require(env.get("DISPLAY") and env.get("XAUTHORITY"), "run with private authenticated Xvfb")
        with socket.socket() as slot:
            slot.bind(("127.0.0.1", 0))
            port = slot.getsockname()[1]
        web = spawn(["WebKitWebDriver", "--host=127.0.0.1", f"--port={port}"], "webdriver.log")
        driver = F["Driver"](port)

        def wait(predicate, label):
            return F["wait_for"](predicate, label, timeout=20)

        def ready():
            require(web.poll() is None, "WebKitWebDriver exited")
            try:
                return driver.request("GET", "/status", timeout=1).get("ready")
            except OSError:
                return False

        wait(ready, "WebDriver did not listen")
        cleanup.callback(driver.close)

        driver.open(binary)
        wait(lambda:F["opened_page"](driver),"native page not loaded")
        driver.command("POST","/window/rect",{"width":1380,"height":1000})
        terminal='#ime-input'
        def state():return driver.execute(F['READ'])
        def view():return driver.execute("return {surface:document.querySelector('#active-pane').dataset.chatSurface,cards:Array.from(document.querySelectorAll('#command-block-list details'),c=>({sequence:c.dataset.sequence,revision:c.dataset.revision,command:c.querySelector('code').textContent,body:c.textContent,output:c.querySelector('textarea')?.value??null}))}")
        driver.click('#open-integrated');wait(lambda:state()['writable'],'integrated shell not writable')
        driver.type('#terminal-chat-toggle','\ue007');wait(lambda:view()['surface']=='prompt','Chat prompt absent')
        pid=owned/'block-shell.pid';tty=owned/'block-shell.tty';once=owned/'block-once';helper=owned/'mixed-tui.py';editor=owned/'real-block-vim.txt';after=owned/'same-shell-after'
        helper.write_text("import os,subprocess,sys,pathlib,json\np=pathlib.Path("+repr(str(owned))+")\n(p/'block-once').open('a').write('once\\n')\nprint('BLOCK_PRE_TAG',flush=True)\nv=subprocess.Popen(['vim','-Nu','NONE','-i','NONE','-n','-X',str(p/'real-block-vim.txt')])\n(p/'real-tui-identity.json').write_text(json.dumps({'pid':v.pid,'tty':os.ttyname(0),'parent':os.getpid()}))\ncode=v.wait()\nprint('BLOCK_POST_TAG',flush=True)\nsys.exit(7 if code==0 else 99)\n")
        command=f"printf '%s' \"$$\" > {shlex.quote(str(pid))}; tty > {shlex.quote(str(tty))}; /usr/bin/python3 {shlex.quote(str(helper))}"
        for offset in range(0,len(command),64):driver.type(terminal,command[offset:offset+64])
        driver.click('#terminal-chat-send')
        wait(lambda:view()['surface']=='fullscreen','real Vim never reached alternate-screen surface')
        identity=F['shell_identity'](pid);beforetty=tty.read_text();tui=json.loads((owned/'real-tui-identity.json').read_text())
        require(tui['tty']==beforetty.strip() and F['birth'](identity[0])==identity[1],'TUI changed live shell or PTY')
        if args.backend=='wayland':
            driver.execute("document.querySelector('#ime-input').focus();")
            for keys in [['wtype','iBLOCK_REAL_VIM_EDIT'],['wtype','-k','Escape'],['wtype',':wq'],['wtype','-k','Return']]:subprocess.run(keys,env=env,check=True,timeout=3)
        else:driver.type(terminal,'iBLOCK_REAL_VIM_EDIT\ue00c:wq\ue007')
        wait(lambda:view()['surface']=='prompt','same shell did not resume after Vim')
        wait(lambda:editor.is_file() and editor.read_bytes()==b'BLOCK_REAL_VIM_EDIT\n','actual TUI did not receive/save typed text')
        wait(lambda:any(c['command']==command for c in view()['cards']),'mixed command output card absent')
        matched=[c for c in view()['cards'] if c['command']==command]
        require(len(matched)==1 and once.read_text()=='once\n','single command duplicated lifecycle/card')
        card=matched[0]
        diagnostic={'case_id':'PRODUCT.BLOCK.001.NATIVE.001','backend':args.backend,'binary_sha256':args.sha256,'command':command,'card':card,'shell_identity':identity,'shell_tty':beforetty.strip(),'tui_identity':tui,'actual_tui_saved_hex':editor.read_bytes().hex(),'one_invocation_one_card':True}
        (output/'mixed-tui-observation.json').write_text(json.dumps(diagnostic,indent=2)+'\n')
        require(card['output']=='BLOCK_PRE_TAG\nBLOCK_POST_TAG\n' and 'Exit 7' in card['body'],'mixed TUI card did not retain exact main-screen tagged output/exit: '+str(card))
        selector=f"#command-block-list details[data-sequence='{card['sequence']}']"
        if not driver.execute(f"return document.querySelector(\"{selector}\").open"):driver.click(selector+' summary')
        wait(lambda:driver.execute(f"return document.querySelector(\"{selector}\").open"),'output card did not open')
        driver.click(selector+' summary');require(not driver.execute(f"return document.querySelector(\"{selector}\").open"),'card did not fold')
        driver.click(selector+' summary');require(driver.execute(f"return document.querySelector(\"{selector}\").open"),'card did not reopen')
        require(next(c for c in view()['cards'] if c['sequence']==card['sequence'])==card,'fold/reopen changed immutable output')
        poison=b'UNRELATED_BLOCK_CLIPBOARD'
        writer=['wl-copy'] if args.backend=='wayland' else ['xclip','-selection','clipboard']
        reader=['wl-paste','--no-newline'] if args.backend=='wayland' else ['xclip','-selection','clipboard','-o']
        subprocess.run(writer,input=poison,env=env,check=True,timeout=3)
        require(subprocess.check_output(reader,env=env,timeout=3)==poison,'clipboard poison not installed')
        buttons=driver.execute(f"return Array.from(document.querySelector(\"{selector}\").querySelectorAll('button')).map((b,i)=>({{i,text:b.textContent}}))")
        button=next(b['i'] for b in buttons if b['text']=='Copy output')
        # Buttons may have different parent action groups; focus the exact
        # public button, then activate it through a native keyboard event.
        driver.execute(f"Array.from(document.querySelector(\"{selector}\").querySelectorAll('button'))[{button}].focus();")
        if args.backend=='wayland':subprocess.run(['wtype','-k','Return'],env=env,check=True,timeout=3)
        else:driver.command('POST','/actions',{'actions':[{'type':'key','id':'copy-key','actions':[{'type':'keyDown','value':'\ue007'},{'type':'keyUp','value':'\ue007'}]}]})
        expected=b'BLOCK_PRE_TAG\nBLOCK_POST_TAG\n'
        wait(lambda:subprocess.check_output(reader,env=env,timeout=3)==expected,'output card OS clipboard bytes changed')
        clipboard=subprocess.check_output(reader,env=env,timeout=3)
        check=f"printf '%s|' \"$$\" > {shlex.quote(str(after))}; tty >> {shlex.quote(str(after))}"
        for offset in range(0,len(check),64):driver.type(terminal,check[offset:offset+64])
        driver.click('#terminal-chat-send')
        wait(lambda:after.is_file() and after.read_text()==str(identity[0])+'|'+beforetty,'return did not preserve original shell/PTY')
        require(F['birth'](identity[0])==identity[1] and once.read_text()=='once\n','shell replaced or invocation duplicated')
        require([c for c in view()['cards'] if c['command']==command]==[card],'later shell activity mutated original output card')
        driver.click('#close');wait(lambda:F['birth'](identity[0])!=identity[1],'original shell not reaped')
        result={**diagnostic,'result':'passed','one_invocation_one_immutable_card':True,'real_alternate_screen_tui_saved_text':True,'same_original_shell_pid_birth_tty_after_tui':True,'fold_reopen_exact_output_preserved':True,'clipboard_poison_replaced_exact_bytes':True,'clipboard_hex':clipboard.hex(),'original_shell_reaped':True}
        (output/'result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
    require(digest(binary)==args.sha256,"binary changed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary",type=Path)
    parser.add_argument("--sha256",required=True)
    parser.add_argument("--backend",choices=["x11","wayland"],required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--expected-source-revision",required=True)
    parser.add_argument("--expected-core-revision",required=True)
    parser.add_argument("--jsh",type=Path,required=True)
    parser.add_argument("--jsh-sha256",required=True)
    parser.add_argument("--vim",type=Path,required=True)
    parser.add_argument("--vim-sha256",required=True)
    args = parser.parse_args()
    before = verify_inputs(args)
    try:
        run(args)
    finally:
        after = verify_inputs(args)
        require(before == after, 'source or executable inputs changed during native test')
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / 'source-and-executable-inputs.json').write_text(json.dumps(before, indent=2) + '\n')

