const legacyKey = 'aether.local-shell-layout';
const modernKey = 'aether.local-shell-layout.v5';
export const MAX_LOCAL_SHELLS = 8;
export const plainLocalTab = Object.freeze({shell: 'plain', presentation: 'terminal'});
export const validPaneId = value => typeof value === 'string' && /^[a-f0-9]{32}$/.test(value);
export const validLocalGeometry = value => value && Object.keys(value).length === 2 &&
  Number.isInteger(value.rows) && value.rows >= 1 && value.rows <= 128 &&
  Number.isInteger(value.cols) && value.cols >= 1 && value.cols <= 256;

// The two visible panes are independent from the bounded tab roster. Switching
// to a parked tab replaces the focused pane, not its observed peer.
export function localSplitPair(epochs, active, pair = [], previous = null) {
  const result = [...new Set(pair.filter(epoch => epochs.includes(epoch)))].slice(0, 2);
  for (const epoch of epochs) if (result.length < 2 && !result.includes(epoch)) result.push(epoch);
  if (result.length < 2) return [];
  if (epochs.includes(active) && !result.includes(active)) {
    const index = result.indexOf(previous);
    result[index < 0 ? 0 : index] = active;
  }
  return result;
}

function validLayout(tabs, active, split, panes, modern = false) {
  if (!Array.isArray(tabs) || tabs.length < 1 || tabs.length > (modern ? MAX_LOCAL_SHELLS : 2) ||
      !Number.isInteger(active) || active < 0 || active >= tabs.length ||
      typeof split !== 'boolean' || (tabs.length === 1 && split)) return false;
  if (!tabs.every(tab => tab && ['plain', 'jsh'].includes(tab.shell) &&
      ['terminal', 'chat'].includes(tab.presentation) &&
      (tab.paneId === undefined || (tab.shell === 'jsh' && validPaneId(tab.paneId))) &&
      (!modern || validLocalGeometry(tab.geometry)))) return false;
  const paneIds = tabs.flatMap(tab => tab.paneId === undefined ? [] : [tab.paneId]);
  if (new Set(paneIds).size !== paneIds.length) return false;
  if (modern && (!Array.isArray(panes) || panes.length !== (split ? 2 : 0) ||
      new Set(panes).size !== panes.length || panes.some(i => !Number.isInteger(i) || i < 0 || i >= tabs.length) ||
      (split && !panes.includes(active)))) return false;
  return true;
}

function decodeLayout(raw) {
  try {
    if (!raw || raw.length > 2048) return null;
    const value = JSON.parse(raw);
    if (!value) return null;
    if (value.version !== 5 && raw.length > (value.version === 4 ? 384 : 256)) return null;
    let {tabs, active, split, panes} = value;
    if (value.version === 1 || value.version === 2) {
      if (Object.keys(value).length !== value.version + 2 || ![1, 2].includes(value.count)) return null;
      tabs = Array.from({length: value.count}, () => ({...plainLocalTab}));
      if (value.version === 1) split = false;
    } else if (value.version === 5) {
      if (Object.keys(value).length !== 5 || !Array.isArray(tabs) || tabs.some(tab => !tab ||
          Object.keys(tab).length !== (tab.paneId === undefined ? 3 : 4))) return null;
    } else if (![3, 4].includes(value.version) || Object.keys(value).length !== 4 ||
        !Array.isArray(tabs) || tabs.some(tab => !tab ||
          Object.keys(tab).length !== (value.version === 4 && tab.paneId !== undefined ? 3 : 2))) return null;
    if (!validLayout(tabs, active, split, panes, value.version === 5)) return null;
    return {version:value.version,layout:value.version === 5 ? {tabs, active, split, panes} : {tabs, active, split}};
  } catch { return null; }
}

function encodeLayout(layout) {
  const {tabs, active, split} = layout;
  const modern = layout.panes !== undefined || tabs?.some(tab => tab.geometry !== undefined) || tabs?.length > 2;
  const panes = layout.panes ?? (split ? [0, 1] : []);
  if (!validLayout(tabs, active, split, panes, modern)) return null;
  const version = modern ? 5 : tabs.some(tab => tab.paneId !== undefined) ? 4 : 3;
  return {version,tabs:tabs.map(({shell,presentation,paneId,geometry}) => ({shell,presentation,paneId,
    ...(modern ? {geometry:{rows:geometry.rows,cols:geometry.cols}} : {})})),active,split,...(modern ? {panes} : {})};
}

