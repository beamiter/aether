const backingRows = new WeakMap();
const identities = new WeakMap();
const cellSpans = new WeakMap();

export function markCellSpan(row, base, first, last) {
  // An identity token avoids retaining the base row after it leaves the viewport.
  let identity = identities.get(base);
  if (!identity) identities.set(base,identity=Symbol());
  cellSpans.set(row,Object.freeze({base:identity,first,last}));
}

export function cellSpanForBase(row, base) {
  const span = cellSpans.get(row);
  return span && span.base === identities.get(base) ? span : undefined;
}

export class DenseChannelRow {
  #cells;
  #view;

  constructor(cells, {wrapped, textBytes, links, imageSlices}) {
    // Admission transfers ownership of the array and has already frozen its cells.
    this.#cells = cells;
    this.length = cells.length;
    this.wrapped = wrapped;
    this.textBytes = textBytes;
    this.links = links;
    this.imageSlices = imageSlices;
    Object.freeze(this);
  }

  cell(column) {
    return this.#cells[column];
  }

  slice() {
    return this.#cells.slice();
  }

  arrayView() {
    const {wrapped, textBytes, links, imageSlices} = this;
    return this.#view ??= Object.freeze(Object.assign(this.slice(), {wrapped,textBytes,links,imageSlices}));
  }
}

export function rowCell(row, column) {
  return Array.isArray(row) ? row[column] : row.cell(column);
}

export function admittedRows(rows) {
  return backingRows.get(rows) ?? rows;
}

export function channelRowView(rows) {
  if (!rows.some(row => row instanceof DenseChannelRow)) return Object.freeze(rows);

  // Keep validation and painting on the immutable row objects. Callers that
  // use the array interface get a frozen copy of each row when they read it.
  const view = new Array(rows.length);
  for (const [index,row] of rows.entries()) {
    if (row instanceof DenseChannelRow) {
      Object.defineProperty(view,index,{enumerable:true,get:()=>row.arrayView()});
    } else {
      view[index] = row;
    }
  }
  Object.freeze(view);
  backingRows.set(view,Object.freeze(rows));
  return view;
}
