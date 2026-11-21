// Only control state crosses the WebView boundary. The native JWM viewer owns
// keyboard capture, transport, decode and presentation.
const POLL_MS = 120;
const MAX_RECENT = 8;
const CLIPBOARD_CONTENT = new Map([['text', 'Text'], ['text-images', 'Text and images'], ['all', 'Text, images and files']]);
const CLIPBOARD_DIRECTION = new Map([['from-host', 'From host'], ['to-host', 'To host'], ['both', 'Both ways']]);
const CLIPBOARD_TRANSFER_KIND = new Map([['image', 'Image'], ['files', 'Files']]);
const BYTE_FORMAT = new Intl.NumberFormat('en');

function clipboardBytes(value) {
  if (typeof value !== 'string' || !/^[0-9]{1,20}$/.test(value)) return null;
  const bytes = BigInt(value);
  return bytes <= 18446744073709551615n ? bytes : null;
}

export function installRemoteDesktop(document, invoke) {
  if (!invoke) return;
  const byId = id => document.getElementById(id);
  const terminalTab = byId('terminal-tab'), remoteTab = byId('remote-tab');
  const terminalPanel = byId('terminal-panel'), remotePanel = byId('remote-panel');
  const address = byId('remote-address'), keyFile = byId('remote-key');
  const recentSelect = byId('remote-recent'), viewer = byId('remote-viewer');
  const scaling = byId('remote-scaling'), control = byId('remote-control');
  const reconnect = byId('remote-reconnect'), reconnectRow = byId('remote-reconnect-row');
  const gestures = byId('remote-gestures'), gesturesRow = byId('remote-gestures-row');
  const clipboard = byId('remote-clipboard'), clipboardRow = byId('remote-clipboard-row');
  const clipboardOptions = byId('remote-clipboard-options'), clipboardConsent = byId('remote-clipboard-consent');
  const clipboardMode = byId('remote-clipboard-mode'), clipboardDirection = byId('remote-clipboard-direction');
  const fileLimit = byId('remote-clipboard-file-limit'), fileLimitRow = byId('remote-clipboard-file-limit-row');
  const clipboardStatus = byId('remote-clipboard-status'), clipboardTransfer = byId('remote-clipboard-transfer');
  const connect = byId('remote-connect'), release = byId('remote-release');
  const disconnect = byId('remote-disconnect'), source = byId('remote-source');
  const form = byId('remote-form');
  const status = byId('remote-status'), feedback = byId('remote-feedback');
  const presentation = byId('remote-presentation');
  const metrics = byId('remote-metrics');

  const recent = new Map();
  let activeHost = null;
  let snapshot = {generation: 0, active: false, closing: false, releaseAvailable: false, sources: []};
  let visible = false, timer = null, polling = null, commandPending = false;
  let tabUpdate = Promise.resolve();
  let error = '', ready = false;
  let shownSources = '', shownRecent = '';

  function replaceOptions(select, entries) {
    const options = entries.map(([value, label]) => {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = label;
      return option;
    });
    select.replaceChildren(...options);
  }

  function renderClipboard(editable) {
    const available = viewer.value === 'x11';
    const optedIn = available && clipboard.checked;
    clipboardRow.hidden = !available;
    clipboard.disabled = !editable || !available;
    clipboardOptions.hidden = !optedIn || snapshot.active;
    clipboardMode.disabled = clipboardDirection.disabled = !editable || !optedIn;
    fileLimitRow.hidden = clipboardMode.value !== 'all';
    fileLimit.disabled = !editable || !optedIn || fileLimitRow.hidden;
    let consent = 'The host must allow clipboard sharing.';
    if (clipboardDirection.value !== 'from-host') consent += ' Sending can share the current local clipboard.';
    if (clipboardMode.value === 'all') consent += ' A limit of 0 disables files.';
    if (clipboardConsent.textContent !== consent) clipboardConsent.textContent = consent;

    const effective = snapshot.active && !snapshot.closing ? snapshot.clipboard : null;
    const content = CLIPBOARD_CONTENT.get(effective?.mode);
    const direction = CLIPBOARD_DIRECTION.get(effective?.direction);
    const limit = clipboardBytes(effective?.file_limit_bytes);
    let policy = '';
    if (content && direction) {
      policy = `Clipboard · ${content} · ${direction}`;
      if (effective.mode === 'all' && limit !== null) policy += ` · File limit ${BYTE_FORMAT.format(limit)} bytes`;
    } else if (effective) {
      policy = 'Clipboard off';
    } else if (snapshot.active && snapshot.clipboardRequested && !snapshot.closing) {
      policy = 'Clipboard status unavailable';
    }
    if (clipboardStatus.textContent !== policy) clipboardStatus.textContent = policy;
    clipboardStatus.hidden = !policy;

    const transfer = effective?.transfer;
    const transferDirection = CLIPBOARD_DIRECTION.get(transfer?.direction);
    const kind = CLIPBOARD_TRANSFER_KIND.get(transfer?.kind);
    const processed = clipboardBytes(transfer?.bytes_processed);
    const total = clipboardBytes(transfer?.total_bytes);
    const transferAllowed = transferDirection && transfer.direction !== 'both'
      && (effective.direction === 'both' || effective.direction === transfer.direction)
      && effective.mode !== 'text' && (transfer.kind !== 'files' || (effective.mode === 'all' && limit > 0n));
    const progress = content && direction && kind && transferAllowed
      && processed !== null && total !== null && processed <= total
      ? `${kind} ${transferDirection.toLowerCase()} · ${BYTE_FORMAT.format(processed)} of ${BYTE_FORMAT.format(total)} bytes processed`
      : '';
    if (clipboardTransfer.textContent !== progress) clipboardTransfer.textContent = progress;
    clipboardTransfer.hidden = !progress;
  }

  function reconnectAvailable() {
    return viewer.value === 'x11' && !control.checked && !clipboard.checked;
  }

  function render() {
    const editable = ready && !snapshot.active && !commandPending;
    form.toggleAttribute('data-embedded', Boolean(snapshot.active && snapshot.embedded));
    for (const field of [address, keyFile, viewer, scaling, connect]) field.disabled = !editable;
    control.disabled = !editable;
    const canRequestGestures = viewer.value === 'wayland' && control.checked;
    gesturesRow.hidden = !canRequestGestures;
    gestures.disabled = !editable || !canRequestGestures;
    if (!canRequestGestures) gestures.checked = false;
    renderClipboard(editable);
    const canReconnect = reconnectAvailable();
    reconnectRow.hidden = !canReconnect;
    reconnect.disabled = !editable || !canReconnect;
    if (!canReconnect) reconnect.checked = false;
    recentSelect.disabled = !editable || recent.size === 0;
    disconnect.disabled = !snapshot.active || snapshot.closing || commandPending;
    release.disabled = !(snapshot.releaseAvailable || snapshot.requestAvailable) || snapshot.closing || commandPending;
    release.textContent = snapshot.requestAvailable ? 'Request Input' : 'Release Input';
    source.disabled = snapshot.sources.length < 2 || snapshot.closing || commandPending;

    const recentOptions = [['', 'Recent hosts'], ...Array.from(recent, ([host, result]) =>
      [host, result ? host + ' · ' + result : host])];
    const recentSignature = JSON.stringify(recentOptions);
    if (shownRecent !== recentSignature) {
      shownRecent = recentSignature;
      replaceOptions(recentSelect, recentOptions);
    }
    const recentValue = recent.has(address.value) ? address.value : '';
    if (recentSelect.value !== recentValue) recentSelect.value = recentValue;

    const sourceOptions = snapshot.sources.map(entry =>
      [JSON.stringify(entry.selection), entry.label]);
    const sourceSignature = JSON.stringify(sourceOptions);
    if (shownSources !== sourceSignature) {
      shownSources = sourceSignature;
      replaceOptions(source, sourceOptions.length ? sourceOptions : [['', 'No source']]);
    }
    const currentSource = snapshot.currentSource ? JSON.stringify(snapshot.currentSource.selection) : '';
    if (source.value !== currentSource) source.value = currentSource;

    const stateText = snapshot.status || 'Disconnected';
    if (status.textContent !== stateText) status.textContent = stateText;
    presentation.hidden = !snapshot.active;
    const presentationText = snapshot.embedded ? 'Video in this window' : 'Separate viewer window';
    if (presentation.textContent !== presentationText) presentation.textContent = presentationText;
    const message = error || snapshot.feedback || '';
    if (feedback.textContent !== message) feedback.textContent = message;
    feedback.hidden = !message;
    const stats = snapshot.metrics || '';
    if (metrics.textContent !== stats) metrics.textContent = stats;
    metrics.hidden = !snapshot.metrics;
  }

  function accept(next) {
    snapshot = next;
    ready = true;
    if (activeHost && !snapshot.active) {
      recent.set(activeHost, snapshot.status);
      activeHost = null;
      clipboard.checked = false;
      reconnect.checked = false;
    }
    render();
  }

  function schedule() {
    clearTimeout(timer);
    if (visible && snapshot.active && !commandPending) timer = setTimeout(inspect, POLL_MS);
  }

  function invokeAction(action) {
    return invoke('remote_desktop', {action});
  }

  async function inspect() {
    if (polling || commandPending || !visible) return;
    polling = (async () => {
      try {
        accept(await invokeAction({type: 'inspect'}));
      } catch (reason) {
        error = String(reason?.message ?? reason);
        render();
      }
    })();
    await polling;
    polling = null;
    schedule();
  }

  async function command(type, parameters = {}) {
    if (!ready || commandPending) return;
    commandPending = true;
    error = '';
    clearTimeout(timer);
    render();
    try {
      await tabUpdate;
      await polling;
      const next = await invokeAction({type, generation: snapshot.generation, ...parameters});
      if (type === 'connect') {
        activeHost = parameters.options.address;
        recent.delete(activeHost);
        const latest = [[activeHost, ''], ...recent].slice(0, MAX_RECENT);
        recent.clear();
        for (const [host, result] of latest) recent.set(host, result);
      }
      accept(next);
    } catch (reason) {
      error = String(reason?.message ?? reason);
    } finally {
      commandPending = false;
      render();
      schedule();
    }
  }

  function showRemote(show) {
    visible = show;
    terminalPanel.hidden = show;
    remotePanel.hidden = !show;
    terminalTab.setAttribute('aria-pressed', String(!show));
    remoteTab.setAttribute('aria-pressed', String(show));
    clearTimeout(timer);
    if (show) error = '';
    tabUpdate = tabUpdate.then(async () => {
      const next = await invokeAction({type: 'set_visible', visible: show});
      if (visible === show && !commandPending) accept(next);
    }).catch(reason => {
      error = String(reason?.message ?? reason);
      render();
    }).finally(schedule);
  }

  terminalTab.addEventListener('click', () => showRemote(false));
  remoteTab.addEventListener('click', () => showRemote(true));
  address.addEventListener('input', render);
  recentSelect.addEventListener('change', () => {
    if (recentSelect.value) address.value = recentSelect.value;
    render();
  });
  viewer.addEventListener('change', () => {
    control.checked = false;
    clipboard.checked = false;
    render();
  });
  control.addEventListener('change', render);
  for (const field of [clipboard, clipboardMode, clipboardDirection]) field.addEventListener('change', render);
  form.addEventListener('submit', event => {
    event.preventDefault();
    const options = {
      address: address.value.trim(),
      keyFile: keyFile.value,
      viewer: viewer.value,
      scaling: scaling.value,
      control: control.checked,
    };
    if (viewer.value === 'wayland' && control.checked && gestures.checked) {
      options.waylandGestures = true;
    }
    if (reconnect.checked && reconnectAvailable()) options.reconnectAutomatically = true;
    if (viewer.value === 'x11' && clipboard.checked) {
      const fileLimitBytes = clipboardMode.value === 'all' ? clipboardBytes(fileLimit.value.trim()) : 0n;
      if (fileLimitBytes === null) {
        error = 'Enter a file limit from 0 to 18,446,744,073,709,551,615 bytes.';
        render();
        return;
      }
      options.clipboard = {mode: clipboardMode.value, direction: clipboardDirection.value, fileLimitBytes: String(fileLimitBytes)};
    }
    if (options.address) void command('connect', {options});
  });
  source.addEventListener('change', () => {
    const chosen = snapshot.sources.find(entry => JSON.stringify(entry.selection) === source.value);
    if (chosen) void command('select_source', {selection: chosen.selection});
  });
  release.addEventListener('click', () => {
    if (snapshot.requestAvailable && snapshot.currentSource) {
      void command('request_input', {selection: snapshot.currentSource.selection});
    } else {
      void command('release_input');
    }
  });
  disconnect.addEventListener('click', () => void command('disconnect'));
  byId('workspace-nav').hidden = false;
  render();
}

if (typeof document !== 'undefined') {
  installRemoteDesktop(document, globalThis.__TAURI__?.core?.invoke);
}