function legacyState(storage) {
  try {
    const raw = storage?.getItem(legacyKey);
    if (raw == null) return {available:true,absent:true,token:'absent',decoded:null};
    const decoded = decodeLayout(raw);
    // Compare semantic intent, so an older app's v1 -> v3 rewrite or key order
    // does not accidentally hide a newer multi-tab layout.
    const clean = decoded && encodeLayout(decoded.layout);
    if (clean) delete clean.version;
    return {available:true,absent:false,decoded,
      token:decoded ? (decoded.version === 5 ? 'single-v5:' : 'legacy:') + JSON.stringify(clean) : 'invalid'};
  } catch { return {available:false,absent:false,token:'unavailable',decoded:null}; }
}

function modernState(storage) {
  try {
    const raw = storage?.getItem(modernKey);
    if (!raw || raw.length > 4096) return null;
    const record = JSON.parse(raw);
    if (!record || Object.keys(record).length !== 3 || record.storageVersion !== 1 ||
        typeof record.legacyToken !== 'string' || record.legacyToken.length > 2048) return null;
    if (record.layout === null) return {...record,decoded:null}; // explicit clear tombstone
    const decoded = decodeLayout(JSON.stringify(record.layout));
    return decoded ? {...record,decoded} : null;
  } catch { return null; }
}

export function readLocalShellLayout(environment = globalThis) {
  try {
    const storage = environment.localStorage;
    const legacy = legacyState(storage), modern = modernState(storage);
    if (!modern) return legacy.decoded?.layout ?? null;
    if (legacy.available && modern.legacyToken !== 'unavailable' && legacy.token !== modern.legacyToken) {
      if (legacy.decoded) return legacy.decoded.layout; // a legitimate older writer's changed intent
      if (legacy.absent && modern.legacyToken !== 'absent') return null; // older app explicitly cleared
    }
    return modern.decoded?.layout ?? null;
  } catch { return null; }
}

export function saveLocalShellLayout(layout, environment = globalThis) {
  try {
    const storage = environment.localStorage;
    if (!storage) return false;
    const legacy = legacyState(storage);
    if (!layout) {
      // Publish the clear before touching legacy state. If removal fails, the
      // intact old snapshot must not unexpectedly become the modern fallback.
      storage.setItem(modernKey, JSON.stringify({storageVersion:1,layout:null,legacyToken:legacy.token}));
      storage.removeItem(legacyKey);
      const after = legacyState(storage);
      if (after.available && after.absent) {
        storage.setItem(modernKey, JSON.stringify({storageVersion:1,layout:null,legacyToken:'absent'}));
      }
      return true;
    }
    const clean = encodeLayout(layout);
    if (!clean) return false;
    // New consumers never overwrite a prior release's supported snapshot.
    // An older app may subsequently edit that slot; reads detect that change.
    storage.setItem(modernKey, JSON.stringify({storageVersion:1,layout:clean,legacyToken:legacy.token}));
    return true;
  } catch {
    // Unavailable preference storage never interrupts current shells. An
    // unconfirmed clear is not replaced with a stale legacy-layout fallback.
    return false;
  }
}

// The opt-in L preview has a separate record so older builds can keep
// reading their last supported layout. A later older-build write wins.
const previewKey = 'aether.local-shell-layout.preview-v1';

function layoutSource(storage) {
  const modern = storage.getItem(modernKey);
  const legacy = storage.getItem(legacyKey);
  if ((modern?.length ?? 0) > 4096 || (legacy?.length ?? 0) > 2048) return null;
  return JSON.stringify([modern, legacy, storage.getItem('aether.split-axis') === 'vertical']);
}

function threePaneRecord(layout) {
  if (!layout || !Array.isArray(layout.triple) || layout.triple.length !== 3 ||
      new Set(layout.triple).size !== 3 || !layout.triple.includes(layout.active)) return null;
  const clean = encodeLayout({tabs: layout.tabs, active: layout.active, split: false, panes: []});
  if (!clean || layout.triple.some(i => !Number.isInteger(i) || i < 0 || i >= clean.tabs.length)) return null;
  return {tabs: clean.tabs, active: clean.active, triple: [...layout.triple]};
}

export function readThreePaneLayout(environment = globalThis) {
  try {
    const storage = environment.localStorage, source = layoutSource(storage);
    const raw = storage.getItem(previewKey);
    if (!source || !raw || raw.length > 8192) return null;
    const value = JSON.parse(raw);
    if (!value || value.version !== 1 || value.source !== source) return null;
    return threePaneRecord(value.layout);
  } catch { return null; }
}

export function saveThreePaneLayout(layout, environment = globalThis) {
  try {
    const storage = environment.localStorage;
    if (layout === null) {
      if (storage.getItem(previewKey) != null) storage.removeItem(previewKey);
      return true;
    }
    const source = layoutSource(storage), clean = threePaneRecord(layout);
    if (!source || !clean) return false;
    const raw = JSON.stringify({version: 1, source, layout: clean});
    if (raw.length > 8192) return false;
    storage.setItem(previewKey, raw);
    return true;
  } catch { return false; }
}
