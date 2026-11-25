function selectStyledRange(document, target, start, end) {
  const nodes = document.createTreeWalker(target, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  let offset = 0, hasStart = false;
  for (let node = nodes.nextNode(); node; node = nodes.nextNode()) {
    const next = offset + node.length;
    if (!hasStart && next >= start) {
      range.setStart(node, start - offset);
      hasStart = true;
    }
    if (next >= end) {
      range.setEnd(node, end - offset);
      const selection = document.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      break;
    }
    offset = next;
  }
}

export function installBlockSearch(document, {getOwner, scan, cardFor}) {
  const form = document.querySelector('#command-block-search');
  const input = document.querySelector('#command-block-query');
  const results = document.querySelector('#command-block-matches');
  const status = document.querySelector('#command-block-search-status');
  let request = null;

  function clear() {
    request = null;
    results.replaceChildren();
    status.textContent = '';
  }

  function reset() {
    input.value = '';
    clear();
  }

  function select(current, hit) {
    if (request !== current || getOwner() !== current.owner) return;
    const card = cardFor(hit.record);
    const output = hit.surface === 'output';
    const textarea = output ? card?.querySelector('textarea') : null;
    const target = textarea ?? card?.querySelector(output ? '.command-block-annotated-output' : 'summary code');
    const text = textarea?.value ?? target?.textContent;
    if (text?.slice(hit.start, hit.end) !== current.query) {
      status.textContent = 'Refresh blocks to open this result, then search again.';
      return;
    }
    card.open = true;
    if (output) {
      target.focus();
      if (textarea) target.setSelectionRange(hit.start, hit.end);
      else selectStyledRange(document, target, hit.start, hit.end);
    } else {
      target.parentElement.focus();
      const range = document.createRange();
      range.setStart(target.firstChild, hit.start);
      range.setEnd(target.firstChild, hit.end);
      const selection = document.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    }
    target.scrollIntoView({block: 'nearest'});
  }

  form.addEventListener('submit', async event => {
    event.preventDefault();
    const owner = getOwner();
    if (form.hidden || !owner || !input.value) return;
    clear();
    const current = {owner, query: input.value};
    request = current;
    status.textContent = 'Searching completed commands…';
    try {
      const {hits, scanIncomplete} = await scan(owner, current.query);
      if (request !== current || getOwner() !== owner) return;
      for (const hit of hits) {
        const item = document.createElement('li');
        const button = document.createElement('button');
        button.type = 'button';
        const surface = hit.surface === 'output' ? 'Output' : 'Command';
        button.textContent = `${surface} · #${hit.record.sequence} · ${hit.label}`;
        button.addEventListener('click', () => select(current, hit));
        item.append(button);
        results.append(item);
      }
      status.textContent = scanIncomplete
        ? 'The search reached its scan limit.'
        : hits.length ? '' : 'No matches in retained commands or captured output.';
    } catch (error) {
      if (request === current && getOwner() === owner) status.textContent = String(error?.message ?? error);
    }
  });
  input.addEventListener('input', clear);
  document.querySelector('#command-block-query-clear').addEventListener('click', reset);
  return {
    reset,
    setAvailable(available) {
      form.hidden = !available;
      if (!available) reset();
    },
  };
}
