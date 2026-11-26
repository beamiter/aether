const key = 'aether.local-shell-layout';
export const plainLocalTab = Object.freeze({shell: 'plain', presentation: 'terminal'});
export const validPaneId = value => typeof value === 'string' && /^[a-f0-9]{32}$/.test(value);

function validLayout(tabs, active, split) {
  if (!Array.isArray(tabs) || ![1, 2].includes(tabs.length) ||
      !Number.isInteger(active) || active < 0 || active >= tabs.length ||
      typeof split !== 'boolean' || (tabs.length === 1 && split)) return false;
  if (!tabs.every(tab => tab && ['plain', 'jsh'].includes(tab.shell) &&
      ['terminal', 'chat'].includes(tab.presentation) &&
      (tab.paneId === undefined || (tab.shell === 'jsh' && validPaneId(tab.paneId))))) return false;
  const paneIds = tabs.flatMap(tab => tab.paneId === undefined ? [] : [tab.paneId]);
  return new Set(paneIds).size === paneIds.length;
}

export function readLocalShellLayout(environment = globalThis) {
  try {
    const raw = environment.localStorage?.getItem(key);
    if (!raw || raw.length > 384) return null;
    const value = JSON.parse(raw);
    if (!value) return null;
    if (value.version !== 4 && raw.length > 256) return null;
    let {tabs, active, split} = value;
    if (value.version === 1 || value.version === 2) {
      if (Object.keys(value).length !== value.version + 2 || ![1, 2].includes(value.count)) return null;
      tabs = Array.from({length: value.count}, () => ({...plainLocalTab}));
      if (value.version === 1) split = false;
    } else if (![3, 4].includes(value.version) || Object.keys(value).length !== 4 ||
        !Array.isArray(tabs) || tabs.some(tab => !tab ||
          Object.keys(tab).length !== (value.version === 4 && tab.paneId !== undefined ? 3 : 2))) {
      return null;
    }
    return validLayout(tabs, active, split) ? {tabs, active, split} : null;
  } catch {
    return null;
  }
}

export function saveLocalShellLayout(layout, environment = globalThis) {
  try {
    const storage = environment.localStorage;
    if (!layout) {
      storage?.removeItem(key);
      return;
    }
    const {tabs, active, split} = layout;
    if (!validLayout(tabs, active, split)) return;
    const version = tabs.some(tab => tab.paneId !== undefined) ? 4 : 3;
    storage?.setItem(key, JSON.stringify({
      version, tabs: tabs.map(({shell, presentation, paneId}) => ({shell, presentation, paneId})), active, split,
    }));
  } catch {
    // An unavailable storage profile must never interrupt the current shells.
  }
}
