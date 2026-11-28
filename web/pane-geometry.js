// Split names describe the arrangement, not the divider. The Rust core's
// Vertical divider maps to columns; legacy Aether's vertical maps to rows.
const half = 500;
const split = (arrangement, first, second, share = half) => ({arrangement, first, second, share});
const leaf = epoch => ({epoch});

export function retainedPaneTree(ordered, legacyAxis, three = false) {
  if (!Array.isArray(ordered) || ordered.length !== (three ? 3 : 2) ||
      new Set(ordered).size !== ordered.length ||
      ordered.some(epoch => !Number.isSafeInteger(epoch) || epoch < 1)) return null;
  const [a,b,c] = ordered.map(leaf);
  return three ? split('columns', a, split('rows', b, c)) :
    split(legacyAxis === 'vertical' ? 'rows' : 'columns', a, b);
}

// Geometry is normalized to the viewport. Shares are integer thousandths
// of the parent rectangle and are bounded at 10% from either edge.
export function paneRectangles(tree) {
  const rectangles = new Map();
  let remaining = 15; // At most eight visible leaves.
  function visit(node, x, y, width, height) {
    if (--remaining < 0 || !node || typeof node !== 'object') return false;
    if (Object.hasOwn(node,'epoch')) {
      if (!Number.isSafeInteger(node.epoch) || node.epoch < 1 || rectangles.has(node.epoch)) return false;
      rectangles.set(node.epoch, {x,y,width,height});
      return rectangles.size <= 8;
    }
    const {arrangement, first, second, share} = node;
    if (!Number.isInteger(share) || share < 100 || share > 900) return false;
    if (arrangement === 'columns') {
      const cut = width * share / 1000;
      return visit(first,x,y,cut,height) && visit(second,x+cut,y,width-cut,height);
    }
    if (arrangement === 'rows') {
      const cut = height * share / 1000;
      return visit(first,x,y,width,cut) && visit(second,x,y+cut,width,height-cut);
    }
    return false;
  }
  return visit(tree,0,0,1,1) ? rectangles : null;
}

export function directionalPaneNeighbor(tree, active, direction) {
  const horizontal = direction === 'left' || direction === 'right';
  if (!horizontal && direction !== 'up' && direction !== 'down') return null;
  const positive = direction === 'right' || direction === 'down';
  const rectangles = paneRectangles(tree), anchor = rectangles?.get(active);
  if (!anchor) return null;
  const axis = horizontal ? 'x' : 'y', extent = horizontal ? 'width' : 'height';
  const cross = horizontal ? 'y' : 'x', span = horizontal ? 'height' : 'width';
  const center = rect => rect[cross] + rect[span] / 2;
  const end = rect => rect[axis] + rect[extent];
  let selected = null;
  for (const [epoch,rect] of rectangles) {
    if (epoch === active) continue;
    const gap = positive ? rect[axis] - end(anchor) : anchor[axis] - end(rect);
    const overlap = Math.min(anchor[cross]+anchor[span],rect[cross]+rect[span]) -
      Math.max(anchor[cross],rect[cross]);
    const tolerance = 1e-9;
    if (gap < -tolerance || overlap <= tolerance) continue;
    const offset = Math.abs(center(rect)-center(anchor));
    // Like the shared layout core: nearest edge, greatest shared edge,
    // then nearest perpendicular center. The fixed L retains its top tie.
    if (!selected || gap < selected.gap-tolerance ||
        (Math.abs(gap-selected.gap)<=tolerance && (overlap > selected.overlap+tolerance ||
         (Math.abs(overlap-selected.overlap)<=tolerance && offset < selected.offset-tolerance)))) {
      selected = {epoch,gap,overlap,offset};
    }
  }
  // Equal candidates retain first child order (top-right wins from the
  // L preview's spanning left pane); corner contact isn't a neighbor.
  return selected?.epoch ?? null;
}
