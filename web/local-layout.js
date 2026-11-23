const key = 'aether.local-shell-layout';
export const plainLocalTab = Object.freeze({shell: 'plain', presentation: 'terminal'});

function validLayout(tabs, active, split) {
  return Array.isArray(tabs) && [1, 2].includes(tabs.length) &&
    Number.isInteger(active) && active >= 0 && active < tabs.length &&
    typeof split === 'boolean' && (tabs.length === 2 || !split) &&
    tabs.every(tab => tab && ['plain', 'jsh'].includes(tab.shell) &&
      ['terminal', 'chat'].includes(tab.presentation));
}

export function readLocalShellLayout(environment = globalThis) {
  try {
    const raw = environment.localStorage?.getItem(key);
    if (!raw || raw.length > 256) return null;
    const value = JSON.parse(raw);
    if (!value) return null;
    let {tabs, active, split} = value;
    if (value.version === 1 || value.version === 2) {
      if (Object.keys(value).length !== value.version + 2 || ![1, 2].includes(value.count)) return null;
      tabs = Array.from({length: value.count}, () => ({...plainLocalTab}));
      if (value.version === 1) split = false;
    } else if (value.version !== 3 || Object.keys(value).length !== 4 ||
        !Array.isArray(tabs) || tabs.some(tab => !tab || Object.keys(tab).length !== 2)) {
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
    storage?.setItem(key, JSON.stringify({
      version: 3, tabs: tabs.map(({shell, presentation}) => ({shell, presentation})), active, split,
    }));
  } catch {
    // An unavailable storage profile must never interrupt the current shells.
  }
}
