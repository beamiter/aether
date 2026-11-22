const MAX_TRANSFER = 1024 * 1024;
const sameProject = (a, b) => a && b && a.view === b.view && a.host === b.host && a.root === b.root;
const join = (dir, name) => dir ? dir + '/' + name : name;
const absolute = (project, path) => project.root.replace(/\/$/, '') + '/' + path;

export function matchesFile(reference, project, path) {
  return reference?.source === 'ssh_project' && sameProject(reference.project, project) &&
    reference.path === path && Number.isSafeInteger(reference.version?.bytes) &&
    reference.version.bytes >= 0 && reference.version.bytes <= MAX_TRANSFER &&
    typeof reference.version.sha256 === 'string' && /^[0-9a-f]{64}$/.test(reference.version.sha256);
}

function encodeFile(bytes) {
  let binary = '';
  for (let start = 0; start < bytes.length; start += 8192) {
    binary += String.fromCharCode(...bytes.subarray(start, start + 8192));
  }
  return btoa(binary);
}

export function installSshFiles(document, invoke) {
  const el = name => document.getElementById('files-' + name);
  const entries = el('entries'), picker = el('local'), status = el('status');
  let folder = null, snapshot = null, intent = 0, busy = false, closing = false;

  function clearSnapshot() {
    snapshot = null;
    el('snapshot').hidden = true;
    el('text').textContent = el('version').textContent = '';
  }
  function selected() { return folder?.directory.entries.find(item => item.name === entries.value); }
  function controls() {
    const locked = busy || !folder;
    const entry = selected();
    el('host').disabled = el('root').disabled = el('open').disabled = busy || !!folder;
    entries.disabled = locked || !folder.directory.entries.length;
    el('refresh').disabled = locked;
    el('parent').disabled = locked || !folder.directory.path;
    el('next').disabled = locked || !folder.directory.next;
    el('enter').disabled = locked || entry?.kind !== 'directory';
    el('preview').disabled = el('download').disabled = locked || entry?.kind !== 'file';
    el('name').disabled = picker.disabled = locked;
    el('mkdir').disabled = locked || !el('name').value;
    el('upload').disabled = locked || picker.files?.length !== 1;
    el('close').disabled = !folder || closing;
  }
  function display(next) {
    folder = next; clearSnapshot(); entries.replaceChildren();
    el('browser').hidden = !folder;
    el('connect').hidden = !!folder;
    el('source').textContent = '';
    if (!folder) {
      picker.value = el('name').value = '';
      return;
    }
    const {project, directory} = folder;
    el('source').textContent = project.host + ' · ' + absolute(project, directory.path);
    for (const entry of directory.entries) {
      const option = document.createElement('option');
      option.textContent = entry.name + (entry.kind === 'directory' ? '/' : '');
      option.value = entry.name;
      entries.append(option);
    }
    if (directory.entries.length) entries.selectedIndex = 0;
  }
  const call = action => invoke('ssh_files', {action});
  const selection = () => ({view: folder.project.view, page_id: folder.page_id});
  function run(operation, interrupt = false) {
    if (busy && !interrupt) return;
    const ticket = ++intent;
    busy = true; closing = interrupt; controls(); status.textContent = '';
    const current = () => ticket === intent;
    return operation(current).catch(error => {
      if (current()) status.textContent = String(error?.message ?? error);
    }).finally(() => { if (current()) { busy = closing = false; controls(); } });
  }
  function setFolder(reply, project, path) {
    const next = reply?.folder;
    if (reply?.kind !== 'folder' || !next?.page_id || !next.project?.view ||
        (project && !sameProject(next.project, project)) || next.directory?.path !== path ||
        !Array.isArray(next.directory.entries) || next.directory.entries.length > 256) {
      throw Error('SSH Files returned a different folder.');
    }
    display(next);
    status.textContent = next.directory.entries.length || next.directory.next ? '' : 'Folder is empty.';
  }
  async function browse(current, navigation) {
    const {directory, project} = folder;
    let path = directory.path;
    if (navigation.kind === 'enter') path = join(path, navigation.name);
    if (navigation.kind === 'parent') path = path.split('/').slice(0, -1).join('/');
    const reply = await call({operation: 'browse', selection: selection(), navigation});
    if (current()) setFolder(reply, project, path);
  }
  el('connect').addEventListener('submit', event => {
    event.preventDefault();
    void run(async current => {
      const host = el('host').value.trim(), root = el('root').value;
      const reply = await call({operation: 'open', host, root});
      if (!current()) return;
      if (reply?.folder?.project.host !== host || reply.folder.project.root !== (root.replace(/\/+$/, '') || '/')) {
        throw Error('SSH project identity changed.');
      }
      setFolder(reply, null, '');
    });
  });
  el('close').addEventListener('click', () => {
    if (!folder || closing) return;
    const view = folder.project.view;
    clearSnapshot();
    void run(async current => {
      const reply = await call({operation: 'close', view});
      if (current() && reply.kind === 'closed') { display(null); status.textContent = 'SSH project closed.'; }
    }, true);
  });
  for (const kind of ['refresh', 'parent', 'next', 'enter']) {
    el(kind).addEventListener('click', () => run(current => browse(current,
      kind === 'enter' ? {kind, name: entries.value} : {kind})));
  }
  entries.addEventListener('change', () => { clearSnapshot(); status.textContent = ''; controls(); });

  async function read(current, preview) {
    const {project, directory} = folder;
    const name = entries.value, path = join(directory.path, name);
    const expected = preview ? null : snapshot?.version ?? null;
    if (preview) clearSnapshot();
    const reply = await call({operation: 'read', selection: selection(), name, preview, expected});
    if (!current() || entries.value !== name) return;
    const ref = preview ? reply.preview?.reference : reply.reference;
    if (!matchesFile(ref, project, path)) throw Error('SSH file source or version changed.');
    if (preview && reply.kind === 'preview' && typeof reply.preview.text === 'string') {
      snapshot = ref;
      el('version').textContent = project.host + ' · ' + absolute(project, path) + ' · SHA-256 ' + ref.version.sha256;
      el('text').textContent = reply.preview.text;
      el('snapshot').hidden = false;
    } else if (!preview && reply.kind === 'download' && (!expected || ref.version.sha256 === expected.sha256)) {
      const bytes = Uint8Array.from(atob(reply.base64), c => c.charCodeAt(0));
      if (bytes.length !== ref.version.bytes) throw Error('Incomplete SSH download.');
      const url = URL.createObjectURL(new Blob([bytes], {type: 'application/octet-stream'}));
      const link = document.createElement('a'); link.href = url; link.download = name;
      document.body.append(link);
      try { link.click(); } finally { link.remove(); setTimeout(() => URL.revokeObjectURL(url), 5000); }
      status.textContent = 'Download requested · SHA-256 ' + ref.version.sha256;
    } else throw Error('Unexpected SSH file receipt.');
  }
  el('preview').addEventListener('click', () => run(current => read(current, true)));
  el('download').addEventListener('click', () => run(current => read(current, false)));
  el('name').addEventListener('input', controls);
  picker.addEventListener('change', controls);
  async function create(current, upload) {
    const {project, directory} = folder, target = selection();
    const file = picker.files?.[0];
    const name = upload ? file?.name : el('name').value;
    let action = {operation: 'create_directory', selection: target, name};
    if (upload) {
      if (!file || file.size > MAX_TRANSFER) throw Error('Choose a file up to 1 MiB.');
      const bytes = new Uint8Array(await file.arrayBuffer());
      if (!current()) return;
      if (bytes.length !== file.size) throw Error('Local file size changed.');
      action = {operation: 'upload', selection: target, name, base64: encodeFile(bytes)};
    }
    const path = join(directory.path, name);
    const reply = await call(action);
    if (!current()) return;
    if (upload ? reply.kind !== 'uploaded' || !matchesFile(reply.reference, project, path) :
        reply.kind !== 'created_directory' || !sameProject(reply.project, project) || reply.path !== path) {
      throw Error('SSH creation was not confirmed. Refresh the folder.');
    }
    const confirmed = 'Created ' + project.host + ' · ' + absolute(project, path);
    if (upload) picker.value = ''; else el('name').value = '';
    try { await browse(current, {kind: 'refresh'}); }
    catch (error) { if (current()) status.textContent = confirmed + '. Refresh failed: ' + error; return; }
    if (current()) status.textContent = confirmed;
  }
  for (const [form, upload] of [['upload-form', true], ['new-directory', false]]) {
    el(form).addEventListener('submit', event => { event.preventDefault(); void run(current => create(current, upload)); });
  }
  document.getElementById('ssh-files').hidden = false;
  void run(async current => {
    const reply = await call({operation: 'inspect'});
    if (current()) display(reply.kind === 'folder' ? reply.folder : null);
  });
}

const invoke = globalThis.__TAURI__?.core?.invoke;
if (invoke) installSshFiles(document, invoke);
