(async () => {
  const get = id => document.getElementById(id);
  const click = id => get(id).click();
  function assert(condition, message) {
    if (!condition) throw Error(message);
  }
  async function waitFor(label, check) {
    const deadline = performance.now() + 4_000;
    do {
      if (check()) return;
      await new Promise(resolve => setTimeout(resolve, 35));
    } while (performance.now() < deadline);
    throw Error(`${label}: ${get('status').textContent} ${get('preview-notice').textContent} ${get('workspace-status').textContent} ${get('workspace-history-status').textContent}`);
  }
  function selectFile(file) {
    const input = get('preview-single');
    const files = new DataTransfer();
    files.items.add(file);
    input.files = files.files;
    input.dispatchEvent(new Event('change', {bubbles: true}));
  }

  try {
    await waitFor('Shell controls unavailable', () => window.__TAURI__?.core?.invoke && !get('open').disabled);
    const viewport = document.querySelector('.viewport');
    const fittedGrid = () => {
      const cols = Math.floor((viewport.clientWidth - 24) / 12);
      const rows = Math.floor((viewport.getBoundingClientRect().height - 26) / 24);
      return `${cols} × ${rows} cells`;
    };
    const initial = fittedGrid();
    click('open');
    await waitFor('Fitted initial PTY frame missing', () => get('cursor-status').textContent.includes(initial));
    const terminal = document.querySelector('.terminal-surface canvas');
    assert(terminal.getContext('2d').getImageData(0, 0, 1, 1).data[3] === 255, 'Terminal Canvas pixels missing');
    get('input').value = "printf 'AETHER_%s\\nTERM_%s_%s\\n' PTY_SMOKE \"$TERM\" \"$(tput colors)\"";
    click('send');
    await waitFor('Shell output missing', () => {
      click('copy-visible');
      const lines = get('history-output').value.split('\n');
      return lines.includes('AETHER_PTY_SMOKE') && lines.includes('TERM_aether_256');
    });

    globalThis.aetherSmokeResize = true;
    await waitFor('Automatically resized PTY frame missing', () =>
      fittedGrid() !== initial && get('cursor-status').textContent.includes(fittedGrid()));
    globalThis.aetherSmokeResize = false;
    get('rows').value = '16';
    get('cols').value = '68';
    click('resize');
    await waitFor('Resized PTY frame missing', () => get('cursor-status').textContent.includes('68 × 16 cells'));
    assert(!get('fit-size').checked, 'Manual geometry did not disable automatic fitting');

    get('agent-workspace').open = true;
    const conversations = get('workspace-rows');
    await waitFor('CLI and shared API fixtures missing', () =>
      conversations.textContent.includes('Aether CLI fixture') &&
      conversations.textContent.includes('Aether API fixture'));
    assert(!conversations.textContent.includes('Aether archived task fixture'),
      'Archived task appeared in the default active library');
    const kind = get('workspace-kind');
    function choose(control, value) {
      control.value = value;
      control.dispatchEvent(new Event('change', {bubbles: true}));
    }
    choose(get('workspace-library'), 'archived');
    choose(kind, 'task');
    await waitFor('Archived task fixture missing', () =>
      conversations.textContent.includes('Aether archived task fixture') &&
      !conversations.textContent.includes('Aether API fixture'));
    choose(get('workspace-library'), 'active');
    choose(kind, 'api');
    await waitFor('Type filter did not select the API catalog', () =>
      conversations.textContent.includes('Aether API fixture') &&
      !conversations.textContent.includes('Aether CLI fixture'));
    const workspaceQuery = get('workspace-query');
    workspaceQuery.value = 'aether-needle';
    workspaceQuery.dispatchEvent(new Event('input', {bubbles: true}));
    await waitFor('Shared indexed history match missing', () =>
      get('workspace-hits').querySelector('button')?.textContent.includes('aether-needle'));
    get('workspace-hits').querySelector('button').click();
    const evidence = get('workspace-detail-text');
    await waitFor('Exact history evidence missing', () =>
      evidence.textContent.includes('aether-needle <b>literal evidence</b>'));
    assert(!evidence.querySelector('b'), 'Workspace history HTML was rendered');
    click('workspace-clear');
    const cliButton = () => [...conversations.querySelectorAll('button')].find(button =>
      button.textContent.includes('Aether CLI fixture'));
    await waitFor('Saved CLI entry missing', () => Boolean(cliButton()));
    cliButton().click();
    await waitFor('Saved CLI deletion control unavailable', () =>
      !get('workspace-delete').hidden && !get('workspace-delete').disabled);
    click('workspace-delete');
    await waitFor('CLI deletion confirmation missing', () =>
      get('workspace-delete-confirmation').open &&
      get('workspace-delete-target').textContent.includes('Aether CLI fixture'));
    click('workspace-delete-cancel');
    assert(cliButton() && !get('workspace-delete-confirmation').open,
      'Cancel changed the saved CLI entry');
    click('workspace-delete');
    await waitFor('Second CLI deletion confirmation missing', () =>
      get('workspace-delete-confirmation').open);
    click('workspace-delete-confirm');
    await waitFor('Saved CLI fixture did not disappear', () =>
      !cliButton() && get('workspace-delete-message').textContent.includes('Saved CLI session deleted.'));
    click('workspace-refresh');
    await waitFor('Refreshed API catalog missing', () =>
      conversations.textContent.includes('Aether API fixture') && !get('workspace-refresh').disabled);
    assert(!cliButton(), 'Deleted record returned after rescan');
    assert(get('cursor-status').textContent.includes('68 × 16 cells'), 'Workspace browsing replaced the live terminal');
    get('agent-workspace').open = false;

    click('close');
    await waitFor('Shell close incomplete', () => !get('open').disabled);
    assert(get('health').textContent === '', 'Shell health remained after close');

    selectFile(new File(['alpha <b>literal</b>\r\nbeta alpha\n'], 'native-preview.md'));
    click('preview-show');
    const source = get('preview-source');
    await waitFor('Text preview missing', () => source.textContent.includes('<b>literal</b>'));
    assert(!source.querySelector('b'), 'Source HTML was rendered');
    const query = get('preview-query');
    query.value = 'alpha';
    query.dispatchEvent(new Event('input', {bubbles: true}));
    assert(get('preview-matches').textContent === '2 matches', 'Expected two literal occurrences');
    click('preview-next');
    click('preview-next');
    assert(get('preview-matches').textContent === 'Match 2 of 2', 'Second occurrence was not selected');
    assert(get('preview-line-status').textContent.startsWith('Line 2 of '), 'Second source line was not revealed');
    // Supply a fresh browser File through the same control without opening a
    // chooser in the synthetic input environment.
    get('preview-single').addEventListener('click', event => event.preventDefault(), {once: true});
    click('preview-reload');
    selectFile(new File(['alpha <b>literal</b>\r\nbeta alpha\nupdated\n'], 'native-preview.md'));
    await waitFor('Reloaded preview missing', () => source.textContent.includes('updated'));
    assert(query.value === 'alpha' && get('preview-matches').textContent === 'Match 2 of 2',
      'Reload did not preserve search position');
    click('preview-close');

    const png = Uint8Array.from(atob(aetherSmokeFixtures.png), ch => ch.charCodeAt(0));
    selectFile(new File([png], 'native-preview.png', {type: 'image/png'}));
    click('preview-show');
    const raster = get('preview-raster');
    await waitFor('Decoded PNG pixels missing', () =>
      !raster.hidden && raster.width === 2 && raster.getContext('2d').getImageData(0, 0, 1, 1).data[3] === 255);
    const zoom = get('preview-zoom');
    zoom.value = '400';
    zoom.dispatchEvent(new Event('change', {bubbles: true}));
    assert(Math.abs(parseFloat(raster.style.width) - 8 / devicePixelRatio) < 0.01,
      'PNG zoom width is incorrect');
    click('preview-close');
    assert(get('file-preview').hidden && raster.width === 1, 'Dismissal did not clear preview pixels');
    globalThis.aetherSmokeResult = true;
  } catch (error) {
    try {
      if (get('close') && !get('close').disabled) {
        click('close');
        await waitFor('Shell cleanup incomplete', () => !get('open').disabled);
      }
      globalThis.aetherSmokeResult = String(error);
    } catch (cleanupError) {
      globalThis.aetherSmokeResult = `${error}. ${cleanupError}`;
    }
  }
})();
