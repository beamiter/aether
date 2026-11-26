const MATCH_LIMIT = 4096;
const MAX_FIND_LENGTH = 4096;

export function sourceDocument(raw) {
  const text = raw.replaceAll('\r\n', '\n').replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, character => {
    if (character === '\n' || character === '\t') return character;
    return '\\u{' + character.codePointAt(0).toString(16) + '}';
  });
  const starts = [0];
  for (let offset = text.indexOf('\n'); offset !== -1; offset = text.indexOf('\n', offset + 1)) {
    starts.push(offset + 1);
  }
  return {
    text,
    lineCount: starts.length,
    line(line) {
      const start = starts[line - 1];
      if (start === undefined) return null;
      const end = line < starts.length ? starts[line] - 1 : text.length;
      return text.slice(start, end);
    },
    find(query, matchCase) {
      const hits = [];
      if (!query || query.length > MAX_FIND_LENGTH || query.includes('\n')) {
        return {hits, truncated: false};
      }
      const literal = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      const pattern = new RegExp(literal, matchCase ? 'gu' : 'giu');
      let line = 1;
      for (const match of text.matchAll(pattern)) {
        if (hits.length === MATCH_LIMIT) return {hits, truncated: true};
        while (line < starts.length && starts[line] <= match.index) line++;
        hits.push({line, start: match.index, end: match.index + match[0].length});
      }
      return {hits, truncated: false};
    }
  };
}

export function nextSourceMatch(hits, active, line, backwards) {
  if (!hits.length) return -1;
  if (active !== -1) return (active + (backwards ? -1 : 1) + hits.length) % hits.length;
  if (line === null) return backwards ? hits.length - 1 : 0;
  if (backwards) {
    const index = hits.findLastIndex(hit => hit.line <= line);
    return index === -1 ? hits.length - 1 : index;
  }
  const index = hits.findIndex(hit => hit.line >= line);
  return index === -1 ? 0 : index;
}

export function sourceMatchStatus(hits, active, truncated) {
  if (!hits.length) return 'No matches';
  const count = hits.length + (truncated ? '+' : '');
  if (active !== -1) return `Match ${active + 1} of ${count}`;
  return hits.length === 1 ? '1 match' : `${count} matches`;
}
