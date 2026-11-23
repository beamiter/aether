import {channelCounter} from './frames-v4.js';

const surfaces = new Set(['prompt', 'running', 'fullscreen', 'passthrough']);
const labels = {prompt: 'Shell prompt', running: 'Command running', passthrough: 'Live terminal'};

export function validateChatStatus(chat) {
  if (chat === undefined) return;
  if (!chat || !surfaces.has(chat.surface) || (chat.surface === 'prompt') !== (chat.prompt !== null)) {
    throw new Error('invalid local terminal Chat status');
  }
  if (chat.prompt !== null) channelCounter(chat.prompt);
  if (chat.newest_completed !== null) channelCounter(chat.newest_completed);
}

export function installTerminalChat(document, {getOwner, blocks, canSend, sendEnter, focusTerminal, revealCursor}) {
  const toggle = document.querySelector('#terminal-chat-toggle');
  const pane = document.querySelector('#active-pane');
  const send = document.querySelector('#terminal-chat-send');
  const dock = document.querySelector('#terminal-chat-dock');
  const label = document.querySelector('#terminal-chat-surface');
  const legacyInput = document.querySelector('#command');
  const transcript = document.querySelector('#terminal-chat-transcript');
  const blockHome = document.querySelector('#command-block-home');
  const blockControls = document.querySelector('#command-blocks');
  const selectedSessions = new Set();
  let owner = null, selected = false, lastStatus = null, sent = null, sending = null;
  const currentStatus = () => owner === lastStatus?.owner ? lastStatus.info : null;

  function selection(session) {
    try {
      return selectedSessions.has(session) || globalThis.sessionStorage?.getItem(`aether.terminal-mode.${session}`) === 'chat';
    } catch {
      return selectedSessions.has(session);
    }
  }

  function setSelection(session, enabled) {
    if (enabled) selectedSessions.add(session);
    else selectedSessions.delete(session);
    try {
      const key = `aether.terminal-mode.${session}`;
      if (enabled) globalThis.sessionStorage?.setItem(key, 'chat');
      else globalThis.sessionStorage?.removeItem(key);
    } catch {
      // Keep the preference usable for the lifetime of this view.
    }
  }

  function render() {
    const shellStatus = currentStatus();
    const chat = selected ? shellStatus?.chat : null;
    const surface = selected ? chat?.surface ?? 'passthrough' : '';
    const showTranscript = surface === 'prompt' || surface === 'running' ||
      (surface === 'passthrough' && shellStatus?.exited && chat?.newest_completed != null);
    const previousSurface = pane.dataset.chatSurface;
    if (surface !== previousSurface) pane.dataset.chatSurface = surface;
    if (selected !== Boolean(previousSurface)) (selected ? transcript : blockHome).append(blockControls);
    toggle.disabled = !owner;
    toggle.setAttribute('aria-pressed', String(selected));
    legacyInput.hidden = selected;
    dock.hidden = !surface || surface === 'fullscreen';
    const nextLabel = labels[surface] ?? '';
    if (label.textContent !== nextLabel) label.textContent = nextLabel;
    send.hidden = surface !== 'prompt';
    send.disabled = send.hidden || !!sending || sent === chat?.prompt || !canSend();
    blocks.setChatPresentation(selected, showTranscript, chat?.newest_completed ?? null);
    if (surface === 'prompt' && surface !== previousSurface) revealCursor();
  }

  function updateOwner() {
    const next = getOwner();
    if (next !== owner) {
      owner = next;
      selected = !!owner && selection(owner.session);
      sent = null;
      sending = null;
    }
    render();
  }

  toggle.addEventListener('click', () => {
    if (!owner || getOwner() !== owner) return;
    selected = !selected;
    setSelection(owner.session, selected);
    render();
  });

  send.addEventListener('click', () => {
    const prompt = currentStatus()?.chat?.prompt;
    if (!selected || !prompt || sending || sent === prompt || getOwner() !== owner || !canSend()) return;
    const attachment = owner;
    const action = {};
    if (document.activeElement === send) focusTerminal();
    sending = action;
    render();
    const queued = sendEnter(prompt, accepted => {
      if (accepted && owner === attachment) sent = prompt;
    });
    void Promise.resolve(queued).finally(() => {
      if (sending === action) {
        sending = null;
        render();
      }
    });
  });

  return {
    updateOwner,
    currentPrompt: () => currentStatus()?.chat?.prompt ?? null,
    showStatus(info, attachment) {
      lastStatus = {owner: attachment, info};
      if (owner && getOwner() === owner) render();
    },
    followCursor() {
      if (selected && currentStatus()?.chat?.surface === 'prompt') revealCursor();
    },
    forget: session => setSelection(session, false),
  };
}
