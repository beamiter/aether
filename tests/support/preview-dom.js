export function previewDom({width = 120, height = 24, dpr = 1, drawImage = () => {}, writeText} = {}) {
  const elements = new Map();
  const doc = {
    getElementById: id => el('#' + id),
    querySelector: el,
    createElement: tag => createNode(tag),
    createDocumentFragment: () => createNode('fragment'),
    defaultView: {
      devicePixelRatio: dpr,
      addEventListener() {},
      getComputedStyle: () => ({lineHeight: '21px'}),
      navigator: {clipboard: {writeText}},
    },
  };
  const outsidePanel = new Set(['canvas', '#preview-folder', '#preview-single', '#preview-files',
    '#preview-show', '#preview-clear', '#preview-notice']);

  function createNode(tag, selector) {
    const classes = new Set();
    return {
      tag, selector, value: '', hidden: false, handlers: {}, children: [], dataset: {},
      scrollTop: 0, scrollLeft: 0, clientHeight: height,
      style: {setProperty(name, value) { this[name] = value; }},
      classList: {
        add: name => classes.add(name),
        remove: name => classes.delete(name),
        contains: name => classes.has(name),
        toggle(name, state) { if (state) classes.add(name); else classes.delete(name); },
      },
      get textContent() { return this.children.map(child => typeof child === 'string' ? child : child.textContent).join(''); },
      set textContent(text) { this.children = [text]; },
      addEventListener(name, fn) { this.handlers[name] = fn; },
      replaceChildren(...children) {
        this.children = children.flatMap(child => child.tag === 'fragment' ? child.children : [child]);
      },
      append(...children) { this.children.push(...children); },
      focus() { this.focused = true; doc.activeElement = this; },
      click() { this.clicked = true; },
      select() { this.selected = true; },
      contains(node) { return !!node && !outsidePanel.has(node.selector); },
      cloneNode() { return createNode(tag, selector); },
      replaceWith(replacement) { elements.set(selector, replacement); },
      getBoundingClientRect() { return {left: 0, top: 0, right: width, bottom: height, width, height}; },
      getContext() { return {drawImage}; },
    };
  }

  function el(selector) {
    if (!elements.has(selector)) elements.set(selector, createNode(null, selector));
    return elements.get(selector);
  }
  el('#file-preview').hidden = true;
  return {doc, el};
}
