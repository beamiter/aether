const ansi = [
  '#000000', '#cd3131', '#0dbc79', '#e5e510', '#2472c8', '#bc3fbc', '#11a8cd', '#e5e5e5',
  '#666666', '#f14c4c', '#23d18b', '#f5f543', '#3b8eea', '#d670d6', '#29b8db', '#ffffff',
];
const rgb = channels => '#' + channels.map(value => value.toString(16).padStart(2, '0')).join('');
const encoder = new TextEncoder();

function validColor(packed) {
  if (!Number.isInteger(packed) || packed < 0 || packed > 0x03_00_00_0f) return false;
  const kind = packed >>> 24;
  return packed === 0 || kind === 2 || (kind === 1 && packed <= 0x01_00_00_ff)
    || (kind === 3 && packed <= 0x03_00_00_0f);
}

function cssColor(packed, fallback) {
  if (!packed) return fallback;
  if ((packed >>> 24) === 2) return rgb([(packed >>> 16) & 255, (packed >>> 8) & 255, packed & 255]);
  const code = packed & 255;
  if (code < 16) return ansi[code];
  if (code >= 232) return rgb(Array(3).fill(8 + (code - 232) * 10));
  const n = code - 16;
  const levels = [0, 95, 135, 175, 215, 255];
  return rgb([levels[Math.floor(n / 36)], levels[Math.floor(n / 6) % 6], levels[n % 6]]);
}

/** Decode shared UTF-8 offsets into the UTF-16 coordinates the DOM selects. */
export function decodeOutputAnnotations(text, annotations) {
  if (!annotations || !Array.isArray(annotations.spans) || annotations.spans.length > 512
      || !Array.isArray(annotations.links) || annotations.links.length > 128
      || annotations.spans.length + annotations.links.length === 0) {
    throw new Error('Invalid command output annotations.');
  }
  const offsets = [0];
  let bytes = 0, units = 0;
  for (const character of text) {
    const code = character.codePointAt(0);
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    units += character.length;
    offsets[bytes] = units;
  }
  const decode = (records, valid) => {
    let previous = 0;
    return records.map(record => {
      const {start, end} = record?.bytes ?? {};
      if (!Number.isInteger(start) || !Number.isInteger(end) || start < previous || end <= start
          || offsets[start] === undefined || offsets[end] === undefined || !valid(record)) {
        throw new Error('Invalid command output annotation range.');
      }
      previous = end;
      return {...record, start: offsets[start], end: offsets[end]};
    });
  };
  let linkBytes = 0;
  const spans = decode(annotations.spans, record => Number.isInteger(record.flags)
    && record.flags >= 0 && record.flags <= 383 && Array.isArray(record.colors)
    && record.colors.length === 3 && record.colors.every(validColor));
  const links = decode(annotations.links, record => {
    if (typeof record.target !== 'string' || !record.target || /[\u0000-\u001f\u007f-\u009f\ufffd]/.test(record.target)) return false;
    const length = encoder.encode(record.target).length;
    linkBytes += length;
    return length <= 2048 && linkBytes <= 32768;
  });
  return {spans, links};
}

function paintSpan(node, annotation, link) {
  const style = node.style;
  const flags = annotation?.flags ?? 0;
  const colors = annotation?.colors ?? [0, 0, 0];
  const inverted = (flags & 4) !== 0;
  let foreground = inverted ? cssColor(colors[1], '#111820') : cssColor(colors[0], '#d9e5ef');
  if (flags & 16) {
    const channels = foreground.slice(1).match(/../g);
    foreground = rgb(channels.map(value => Math.floor(Number.parseInt(value, 16) * 2 / 3)));
  }
  if (annotation) {
    style.color = foreground;
    if (inverted || colors[1]) style.backgroundColor = inverted
      ? cssColor(colors[0], '#d9e5ef') : cssColor(colors[1], '#111820');
    if (flags & 1) style.fontWeight = 'bold';
    if (flags & 2) style.fontStyle = 'italic';
  }
  const underline = flags >>> 6;
  if (underline || link || (flags & 8)) {
    style.textDecorationLine = [underline || link ? 'underline' : '', flags & 8 ? 'line-through' : ''].filter(Boolean).join(' ');
    style.textDecorationStyle = ['solid', 'solid', 'double', 'wavy', 'dotted', 'dashed'][underline];
    if (colors[2]) style.textDecorationColor = cssColor(colors[2], foreground);
  }
  if (link) node.title = link.target;
}

export function annotatedOutput(document, text, annotations) {
  const {spans, links} = decodeOutputAnnotations(text, annotations);
  const starts = new Set([0, text.length]);
  for (const record of [...spans, ...links]) {
    starts.add(record.start);
    starts.add(record.end);
  }
  const cuts = [...starts].sort((a, b) => a - b);
  const view = document.createElement('pre');
  view.className = 'command-block-annotated-output';
  view.tabIndex = 0;
  let i = 0, j = 0;
  for (let cut = 0; cut + 1 < cuts.length; cut++) {
    const start = cuts[cut];
    const segment = text.slice(start, cuts[cut + 1]);
    while (spans[i]?.end <= start) i++;
    while (links[j]?.end <= start) j++;
    const annotation = spans[i]?.start <= start ? spans[i] : null;
    const link = links[j]?.start <= start ? links[j] : null;
    if (!annotation && !link) {
      view.append(document.createTextNode(segment));
    } else {
      const node = document.createElement('span');
      node.textContent = segment;
      paintSpan(node, annotation, link);
      view.append(node);
    }
  }
  return view;
}
