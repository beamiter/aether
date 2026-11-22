const key = 'aether.local-shell-layout';

export function readLocalShellLayout(environment = globalThis) {
  try {
    const raw = environment.localStorage?.getItem(key);
    if (!raw || raw.length > 96) return null;
    const value = JSON.parse(raw);
    if (!value ||
        ![1, 2].includes(value.count) || !Number.isInteger(value.active) ||
        value.active < 0 || value.active >= value.count) return null;
    if (value.version === 1 && Object.keys(value).length === 3) {
      return {count: value.count, active: value.active, split: false};
    }
    if (value.version !== 2 || Object.keys(value).length !== 4 || typeof value.split !== 'boolean' ||
        (value.count === 1 && value.split)) return null;
    return {count: value.count, active: value.active, split: value.split};
  } catch {
    return null;
  }
}

export function saveLocalShellLayout(layout, environment = globalThis) {
  try {
    const storage = environment.localStorage;
    if (layout) storage?.setItem(key, JSON.stringify({version: 2, count: layout.count, active: layout.active,
      split: layout.count === 2 && layout.split === true}));
    else storage?.removeItem(key);
  } catch {
    // An unavailable storage profile must never interrupt the current shells.
  }
}
