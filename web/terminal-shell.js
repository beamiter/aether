export function installTerminalShell(document, {actions, isComposing, keyboardScope, paletteScope = keyboardScope}) {
  const dialog = document.querySelector('#command-palette');
  const query = document.querySelector('#command-palette-query');
  const list = document.querySelector('#command-palette-results');
  const empty = document.querySelector('#command-palette-empty');
  let paletteEnter = false;
  const bindings = new Map();
  for (const action of actions) {
    for (const key of action.keys ?? []) bindings.set(key, action);
  }

  function run(action) {
    if (action.enabled && !action.enabled()) return;
    if (dialog.open) dialog.close();
    action.run();
  }

  function render() {
    const words = query.value.toLowerCase().trim().split(/\s+/);
    const matches = actions.filter(action => {
      const label = [action.label, action.hint].join(' ').toLowerCase();
      return words.every(word => label.includes(word));
    });
    list.replaceChildren(...matches.map(action => {
      const item = document.createElement('li');
      const button = document.createElement('button');
      const label = document.createElement('span');
      label.textContent = action.label;
      button.type = 'button';
      button.append(label);
      if (action.enabled && !action.enabled()) {
        button.setAttribute('aria-disabled', 'true');
        const reason = document.createElement('small');
        reason.textContent = action.unavailableReason?.() ?? 'Not available in this view.';
        label.append(reason);
      }
      if (action.hint) {
        const hint = document.createElement('kbd');
        hint.textContent = action.hint;
        button.append(hint);
      }
      button.addEventListener('click', () => {
        if (button.getAttribute('aria-disabled') !== 'true') run(action);
      });
      item.append(button);
      return item;
    }));
    empty.hidden = matches.length > 0;
    list.setAttribute('data-query-focus', '');
  }

  function toggle() {
    if (dialog.open) {
      dialog.close();
      return;
    }
    query.value = '';
    render();
    dialog.showModal();
    query.focus();
  }

  document.querySelector('#command-palette-open').addEventListener('click', toggle);
  query.addEventListener('input', render);
  query.addEventListener('focus', () => list.setAttribute('data-query-focus', ''));
  query.addEventListener('blur', () => list.removeAttribute('data-query-focus'));
  dialog.addEventListener('keydown', event => {
    if (event.isComposing || event.keyCode === 229 || event.key === 'Process' || isComposing()) return;
    const buttons = [...list.querySelectorAll('button')];
    if (!buttons.length) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      if (!event.repeat) {
        paletteEnter = true;
        const target = buttons.find(button => button === event.target) ??
          buttons.find(button => button.getAttribute('aria-disabled') !== 'true');
        target?.click();
      }
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const index = buttons.indexOf(document.activeElement);
      if (event.key === 'ArrowUp' && index <= 0) query.focus();
      else buttons[Math.min(index + (event.key === 'ArrowDown' ? 1 : -1), buttons.length - 1)].focus();
    }
  });

  document.addEventListener('keydown', event => {
    if (paletteEnter && event.key === 'Enter' && event.repeat) {
      event.preventDefault();
      return;
    }
    if (event.defaultPrevented || !event.ctrlKey || event.altKey || event.metaKey ||
        event.getModifierState?.('AltGraph') || event.isComposing || event.keyCode === 229 || isComposing()) return;
    const name = event.code === 'Tab' || event.key === 'ISOLeftTab' ? 'tab' : event.key.toLowerCase();
    const key = `${event.shiftKey ? 'Shift+' : ''}${name}`;
    if (key === 'Shift+p' && (dialog.open || paletteScope(event.target))) {
      event.preventDefault();
      if (!event.repeat) toggle();
    } else if (!dialog.open && keyboardScope(event.target)) {
      const action = bindings.get(key);
      if (!action) return;
      // Host shortcuts never become input to the attached PTY.
      event.preventDefault();
      if (!event.repeat) run(action);
    }
  }, true);
  document.addEventListener('keyup', event => {
    if (event.key === 'Enter') paletteEnter = false;
  }, true);
}
