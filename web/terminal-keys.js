// Pair ordinary insertText with its keydown. Composition commits stay with IME.
const named = /^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;
const keypad = /^Numpad([0-9]|Decimal|Divide|Multiply|Subtract|Add|Enter|Equal|Comma)$/;
const modifiers = new Set(['Shift','Control','Alt','AltGraph','Meta','Super','CapsLock','NumLock','ScrollLock']);
const modifierSides = {
  ShiftLeft:[1,3,'shift'],ShiftRight:[2,3,'shift'],
  ControlLeft:[4,12,'ctrl'],ControlRight:[8,12,'ctrl'],
  AltLeft:[16,48,'alt'],AltRight:[32,48,'alt'],
  MetaLeft:[64,192,'super_key'],MetaRight:[128,192,'super_key'],
  OSLeft:[64,192,'super_key'],OSRight:[128,192,'super_key'],
};

export function modifierKey(name) {
  return modifiers.has(name);
}

function printable(name) {
  const code = name.codePointAt(0);
  return code !== undefined && code >= 32 && !(code >= 127 && code <= 159)
    && name.length === (code > 65535 ? 2 : 1);
}

export function installTerminalKeys({getOwner,send,sendText}) {
  let owner = null, focusGeneration = 0, pending = null;
  let pressedModifiers = 0;
  const held = new Map();

  const clearText = () => { pending = null; };
  const reset = () => {
    focusGeneration = (focusGeneration + 1) >>> 0;
    held.clear();
    pressedModifiers = 0;
    clearText();
    owner = null;
  };
  function current() {
    const currentOwner = getOwner();
    if (currentOwner !== owner) {
      reset();
      owner = currentOwner;
    }
    return currentOwner;
  }
  function snapshot(event, phase) {
    const keyEvent = {
      phase, name: event.key, code: event.code || '',
      shift: !!event.shiftKey, alt: !!event.altKey, ctrl: !!event.ctrlKey,
      super_key: !!event.metaKey,
      caps_lock: event.getModifierState?.('CapsLock') === true,
      num_lock: event.getModifierState?.('NumLock') === true,
      focus_generation: focusGeneration,
      native_time: event.isTrusted === true ? Date.now() : null,
    };
    const side = modifierKey(event.key) && Object.hasOwn(modifierSides,event.code)
      ? modifierSides[event.code] : null;
    if (side) {
      const [bit, family, field] = side;
      if (phase === 'release') pressedModifiers &= ~bit;
      else pressedModifiers |= bit;
      // X11 WebKit can expose the modifier state just before the event.
      keyEvent[field] = (pressedModifiers & family) !== 0;
    }
    return keyEvent;
  }
  function submit(event, attachment) {
    if (!attachment || attachment !== current()) return;
    const code = event.code;
    const slot = event.phase === 'repeat' && held.get(code) || {tracks: null};
    const scheduled = send(event, tracks => {
      slot.tracks = tracks === true;
      if (!slot.tracks && held.get(code) === slot) held.delete(code);
    });
    if (code && code !== 'Unidentified' && scheduled && slot.tracks !== false) held.set(code, slot);
  }

  function down(event, textInput) {
    clearText();
    const attachment = current();
    if (!attachment) return false;
    const key = event.key;
    const physicalPad = keypad.test(event.code || '');
    const hasText = printable(key) && !event.ctrlKey && !event.altKey && !event.metaKey;
    if (modifierKey(key) && (!event.code || event.code === 'Unidentified')) return false;
    if (!physicalPad && !named.test(key) && !modifierKey(key) && !printable(key)) return false;
    const keyEvent = snapshot(event, event.repeat ? 'repeat' : 'press');
    if (textInput && hasText && !physicalPad) {
      pending = {event: keyEvent, attachment};
      return false;
    }
    if (hasText) keyEvent.text = key;
    submit(keyEvent, attachment);
    return true;
  }

  function text(value, attachment) {
    const key = pending;
    clearText();
    if (key && key.attachment === attachment && attachment === current() && key.event.name === value) {
      submit({...key.event,text:value},attachment);
    } else {
      sendText(value,attachment);
    }
  }

  function up(event) {
    if (pending?.event.code === event.code) clearText();
    const attachment = current();
    const slot = held.get(event.code);
    held.delete(event.code);
    const keyEvent = snapshot(event, 'release');
    if (!attachment || !slot || slot.tracks === false || event.defaultPrevented) return;
    // The press receipt can arrive after the DOM keyup. Reserve the release's
    // order in the same queue, and skip its IPC when no wire release is due.
    send(keyEvent, undefined, () =>
      slot.tracks === true && keyEvent.focus_generation === focusGeneration && getOwner() === attachment);
  }

  return {down, up, text, clearText, reset};
}
