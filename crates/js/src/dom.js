// The DOM: Node and its subclasses, the document, attributes, collections,
// classList, dataset, inline style, and the UI event classes.
// https://dom.spec.whatwg.org/ https://html.spec.whatwg.org/
//
// The tree lives in Rust (native.dom). Node objects are host wrappers made
// by Rust, one per node for as long as script can reach it; the classes
// here only supply their prototypes. Per-node state that lives in JS (form
// control values, cached collections, style objects) is kept in a private
// field stamped onto the wrapper, so it is traced and collected with it.
//
// Nothing is laid out. Geometry comes from internals.elementBox, which
// makes every rendered element a visible, in-viewport box; the page
// lifecycle and scrolling code can replace it.
(function (native, internals) {
  'use strict';

  const N = native.dom;
  const env = native.env;
  const domException = internals.domException;
  const eventClass = internals.eventClass;
  const defineProperty = Object.defineProperty;
  const getPrototypeOf = Object.getPrototypeOf;
  const setPrototypeOf = Object.setPrototypeOf;
  const ArrayIsArray = Array.isArray;
  const ReflectApply = Reflect.apply;
  const queueTask = internals.queueTask;
  const unproxy = N.unproxy;

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  const KEY = Symbol('dom');
  // Set once createHTMLDocument has made a second document.
  let otherDocuments = false;
  function illegal(key) {
    if (key !== KEY) throw new TypeError('Illegal constructor');
  }

  function requireArgs(args, n, what) {
    if (args.length < n) {
      throw new TypeError('Failed to execute ' + what + ': ' + n + ' argument' + (n === 1 ? '' : 's') +
        ' required, but only ' + args.length + ' present.');
    }
  }

  // Strings cross into Rust as valid UTF-8.
  const str = internals.usv;
  const nullableStr = (v) => (v === null || v === undefined ? null : str(v));

  // Per-object state in a private field, stamped on with the constructor
  // return-override trick so it works on objects made by Rust.
  class Stamp {
    constructor(o) {
      return o;
    }
  }
  class State extends Stamp {
    #s;
    constructor(o) {
      super(o);
      this.#s = { __proto__: null };
    }
    static of(o) {
      if (!(#s in o)) new State(o);
      return o.#s;
    }
  }
  const state = State.of;

  // ---------------------------------------------------------------------
  // Collections. Live ones are Rust objects behind a Proxy that answers
  // indexed (and, for HTMLCollection, named) properties; methods reach the
  // Rust object through native.dom.unproxy. Static NodeLists are arrays
  // with NodeList.prototype.

  function arrayIndex(key) {
    if (typeof key !== 'string') return -1;
    const n = +key;
    return n >>> 0 === n && n !== 4294967295 && String(n) === key ? n : -1;
  }

  function listLength(list) {
    return ArrayIsArray(list) ? list.length : N.collLength(list);
  }

  function listItem(list, i) {
    if (ArrayIsArray(list)) {
      const v = list[i];
      return v === undefined ? null : v;
    }
    return N.collItem(list, i);
  }

  function makeHandler(named) {
    return {
      get(target, key, receiver) {
        const i = arrayIndex(key);
        if (i >= 0) {
          const v = N.collItem(target, i);
          return v === null ? undefined : v;
        }
        if (named && typeof key === 'string' && !(key in target)) {
          const v = N.collNamed(target, key);
          return v === null ? undefined : v;
        }
        return Reflect.get(target, key, receiver);
      },
      has(target, key) {
        const i = arrayIndex(key);
        if (i >= 0) return i < N.collLength(target);
        if (named && typeof key === 'string' && !(key in target)) return N.collNamed(target, key) !== null;
        return Reflect.has(target, key);
      },
      set(target, key, value, receiver) {
        if (arrayIndex(key) >= 0) return true;
        return Reflect.set(target, key, value, receiver);
      },
      ownKeys(target) {
        const keys = [];
        const n = N.collLength(target);
        for (let i = 0; i < n; i++) keys.push(String(i));
        if (named) for (const name of N.collNames(target)) if (!keys.includes(name)) keys.push(name);
        return keys.concat(Reflect.ownKeys(target));
      },
      getOwnPropertyDescriptor(target, key) {
        const i = arrayIndex(key);
        if (i >= 0) {
          const v = N.collItem(target, i);
          return v === null ? undefined : { value: v, writable: false, enumerable: true, configurable: true };
        }
        if (named && typeof key === 'string' && !Reflect.has(target, key)) {
          const v = N.collNamed(target, key);
          if (v !== null) return { value: v, writable: false, enumerable: false, configurable: true };
        }
        return Reflect.getOwnPropertyDescriptor(target, key);
      },
    };
  }
  const nodeListHandler = makeHandler(false);
  const htmlCollectionHandler = makeHandler(true);

  const KIND = { childNodes: 0, children: 1, tag: 2, tagNS: 3, className: 4, name: 5, selector: 6 };

  function nodeList(root, kind, a, b) {
    return new Proxy(N.collection(root, kind, a, b, NodeList.prototype), nodeListHandler);
  }

  function htmlCollection(root, kind, a, b, proto = HTMLCollection.prototype) {
    return new Proxy(N.collection(root, kind, a, b, proto), htmlCollectionHandler);
  }

  // A collection cached on the node, so repeated reads return the same one.
  function cached(node, key, make) {
    const s = state(node);
    let v = s[key];
    if (v === undefined) {
      v = make();
      s[key] = v;
    }
    return v;
  }

  function staticNodeList(array) {
    return setPrototypeOf(array, NodeList.prototype);
  }

  function* iterate(list) {
    for (let i = 0; i < listLength(list); i++) yield listItem(list, i);
  }

  class NodeList {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return N.collLength(this);
    }
    item(index) {
      return listItem(this, index >>> 0);
    }
    forEach(callback, thisArg) {
      for (let i = 0; i < listLength(this); i++) ReflectApply(callback, thisArg, [listItem(this, i), i, this]);
    }
    *entries() {
      let i = 0;
      for (const v of iterate(this)) yield [i++, v];
    }
    *keys() {
      for (let i = 0; i < listLength(this); i++) yield i;
    }
    values() {
      return iterate(this);
    }
    [Symbol.iterator]() {
      return iterate(this);
    }
  }
  tag(NodeList, 'NodeList');

  class HTMLCollection {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return N.collLength(this);
    }
    item(index) {
      return N.collItem(this, index >>> 0);
    }
    namedItem(name) {
      return N.collNamed(this, str(name));
    }
    [Symbol.iterator]() {
      return iterate(this);
    }
  }
  tag(HTMLCollection, 'HTMLCollection');

  class HTMLFormControlsCollection extends HTMLCollection {}
  tag(HTMLFormControlsCollection, 'HTMLFormControlsCollection');

  class HTMLOptionsCollection extends HTMLCollection {}
  tag(HTMLOptionsCollection, 'HTMLOptionsCollection');

  // ---------------------------------------------------------------------
  // Node.

  const NODE_TYPES = [
    ['ELEMENT_NODE', 1], ['ATTRIBUTE_NODE', 2], ['TEXT_NODE', 3], ['CDATA_SECTION_NODE', 4],
    ['ENTITY_REFERENCE_NODE', 5], ['ENTITY_NODE', 6], ['PROCESSING_INSTRUCTION_NODE', 7],
    ['COMMENT_NODE', 8], ['DOCUMENT_NODE', 9], ['DOCUMENT_TYPE_NODE', 10],
    ['DOCUMENT_FRAGMENT_NODE', 11], ['NOTATION_NODE', 12],
    ['DOCUMENT_POSITION_DISCONNECTED', 0x01], ['DOCUMENT_POSITION_PRECEDING', 0x02],
    ['DOCUMENT_POSITION_FOLLOWING', 0x04], ['DOCUMENT_POSITION_CONTAINS', 0x08],
    ['DOCUMENT_POSITION_CONTAINED_BY', 0x10], ['DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC', 0x20],
  ];

  let documentNode = null;

  // Script elements that become connected. Inline classic scripts run
  // right away, as they would in a browser. The others (external scripts,
  // modules) go to the page loader through internals.scriptConnected, with
  // the script's "non-blocking" flag; without a loader they are left alone.
  // Links that load something come here too, for internals.linkConnected.
  function connected(scripts) {
    for (const script of scripts) {
      try {
        if (N.elementName(script)[0] === 'link') {
          const hook = internals.linkConnected;
          if (typeof hook === 'function') hook(script);
        } else if (!N.hasAttr(script, 'src') && scriptType(script) === 'classic') {
          runInlineScript(script);
        } else {
          const hook = internals.scriptConnected;
          if (typeof hook === 'function') hook(script, state(script).forceAsync === true);
        }
      } catch (error) {
        native.reportError(error);
      }
    }
  }

  const CLASSIC_TYPES = [
    '', 'text/javascript', 'application/javascript', 'text/ecmascript', 'application/ecmascript',
    'application/x-javascript', 'application/x-ecmascript', 'text/x-javascript', 'text/jscript',
    'text/livescript', 'text/x-ecmascript', 'text/javascript1.0', 'text/javascript1.1',
    'text/javascript1.2', 'text/javascript1.3', 'text/javascript1.4', 'text/javascript1.5',
  ];

  // 'classic', 'module', 'importmap' or null (a data block, not run).
  function scriptType(script) {
    const type = N.getAttr(script, 'type');
    const language = N.getAttr(script, 'language');
    let t;
    if (type === null) t = language === null || language === '' ? '' : 'text/' + language;
    else t = type;
    t = t.trim().toLowerCase();
    if (CLASSIC_TYPES.includes(t)) return 'classic';
    if (t === 'module' || t === 'importmap') return t;
    return null;
  }

  const indirectEval = eval;

  function runInlineScript(script) {
    if (N.hasAttr(script, 'src') || scriptType(script) !== 'classic') return;
    // An empty script is not started, so it runs if it gets text later.
    const text = script.text;
    if (text === '') return;
    N.markScriptStarted(script);
    const previous = currentScript;
    currentScript = script;
    try {
      indirectEval(native.modules.rewrite(text));
    } catch (error) {
      native.reportError(error);
    } finally {
      currentScript = previous;
    }
  }

  class Node extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get nodeType() {
      return N.nodeType(this);
    }
    get nodeName() {
      return N.nodeName(this);
    }
    get baseURI() {
      return internals.baseURL();
    }
    get isConnected() {
      if (N.isConnected(this)) return true;
      // A shadow tree is connected when its host is.
      const shadow = shadowState.get(N.rootNode(this));
      return shadow !== undefined && shadow.host.isConnected;
    }
    get ownerDocument() {
      if (N.nodeType(this) === 9) return null;
      if (!otherDocuments) return documentNode;
      // Nodes in a document made by createHTMLDocument belong to it.
      // Detached nodes are taken to belong to the page's document.
      const root = N.rootNode(this);
      return N.nodeType(root) === 9 ? root : documentNode;
    }
    getRootNode(options) {
      let root = N.rootNode(this);
      if (options !== null && typeof options === 'object' && options.composed) {
        for (let shadow = shadowState.get(root); shadow !== undefined; shadow = shadowState.get(root)) {
          root = N.rootNode(shadow.host);
        }
      }
      return root;
    }
    get parentNode() {
      return N.nav(this, 0);
    }
    get parentElement() {
      return N.nav(this, 5);
    }
    hasChildNodes() {
      return N.nav(this, 1) !== null;
    }
    get childNodes() {
      return cached(this, 'childNodes', () => nodeList(this, KIND.childNodes, null, null));
    }
    get firstChild() {
      return N.nav(this, 1);
    }
    get lastChild() {
      return N.nav(this, 2);
    }
    get previousSibling() {
      return N.nav(this, 3);
    }
    get nextSibling() {
      return N.nav(this, 4);
    }
    get nodeValue() {
      return N.data(this);
    }
    set nodeValue(value) {
      const type = N.nodeType(this);
      if (type === 3 || type === 8) N.setData(this, value === null ? '' : str(value));
    }
    get textContent() {
      return N.textContent(this);
    }
    set textContent(value) {
      N.setTextContent(this, value === null || value === undefined ? '' : str(value));
    }
    normalize() {
      N.normalize(this);
    }
    cloneNode(deep = false) {
      return cloneWithState(this, !!deep);
    }
    isEqualNode(other) {
      requireArgs(arguments, 1, "'isEqualNode' on 'Node'");
      return other === null ? false : N.isEqual(this, other);
    }
    isSameNode(other) {
      return this === other;
    }
    compareDocumentPosition(other) {
      requireArgs(arguments, 1, "'compareDocumentPosition' on 'Node'");
      return N.compare(this, other);
    }
    contains(other) {
      if (other === null || other === undefined) return false;
      return N.contains(this, other);
    }
    lookupPrefix(namespace) {
      for (let el = this.nodeType === 1 ? this : this.parentElement; el; el = el.parentElement) {
        if (namespace !== null && el.namespaceURI === namespace && el.prefix !== null) return el.prefix;
      }
      return null;
    }
    lookupNamespaceURI(prefix) {
      if (prefix === '') prefix = null;
      if (prefix === 'xml') return 'http://www.w3.org/XML/1998/namespace';
      if (prefix === 'xmlns') return 'http://www.w3.org/2000/xmlns/';
      for (let el = this.nodeType === 9 ? this.documentElement : (this.nodeType === 1 ? this : this.parentElement);
        el; el = el.parentElement) {
        if (el.prefix === prefix && el.namespaceURI !== null) return el.namespaceURI;
        const attr = prefix === null ? el.getAttributeNS('http://www.w3.org/2000/xmlns/', 'xmlns')
          : el.getAttributeNS('http://www.w3.org/2000/xmlns/', prefix);
        if (attr !== null) return attr || null;
      }
      return null;
    }
    isDefaultNamespace(namespace) {
      return this.lookupNamespaceURI(null) === (namespace === '' ? null : namespace);
    }
    insertBefore(node, child) {
      requireArgs(arguments, 2, "'insertBefore' on 'Node'");
      connected(N.insert(this, node, child === undefined ? null : child));
      return node;
    }
    appendChild(node) {
      requireArgs(arguments, 1, "'appendChild' on 'Node'");
      connected(N.insert(this, node, null));
      return node;
    }
    replaceChild(node, child) {
      requireArgs(arguments, 2, "'replaceChild' on 'Node'");
      connected(N.replaceChild(this, node, child));
      return child;
    }
    removeChild(child) {
      requireArgs(arguments, 1, "'removeChild' on 'Node'");
      N.removeChild(this, child);
      return child;
    }
  }
  for (const [name, value] of NODE_TYPES) {
    defineProperty(Node, name, { value, enumerable: true });
    defineProperty(Node.prototype, name, { value, enumerable: true });
  }
  tag(Node, 'Node');

  // Copies the JS-side state that cloning carries over (form control values).
  function cloneWithState(node, deep) {
    const copy = N.clone(node, deep);
    const hook = internals.cloningSteps;
    if (typeof hook === 'function') hook(node, copy);
    return copy;
  }

  // Turns the arguments of append() and friends into one node.
  function convertNodes(nodes) {
    if (nodes.length === 1) {
      const n = nodes[0];
      return n instanceof Node ? n : N.createText(str(n));
    }
    const fragment = N.createFragment();
    for (const n of nodes) N.insert(fragment, n instanceof Node ? n : N.createText(str(n)), null);
    return fragment;
  }

  // ParentNode. https://dom.spec.whatwg.org/#interface-parentnode
  const ParentNode = {
    get children() {
      return cached(this, 'children', () => htmlCollection(this, KIND.children, null, null));
    },
    get firstElementChild() {
      return N.nav(this, 6);
    },
    get lastElementChild() {
      return N.nav(this, 7);
    },
    get childElementCount() {
      return N.childCount(this, true);
    },
    prepend(...nodes) {
      connected(N.insert(this, convertNodes(nodes), N.nav(this, 1)));
    },
    append(...nodes) {
      connected(N.insert(this, convertNodes(nodes), null));
    },
    replaceChildren(...nodes) {
      connected(N.replaceAll(this, nodes.length === 0 ? null : convertNodes(nodes)));
    },
    querySelector(selectors) {
      requireArgs(arguments, 1, "'querySelector'");
      return N.query(this, str(selectors));
    },
    querySelectorAll(selectors) {
      requireArgs(arguments, 1, "'querySelectorAll'");
      return staticNodeList(N.queryAll(this, str(selectors)));
    },
  };

  // ChildNode. https://dom.spec.whatwg.org/#interface-childnode
  function viableSibling(node, nodes, step) {
    let s = N.nav(node, step);
    while (s !== null && nodes.includes(s)) s = N.nav(s, step);
    return s;
  }
  const ChildNode = {
    before(...nodes) {
      const parent = N.nav(this, 0);
      if (parent === null) return;
      const prev = viableSibling(this, nodes, 3);
      const node = convertNodes(nodes);
      connected(N.insert(parent, node, prev === null ? N.nav(parent, 1) : N.nav(prev, 4)));
    },
    after(...nodes) {
      const parent = N.nav(this, 0);
      if (parent === null) return;
      const next = viableSibling(this, nodes, 4);
      connected(N.insert(parent, convertNodes(nodes), next));
    },
    replaceWith(...nodes) {
      const parent = N.nav(this, 0);
      if (parent === null) return;
      const next = viableSibling(this, nodes, 4);
      const node = convertNodes(nodes);
      if (N.nav(this, 0) === parent) {
        connected(N.replaceChild(parent, node, this));
      } else {
        connected(N.insert(parent, node, next));
      }
    },
    remove() {
      N.remove(this);
    },
  };

  const NonDocumentTypeChildNode = {
    get previousElementSibling() {
      return N.nav(this, 8);
    },
    get nextElementSibling() {
      return N.nav(this, 9);
    },
  };

  function mixin(target, source) {
    for (const key of Reflect.ownKeys(source)) {
      const desc = Object.getOwnPropertyDescriptor(source, key);
      desc.enumerable = true;
      defineProperty(target.prototype, key, desc);
    }
  }

  // ---------------------------------------------------------------------
  // CharacterData, Text, Comment, DocumentType, DocumentFragment.

  class CharacterData extends Node {
    get data() {
      return N.data(this);
    }
    set data(value) {
      N.setData(this, value === null ? '' : str(value));
    }
    get length() {
      return N.data(this).length;
    }
    substringData(offset, count) {
      const data = N.data(this);
      offset >>>= 0;
      if (offset > data.length) throw domException('IndexSizeError', 'The offset is greater than the length.');
      return data.substring(offset, offset + (count >>> 0));
    }
    appendData(data) {
      N.setData(this, N.data(this) + str(data));
    }
    insertData(offset, data) {
      this.replaceData(offset, 0, data);
    }
    deleteData(offset, count) {
      this.replaceData(offset, count, '');
    }
    replaceData(offset, count, data) {
      const old = N.data(this);
      offset >>>= 0;
      if (offset > old.length) throw domException('IndexSizeError', 'The offset is greater than the length.');
      N.setData(this, old.slice(0, offset) + str(data) + old.slice(offset + (count >>> 0)));
    }
  }
  mixin(CharacterData, ChildNode);
  mixin(CharacterData, NonDocumentTypeChildNode);
  tag(CharacterData, 'CharacterData');

  function construct(node, newTarget, Base) {
    if (newTarget !== Base) setPrototypeOf(node, newTarget.prototype);
    return node;
  }

  class Text extends CharacterData {
    // eslint-disable-next-line constructor-super
    constructor(data = '') {
      return construct(N.createText(str(data)), new.target, Text);
    }
    splitText(offset) {
      const data = N.data(this);
      offset >>>= 0;
      if (offset > data.length) throw domException('IndexSizeError', 'The offset is greater than the length.');
      const rest = N.createText(data.slice(offset));
      const parent = N.nav(this, 0);
      if (parent !== null) N.insert(parent, rest, N.nav(this, 4));
      N.setData(this, data.slice(0, offset));
      return rest;
    }
    get wholeText() {
      let start = this;
      for (let p = N.nav(start, 3); p !== null && N.nodeType(p) === 3; p = N.nav(p, 3)) start = p;
      let out = '';
      for (let n = start; n !== null && N.nodeType(n) === 3; n = N.nav(n, 4)) out += N.data(n);
      return out;
    }
    get assignedSlot() {
      return null;
    }
  }
  tag(Text, 'Text');

  class CDATASection extends Text {}
  tag(CDATASection, 'CDATASection');

  class Comment extends CharacterData {
    constructor(data = '') {
      return construct(N.createComment(str(data)), new.target, Comment);
    }
  }
  tag(Comment, 'Comment');

  class ProcessingInstruction extends CharacterData {
    get target() {
      return '';
    }
  }
  tag(ProcessingInstruction, 'ProcessingInstruction');

  class DocumentType extends Node {
    get name() {
      return N.doctype(this)[0];
    }
    get publicId() {
      return N.doctype(this)[1];
    }
    get systemId() {
      return N.doctype(this)[2];
    }
  }
  mixin(DocumentType, ChildNode);
  tag(DocumentType, 'DocumentType');

  class DocumentFragment extends Node {
    constructor() {
      return construct(N.createFragment(), new.target, DocumentFragment);
    }
    getElementById(elementId) {
      return N.byId(this, str(elementId));
    }
  }
  mixin(DocumentFragment, ParentNode);
  tag(DocumentFragment, 'DocumentFragment');

  // Shadow roots. A shadow root is a document fragment in the arena that
  // holds the shadow tree; its wrapper gets ShadowRoot.prototype, and the
  // maps below tie it to its host (keeping each alive while the other is).
  // Nothing is rendered, so shadow DOM only matters to script: components
  // build their content in a shadow root and find it again through
  // `host.shadowRoot`. Not supported: slot assignment, retargeting events
  // that leave a shadow tree, declarative shadow roots in markup, and
  // serializing shadow trees into the document's HTML.
  const shadowOf = new WeakMap();
  const shadowState = new WeakMap();
  const SHADOW_HOSTS = new Set(['article', 'aside', 'blockquote', 'body', 'div', 'footer', 'h1', 'h2',
    'h3', 'h4', 'h5', 'h6', 'header', 'main', 'nav', 'p', 'section', 'span']);
  const RESERVED_CUSTOM_NAMES = new Set(['annotation-xml', 'color-profile', 'font-face',
    'font-face-src', 'font-face-uri', 'font-face-format', 'font-face-name', 'missing-glyph']);

  function isValidCustomElementName(name) {
    return /^[a-z][^A-Z\s/>]*-[^A-Z\s/>]*$/.test(name) && !RESERVED_CUSTOM_NAMES.has(name);
  }

  function shadowRootState(root) {
    const s = shadowState.get(root);
    if (s === undefined) throw new TypeError('Illegal invocation');
    return s;
  }

  class ShadowRoot extends DocumentFragment {
    // eslint-disable-next-line constructor-super
    constructor() {
      throw new TypeError('Illegal constructor');
    }
    get mode() {
      return shadowRootState(this).mode;
    }
    get host() {
      return shadowRootState(this).host;
    }
    get delegatesFocus() {
      return shadowRootState(this).delegatesFocus;
    }
    get slotAssignment() {
      return shadowRootState(this).slotAssignment;
    }
    get clonable() {
      return shadowRootState(this).clonable;
    }
    get serializable() {
      return shadowRootState(this).serializable;
    }
    get activeElement() {
      return null;
    }
    get innerHTML() {
      return N.innerHTML(this);
    }
    set innerHTML(value) {
      setShadowHTML(this, value === null ? '' : str(value));
    }
    setHTMLUnsafe(html) {
      setShadowHTML(this, str(html));
    }
    getHTML() {
      return N.innerHTML(this);
    }
    get adoptedStyleSheets() {
      const s = shadowRootState(this);
      return s.adoptedStyleSheets || (s.adoptedStyleSheets = []);
    }
    set adoptedStyleSheets(v) {
      shadowRootState(this).adoptedStyleSheets = Array.from(v);
    }
  }
  tag(ShadowRoot, 'ShadowRoot');

  // Parses `html` as a template's contents would be, then moves the result
  // into the shadow root.
  function setShadowHTML(root, html) {
    shadowRootState(root);
    const template = N.createElement('template');
    N.setInnerHTML(template, html);
    N.replaceAll(root, template.content);
  }

  function attachShadow(host, init) {
    const what = "Failed to execute 'attachShadow' on 'Element': ";
    const mode = init === null || init === undefined ? undefined : init.mode;
    if (mode !== 'open' && mode !== 'closed') {
      throw new TypeError(what + "Failed to read the 'mode' property from 'ShadowRootInit': The provided value '" +
        mode + "' is not a valid enum value of type ShadowRootMode.");
    }
    const name = host.localName;
    if (host.namespaceURI !== HTML_NS || !(SHADOW_HOSTS.has(name) || isValidCustomElementName(name))) {
      throw domException('NotSupportedError', what + 'This element does not support attachShadow');
    }
    if (shadowOf.has(host)) {
      throw domException('NotSupportedError', what + 'Shadow root cannot be created on a host which already hosts a shadow tree.');
    }
    const root = N.createFragment();
    setPrototypeOf(root, ShadowRoot.prototype);
    shadowState.set(root, {
      host,
      mode,
      delegatesFocus: !!init.delegatesFocus,
      slotAssignment: init.slotAssignment === 'manual' ? 'manual' : 'named',
      clonable: !!init.clonable,
      serializable: !!init.serializable,
      adoptedStyleSheets: null,
    });
    shadowOf.set(host, root);
    return root;
  }

  // ---------------------------------------------------------------------
  // Attributes: Attr and NamedNodeMap.

  // Attributes are not tree nodes in the arena; Attr objects are views of
  // an element's attribute, made on demand (so not identity-stable).
  class Attr extends Node {
    static create(owner, name, ns, value) {
      const attr = Object.create(Attr.prototype);
      new AttrState(attr, owner, name, ns, value);
      return attr;
    }
  }

  // Attr state, stamped on (Attr objects are made without a constructor).
  class AttrState extends Stamp {
    #a;
    constructor(o, owner, name, ns, value) {
      super(o);
      this.#a = { owner, name, ns, value };
    }
    static get(o) {
      if (!(#a in o)) throw new TypeError('Illegal invocation');
      return o.#a;
    }
  }
  const attrState = AttrState.get;

  function splitName(name) {
    const i = name.indexOf(':');
    return i < 0 ? [null, name] : [name.slice(0, i), name.slice(i + 1)];
  }

  const attrMembers = {
    get nodeType() { return 2; },
    get nodeName() { return attrState(this).name; },
    get name() { return attrState(this).name; },
    get localName() { return splitName(attrState(this).name)[1]; },
    get prefix() { return splitName(attrState(this).name)[0]; },
    get namespaceURI() { return attrState(this).ns; },
    get ownerElement() { return attrState(this).owner; },
    get ownerDocument() { return documentNode; },
    get specified() { return true; },
    get value() {
      const a = attrState(this);
      if (a.owner === null) return a.value;
      const v = a.ns === null ? N.getAttr(a.owner, a.name) : N.getAttrNS(a.owner, a.ns, splitName(a.name)[1]);
      return v === null ? a.value : (a.value = v);
    },
    set value(value) {
      const a = attrState(this);
      a.value = str(value);
      if (a.owner === null) return;
      if (a.ns === null) N.setAttr(a.owner, a.name, a.value);
      else N.setAttrNS(a.owner, a.ns, a.name, a.value);
    },
    get nodeValue() { return this.value; },
    set nodeValue(v) { this.value = v; },
    get textContent() { return this.value; },
    set textContent(v) { this.value = v; },
    get parentNode() { return null; },
    get parentElement() { return null; },
    get firstChild() { return null; },
    get lastChild() { return null; },
    get previousSibling() { return null; },
    get nextSibling() { return null; },
    get isConnected() { const o = attrState(this).owner; return o !== null && N.isConnected(o); },
    get childNodes() { return staticNodeList([]); },
    hasChildNodes() { return false; },
    cloneNode() { const a = attrState(this); return Attr.create(null, a.name, a.ns, this.value); },
  };
  mixin(Attr, attrMembers);
  tag(Attr, 'Attr');

  function attrAt(el, i) {
    const a = N.attr(el, i);
    return a === null ? null : Attr.create(el, a[0], a[1], a[4]);
  }

  function attrNamed(el, name) {
    const v = N.getAttr(el, name);
    if (v === null) return null;
    const lower = N.elementName(el)[1] === HTML_NS ? name.toLowerCase() : name;
    const n = N.attrCount(el);
    for (let i = 0; i < n; i++) {
      const a = N.attr(el, i);
      if (a[0] === lower) return Attr.create(el, a[0], a[1], a[4]);
    }
    return null;
  }

  const namedNodeMapHandler = {
    get(target, key, receiver) {
      const i = arrayIndex(key);
      const el = mapOwner(target);
      if (i >= 0) {
        const a = attrAt(el, i);
        return a === null ? undefined : a;
      }
      if (typeof key === 'string' && !(key in target)) {
        const a = attrNamed(el, key);
        return a === null ? undefined : a;
      }
      return Reflect.get(target, key, receiver);
    },
    has(target, key) {
      const i = arrayIndex(key);
      if (i >= 0) return i < N.attrCount(mapOwner(target));
      return Reflect.has(target, key);
    },
    ownKeys(target) {
      const keys = [];
      const n = N.attrCount(mapOwner(target));
      for (let i = 0; i < n; i++) keys.push(String(i));
      return keys.concat(Reflect.ownKeys(target));
    },
    getOwnPropertyDescriptor(target, key) {
      const i = arrayIndex(key);
      if (i >= 0) {
        const a = attrAt(mapOwner(target), i);
        return a === null ? undefined : { value: a, writable: false, enumerable: true, configurable: true };
      }
      return Reflect.getOwnPropertyDescriptor(target, key);
    },
  };

  class MapState extends Stamp {
    #el;
    constructor(o, el) {
      super(o);
      this.#el = el;
    }
    static owner(o) {
      o = unproxy(o);
      if (!(#el in o)) throw new TypeError('Illegal invocation');
      return o.#el;
    }
  }
  const mapOwner = MapState.owner;

  class NamedNodeMap {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return N.attrCount(mapOwner(this));
    }
    item(index) {
      return attrAt(mapOwner(this), index >>> 0);
    }
    getNamedItem(name) {
      return attrNamed(mapOwner(this), str(name));
    }
    getNamedItemNS(namespace, localName) {
      return mapOwner(this).getAttributeNodeNS(namespace, localName);
    }
    setNamedItem(attr) {
      return mapOwner(this).setAttributeNode(attr);
    }
    setNamedItemNS(attr) {
      return mapOwner(this).setAttributeNode(attr);
    }
    removeNamedItem(name) {
      const el = mapOwner(this);
      const attr = attrNamed(el, str(name));
      if (attr === null) throw domException('NotFoundError', "No item with name '" + name + "' was found.");
      el.removeAttribute(name);
      return attr;
    }
    removeNamedItemNS(namespace, localName) {
      const el = mapOwner(this);
      const attr = el.getAttributeNodeNS(namespace, localName);
      if (attr === null) throw domException('NotFoundError', 'No item was found.');
      el.removeAttributeNS(namespace, localName);
      return attr;
    }
    *[Symbol.iterator]() {
      const el = mapOwner(this);
      for (let i = 0; i < N.attrCount(el); i++) yield attrAt(el, i);
    }
  }
  tag(NamedNodeMap, 'NamedNodeMap');

  function namedNodeMap(el) {
    const target = Object.create(NamedNodeMap.prototype);
    new MapState(target, el);
    return new Proxy(target, namedNodeMapHandler);
  }

  // ---------------------------------------------------------------------
  // DOMTokenList (classList, relList).

  const ASCII_WS = /[\t\n\f\r ]+/;

  class TokenState extends Stamp {
    #t;
    constructor(o, el, attr, supported) {
      super(o);
      this.#t = { el, attr, supported };
    }
    static get(o) {
      o = unproxy(o);
      if (!(#t in o)) throw new TypeError('Illegal invocation');
      return o.#t;
    }
  }
  const tokenState = TokenState.get;

  function tokens(t) {
    const v = N.getAttr(t.el, t.attr);
    if (v === null) return [];
    const out = [];
    for (const tok of v.split(ASCII_WS)) if (tok && !out.includes(tok)) out.push(tok);
    return out;
  }

  function checkToken(token, what) {
    token = str(token);
    if (token === '') throw domException('SyntaxError', "Failed to execute '" + what + "' on 'DOMTokenList': The token provided must not be empty.");
    if (/[\t\n\f\r ]/.test(token)) {
      throw domException('InvalidCharacterError', "Failed to execute '" + what + "' on 'DOMTokenList': The token provided ('" + token + "') contains HTML space characters, which are not valid in tokens.");
    }
    return token;
  }

  function writeTokens(t, list) {
    N.setAttr(t.el, t.attr, list.join(' '));
  }

  class DOMTokenList {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return tokens(tokenState(this)).length;
    }
    item(index) {
      const v = tokens(tokenState(this))[index >>> 0];
      return v === undefined ? null : v;
    }
    contains(token) {
      return tokens(tokenState(this)).includes(str(token));
    }
    add(...toAdd) {
      const t = tokenState(this);
      const checked = toAdd.map((x) => checkToken(x, 'add'));
      const list = tokens(t);
      for (const tok of checked) if (!list.includes(tok)) list.push(tok);
      writeTokens(t, list);
    }
    remove(...toRemove) {
      const t = tokenState(this);
      const checked = toRemove.map((x) => checkToken(x, 'remove'));
      if (N.getAttr(t.el, t.attr) === null) return;
      writeTokens(t, tokens(t).filter((x) => !checked.includes(x)));
    }
    toggle(token, force) {
      const t = tokenState(this);
      token = checkToken(token, 'toggle');
      const list = tokens(t);
      if (list.includes(token)) {
        if (force === true) return true;
        writeTokens(t, list.filter((x) => x !== token));
        return false;
      }
      if (force === false) return false;
      list.push(token);
      writeTokens(t, list);
      return true;
    }
    replace(token, newToken) {
      const t = tokenState(this);
      token = checkToken(token, 'replace');
      newToken = checkToken(newToken, 'replace');
      const list = tokens(t);
      const i = list.indexOf(token);
      if (i < 0) return false;
      const out = [];
      for (const x of list) {
        const v = x === token ? newToken : x;
        if (!out.includes(v)) out.push(v);
      }
      writeTokens(t, out);
      return true;
    }
    supports(token) {
      const t = tokenState(this);
      if (t.supported === null) {
        throw new TypeError("Failed to execute 'supports' on 'DOMTokenList': DOMTokenList has no supported tokens.");
      }
      return t.supported.includes(str(token).toLowerCase());
    }
    get value() {
      const t = tokenState(this);
      const v = N.getAttr(t.el, t.attr);
      return v === null ? '' : v;
    }
    set value(value) {
      const t = tokenState(this);
      N.setAttr(t.el, t.attr, str(value));
    }
    toString() {
      return this.value;
    }
    forEach(callback, thisArg) {
      const list = tokens(tokenState(this));
      for (let i = 0; i < list.length; i++) ReflectApply(callback, thisArg, [list[i], i, this]);
    }
    entries() {
      return tokens(tokenState(this)).entries();
    }
    keys() {
      return tokens(tokenState(this)).keys();
    }
    values() {
      return tokens(tokenState(this)).values();
    }
    [Symbol.iterator]() {
      return tokens(tokenState(this)).values();
    }
  }
  tag(DOMTokenList, 'DOMTokenList');

  const tokenListHandler = {
    get(target, key, receiver) {
      const i = arrayIndex(key);
      if (i >= 0) {
        const v = tokens(tokenState(target))[i];
        return v;
      }
      return Reflect.get(target, key, receiver);
    },
    has(target, key) {
      const i = arrayIndex(key);
      if (i >= 0) return i < tokens(tokenState(target)).length;
      return Reflect.has(target, key);
    },
  };

  function tokenList(el, attr, supported = null) {
    const target = Object.create(DOMTokenList.prototype);
    new TokenState(target, el, attr, supported);
    return new Proxy(target, tokenListHandler);
  }

  // ---------------------------------------------------------------------
  // Inline style: CSSStyleDeclaration over the style attribute.

  function kebab(prop) {
    if (prop.startsWith('--') || prop.includes('-')) return prop.toLowerCase().startsWith('--') ? prop : prop.toLowerCase();
    if (prop === 'cssFloat') return 'float';
    let out = prop.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
    if (/^(webkit|moz|ms|o)-/.test(out)) out = '-' + out;
    return out;
  }

  function camel(name) {
    if (name === 'float') return 'cssFloat';
    let n = name;
    if (n.startsWith('-ms-')) n = n.slice(1);
    else if (n.startsWith('-')) n = n.slice(1).replace(/^(\w)/, (c) => c.toUpperCase());
    return n.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  }

  // Splits declarations on semicolons outside strings and brackets.
  function parseDeclarations(text) {
    const decls = [];
    let depth = 0;
    let quote = '';
    let start = 0;
    const push = (end) => {
      const part = text.slice(start, end);
      const colon = part.indexOf(':');
      if (colon > 0) {
        let name = part.slice(0, colon).trim();
        let value = part.slice(colon + 1).trim();
        let important = false;
        const m = /!\s*important\s*$/i.exec(value);
        if (m) {
          important = true;
          value = value.slice(0, m.index).trim();
        }
        if (!name.startsWith('--')) name = name.toLowerCase();
        if (name && value) {
          const i = decls.findIndex((d) => d[0] === name);
          if (i >= 0) decls.splice(i, 1);
          decls.push([name, value, important]);
        }
      }
      start = end + 1;
    };
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (quote) {
        if (c === '\\') i++;
        else if (c === quote) quote = '';
      } else if (c === '"' || c === "'") {
        quote = c;
      } else if (c === '(' || c === '[') {
        depth++;
      } else if ((c === ')' || c === ']') && depth > 0) {
        depth--;
      } else if (c === ';' && depth === 0) {
        push(i);
      }
    }
    push(text.length);
    return decls;
  }

  function serializeDeclarations(decls) {
    return decls.map(([n, v, imp]) => n + ': ' + v + (imp ? ' !important' : '') + ';').join(' ');
  }

  class StyleState extends Stamp {
    #s;
    constructor(o, s) {
      super(o);
      this.#s = s;
    }
    static get(o) {
      o = unproxy(o);
      if (!(#s in o)) throw new TypeError('Illegal invocation');
      return o.#s;
    }
  }
  const styleState = StyleState.get;

  // The declarations: parsed from the owner's style attribute (re-parsed
  // only when it changed), or fixed for a computed style.
  function declarations(s) {
    if (s.owner === null) return s.decls;
    const text = N.getAttr(s.owner, 'style');
    if (text !== s.text) {
      s.text = text;
      s.decls = text === null ? [] : parseDeclarations(text);
    }
    return s.decls;
  }

  function writeDeclarations(s, decls) {
    if (s.readOnly) {
      throw domException('NoModificationAllowedError', 'These styles are computed, and therefore read-only.');
    }
    const text = serializeDeclarations(decls);
    s.decls = decls;
    s.text = text;
    N.setAttr(s.owner, 'style', text);
  }

  class CSSStyleDeclaration {
    constructor(key) {
      illegal(key);
    }
    get cssText() {
      const s = styleState(this);
      return s.owner === null ? serializeDeclarations(s.decls) : serializeDeclarations(declarations(s));
    }
    set cssText(value) {
      const s = styleState(this);
      writeDeclarations(s, parseDeclarations(value === null ? '' : str(value)));
    }
    get length() {
      return declarations(styleState(this)).length;
    }
    item(index) {
      const d = declarations(styleState(this))[index >>> 0];
      return d === undefined ? '' : d[0];
    }
    getPropertyValue(property) {
      const name = str(property);
      const key = name.startsWith('--') ? name : name.toLowerCase();
      const d = declarations(styleState(this)).find((x) => x[0] === key);
      return d === undefined ? '' : d[1];
    }
    getPropertyPriority(property) {
      const name = str(property).toLowerCase();
      const d = declarations(styleState(this)).find((x) => x[0] === name);
      return d !== undefined && d[2] ? 'important' : '';
    }
    setProperty(property, value, priority = '') {
      const s = styleState(this);
      let name = str(property);
      if (!name.startsWith('--')) name = name.toLowerCase();
      if (value === null || value === undefined || str(value).trim() === '') {
        this.removeProperty(name);
        return;
      }
      const v = str(value).trim();
      const important = str(priority).toLowerCase() === 'important';
      const decls = declarations(s).slice();
      const i = decls.findIndex((d) => d[0] === name);
      if (i >= 0) decls[i] = [name, v, important];
      else decls.push([name, v, important]);
      writeDeclarations(s, decls);
    }
    removeProperty(property) {
      const s = styleState(this);
      let name = str(property);
      if (!name.startsWith('--')) name = name.toLowerCase();
      const decls = declarations(s);
      const i = decls.findIndex((d) => d[0] === name);
      if (i < 0) return '';
      const old = decls[i][1];
      writeDeclarations(s, decls.filter((_, j) => j !== i));
      return old;
    }
    get cssFloat() {
      return this.getPropertyValue('float');
    }
    set cssFloat(value) {
      this.setProperty('float', value);
    }
    get parentRule() {
      return null;
    }
    *[Symbol.iterator]() {
      for (const d of declarations(styleState(this))) yield d[0];
    }
  }
  tag(CSSStyleDeclaration, 'CSSStyleDeclaration');

  const CSS_PROP = /^-?-?[a-zA-Z][a-zA-Z0-9-]*$/;
  const styleHandler = {
    get(target, key, receiver) {
      if (typeof key !== 'string' || key in target) return Reflect.get(target, key, receiver);
      const i = arrayIndex(key);
      if (i >= 0) {
        const d = declarations(styleState(target))[i];
        return d === undefined ? undefined : d[0];
      }
      if (!CSS_PROP.test(key)) return undefined;
      return CSSStyleDeclaration.prototype.getPropertyValue.call(target, kebab(key));
    },
    set(target, key, value, receiver) {
      if (typeof key !== 'string' || key in target) return Reflect.set(target, key, value, receiver);
      if (arrayIndex(key) >= 0 || !CSS_PROP.test(key)) return true;
      CSSStyleDeclaration.prototype.setProperty.call(target, kebab(key), value);
      return true;
    },
    has(target, key) {
      return typeof key === 'string' ? (key in target || CSS_PROP.test(key)) : Reflect.has(target, key);
    },
    ownKeys(target) {
      const decls = declarations(styleState(target));
      return decls.map((_, i) => String(i)).concat(Reflect.ownKeys(target));
    },
    getOwnPropertyDescriptor(target, key) {
      const i = arrayIndex(key);
      if (i >= 0) {
        const d = declarations(styleState(target))[i];
        return d === undefined ? undefined : { value: d[0], writable: false, enumerable: true, configurable: true };
      }
      return Reflect.getOwnPropertyDescriptor(target, key);
    },
  };

  function makeStyle(owner, decls, readOnly) {
    const target = Object.create(CSSStyleDeclaration.prototype);
    new StyleState(target, { owner, decls, text: undefined, readOnly });
    return new Proxy(target, styleHandler);
  }

  function inlineStyle(el) {
    return cached(el, 'style', () => makeStyle(el, [], false));
  }

  // ---------------------------------------------------------------------
  // dataset: DOMStringMap over data-* attributes.

  class DOMStringMap {
    constructor(key) {
      illegal(key);
    }
  }
  tag(DOMStringMap, 'DOMStringMap');

  function dataAttr(key) {
    if (/-[a-z]/.test(key)) {
      throw domException('SyntaxError', "'" + key + "' is not a valid property name.");
    }
    return 'data-' + key.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
  }

  function dataKey(attr) {
    return attr.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  }

  function dataset(el) {
    const handler = {
      get(target, key, receiver) {
        if (typeof key !== 'string' || key in target) return Reflect.get(target, key, receiver);
        const v = N.getAttr(el, 'data-' + key.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()));
        return v === null ? undefined : v;
      },
      set(target, key, value) {
        if (typeof key !== 'string') return false;
        N.setAttr(el, dataAttr(key), str(value));
        return true;
      },
      deleteProperty(target, key) {
        if (typeof key !== 'string') return true;
        N.removeAttr(el, 'data-' + key.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()));
        return true;
      },
      has(target, key) {
        if (typeof key !== 'string') return Reflect.has(target, key);
        return N.getAttr(el, 'data-' + key.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())) !== null;
      },
      ownKeys() {
        const keys = [];
        const n = N.attrCount(el);
        for (let i = 0; i < n; i++) {
          const a = N.attr(el, i);
          if (a[1] === null && a[0].startsWith('data-') && !/[A-Z]/.test(a[0])) keys.push(dataKey(a[0]));
        }
        return keys;
      },
      getOwnPropertyDescriptor(target, key) {
        if (typeof key !== 'string') return undefined;
        const v = N.getAttr(el, 'data-' + key.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()));
        return v === null ? undefined : { value: v, writable: true, enumerable: true, configurable: true };
      },
      defineProperty(target, key, desc) {
        if (typeof key !== 'string' || !('value' in desc)) return false;
        N.setAttr(el, dataAttr(key), str(desc.value));
        return true;
      },
    };
    return new Proxy(Object.create(DOMStringMap.prototype), handler);
  }

  // ---------------------------------------------------------------------
  // Geometry. Nothing is laid out (see LayoutMode in window.rs): every
  // rendered element is a box as wide as the viewport, hidden ones are
  // empty. In the default 'inView' layout each box is one 20px line at the
  // top of the viewport, so anything gated on visibility counts as visible
  // and in view. In the 'scrolled' layout the elements under <body> are
  // stacked in document order, a line each, every element spanning its
  // descendants (N.layoutLines), and boxes move as the page scrolls. Either
  // way the root element is as tall as the content, at least the viewport.
  //
  // internals.elementBox gives an element's [x, y, width, height] in
  // document coordinates; replace it to change all of this.

  const LINE = 20;
  const SCROLLED = env.layout === 'scrolled';
  const ROOT_BOX = { version: -1, height: 0 };

  function rootHeight() {
    const v = N.version();
    if (ROOT_BOX.version !== v) {
      ROOT_BOX.version = v;
      let lines;
      if (SCROLLED) {
        const body = N.docPart(2);
        lines = body === null ? 0 : N.layoutLines([body])[1];
      } else {
        lines = N.count(documentNode, 'body *');
      }
      ROOT_BOX.height = Math.max(env.height, lines * LINE);
    }
    return ROOT_BOX.height;
  }

  function rendered(el) {
    return N.isConnected(el) && N.matches(el, ':visible');
  }

  // Where an element sits in the stacked layout, as [y, height] in
  // document coordinates, or null if it is not rendered under <body>.
  function stackedBox(el) {
    if (!rendered(el)) return null;
    const lines = N.layoutLines([el]);
    return lines[0] < 0 ? null : [lines[0] * LINE, lines[1] * LINE];
  }

  function defaultElementBox(el) {
    if (!rendered(el)) return [0, 0, 0, 0];
    const name = N.elementName(el);
    if (name[1] === HTML_NS && (name[0] === 'html' || name[0] === 'body')) {
      return [0, 0, env.width, rootHeight()];
    }
    if (SCROLLED) {
      const stacked = stackedBox(el);
      if (stacked !== null) return [0, stacked[0], env.width, stacked[1]];
    }
    return [0, 0, env.width, LINE];
  }
  internals.elementBox = defaultElementBox;
  if (SCROLLED) {
    internals.scrollLimit = () => [0, Math.max(0, rootHeight() - env.height)];
  }

  function box(el) {
    return internals.elementBox(el);
  }

  // An element's box relative to the viewport, as getBoundingClientRect()
  // reports it. In the 'inView' layout boxes stay put when scrolling.
  function clientBox(el) {
    const b = box(el);
    if (!SCROLLED || (b[2] === 0 && b[3] === 0)) return b;
    const pos = internals.scrollPosition();
    return [b[0] - pos[0], b[1] - pos[1], b[2], b[3]];
  }

  // Scrolls the window to bring el into view, in either layout, so pages
  // that scroll to an element see the position move and get scroll events.
  function scrollIntoView(el, block, ifNeeded) {
    const stacked = stackedBox(el);
    if (stacked === null) return;
    const [y, height] = stacked;
    const pos = internals.scrollPosition();
    const viewport = env.height;
    const inView = y >= pos[1] && y + height <= pos[1] + viewport;
    if (ifNeeded && (inView || !SCROLLED)) return;
    let top;
    switch (block) {
      case 'end':
        top = y + height - viewport;
        break;
      case 'center':
        top = y + height / 2 - viewport / 2;
        break;
      case 'nearest':
        if (inView) return;
        top = y < pos[1] ? y : y + height - viewport;
        break;
      default:
        top = y;
    }
    internals.scrollTo(pos[0], Math.max(0, top));
  }

  class DOMRectReadOnly {
    #r;
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.#r = [+x, +y, +width, +height];
    }
    static fromRect(rect = {}) {
      return new this(rect.x, rect.y, rect.width, rect.height);
    }
    static {
      DOMRectReadOnly.raw = (r) => r.#r;
    }
    get x() { return this.#r[0]; }
    get y() { return this.#r[1]; }
    get width() { return this.#r[2]; }
    get height() { return this.#r[3]; }
    get left() { return Math.min(this.#r[0], this.#r[0] + this.#r[2]); }
    get top() { return Math.min(this.#r[1], this.#r[1] + this.#r[3]); }
    get right() { return Math.max(this.#r[0], this.#r[0] + this.#r[2]); }
    get bottom() { return Math.max(this.#r[1], this.#r[1] + this.#r[3]); }
    toJSON() {
      const { x, y, width, height, top, right, bottom, left } = this;
      return { x, y, width, height, top, right, bottom, left };
    }
  }
  const rawRect = DOMRectReadOnly.raw;
  delete DOMRectReadOnly.raw;
  tag(DOMRectReadOnly, 'DOMRectReadOnly');

  class DOMRect extends DOMRectReadOnly {
    get x() { return rawRect(this)[0]; }
    set x(v) { rawRect(this)[0] = +v; }
    get y() { return rawRect(this)[1]; }
    set y(v) { rawRect(this)[1] = +v; }
    get width() { return rawRect(this)[2]; }
    set width(v) { rawRect(this)[2] = +v; }
    get height() { return rawRect(this)[3]; }
    set height(v) { rawRect(this)[3] = +v; }
  }
  tag(DOMRect, 'DOMRect');

  class DOMRectList {
    #rects;
    constructor(key, rects) {
      illegal(key);
      this.#rects = rects;
      rects.forEach((r, i) => defineProperty(this, i, { value: r, enumerable: true }));
    }
    get length() {
      return this.#rects.length;
    }
    item(i) {
      return this.#rects[i >>> 0] || null;
    }
    [Symbol.iterator]() {
      return this.#rects.values();
    }
  }
  tag(DOMRectList, 'DOMRectList');

  function isRootScroller(el) {
    const name = N.elementName(el);
    return name[1] === HTML_NS && (name[0] === 'html' || (name[0] === 'body' && N.nav(el, 0) === N.docPart(0)));
  }

  function scrollElement(el, x, y) {
    if (isRootScroller(el)) {
      internals.scrollTo(x, y);
      return;
    }
    const s = state(el);
    x = Math.max(0, +x || 0);
    y = Math.max(0, +y || 0);
    if (s.scrollLeft === x && s.scrollTop === y) return;
    s.scrollLeft = x;
    s.scrollTop = y;
    internals.scrolled();
    if (!s.scrollQueued) {
      s.scrollQueued = true;
      queueTask(() => {
        s.scrollQueued = false;
        internals.fire(el, 'scroll', { bubbles: false });
      });
    }
  }

  function scrollArgs(el, args, relative) {
    const pos = isRootScroller(el) ? internals.scrollPosition() : [state(el).scrollLeft || 0, state(el).scrollTop || 0];
    let x;
    let y;
    if (args.length === 1 && typeof args[0] === 'object' && args[0] !== null) {
      x = args[0].left;
      y = args[0].top;
      x = x === undefined ? (relative ? 0 : pos[0]) : +x;
      y = y === undefined ? (relative ? 0 : pos[1]) : +y;
    } else {
      x = +args[0] || 0;
      y = +args[1] || 0;
    }
    if (relative) {
      x += pos[0];
      y += pos[1];
    }
    scrollElement(el, x, y);
  }

  // ---------------------------------------------------------------------
  // Element.

  const HTML_NS = 'http://www.w3.org/1999/xhtml';
  const SVG_NS = 'http://www.w3.org/2000/svg';

  class Element extends Node {
    get namespaceURI() {
      return N.elementName(this)[1];
    }
    get prefix() {
      return N.elementName(this)[2];
    }
    get localName() {
      return N.elementName(this)[0];
    }
    get tagName() {
      return N.tagName(this);
    }
    get id() {
      const v = N.getAttr(this, 'id');
      return v === null ? '' : v;
    }
    set id(value) {
      N.setAttr(this, 'id', str(value));
    }
    get className() {
      const v = N.getAttr(this, 'class');
      return v === null ? '' : v;
    }
    set className(value) {
      N.setAttr(this, 'class', str(value));
    }
    get classList() {
      return cached(this, 'classList', () => tokenList(this, 'class'));
    }
    set classList(value) {
      N.setAttr(this, 'class', str(value));
    }
    get slot() {
      const v = N.getAttr(this, 'slot');
      return v === null ? '' : v;
    }
    set slot(value) {
      N.setAttr(this, 'slot', str(value));
    }
    get part() {
      return cached(this, 'part', () => tokenList(this, 'part'));
    }
    get attributes() {
      return cached(this, 'attributes', () => namedNodeMap(this));
    }
    hasAttributes() {
      return N.attrCount(this) > 0;
    }
    getAttributeNames() {
      const names = [];
      const n = N.attrCount(this);
      for (let i = 0; i < n; i++) names.push(N.attr(this, i)[0]);
      return names;
    }
    getAttribute(name) {
      requireArgs(arguments, 1, "'getAttribute' on 'Element'");
      return N.getAttr(this, str(name));
    }
    getAttributeNS(namespace, localName) {
      return N.getAttrNS(this, nullableStr(namespace) || null, str(localName));
    }
    setAttribute(name, value) {
      requireArgs(arguments, 2, "'setAttribute' on 'Element'");
      N.setAttr(this, str(name), str(value));
    }
    setAttributeNS(namespace, name, value) {
      requireArgs(arguments, 3, "'setAttributeNS' on 'Element'");
      N.setAttrNS(this, nullableStr(namespace) || null, str(name), str(value));
    }
    removeAttribute(name) {
      requireArgs(arguments, 1, "'removeAttribute' on 'Element'");
      N.removeAttr(this, str(name));
    }
    removeAttributeNS(namespace, localName) {
      N.removeAttrNS(this, nullableStr(namespace) || null, str(localName));
    }
    toggleAttribute(name, force) {
      requireArgs(arguments, 1, "'toggleAttribute' on 'Element'");
      name = str(name);
      if (N.hasAttr(this, name)) {
        if (force === true) return true;
        N.removeAttr(this, name);
        return false;
      }
      if (force === false) return false;
      N.setAttr(this, name, '');
      return true;
    }
    hasAttribute(name) {
      requireArgs(arguments, 1, "'hasAttribute' on 'Element'");
      return N.hasAttr(this, str(name));
    }
    hasAttributeNS(namespace, localName) {
      return N.getAttrNS(this, nullableStr(namespace) || null, str(localName)) !== null;
    }
    getAttributeNode(name) {
      return attrNamed(this, str(name));
    }
    getAttributeNodeNS(namespace, localName) {
      namespace = nullableStr(namespace) || null;
      localName = str(localName);
      const n = N.attrCount(this);
      for (let i = 0; i < n; i++) {
        const a = N.attr(this, i);
        if (a[1] === namespace && a[3] === localName) return Attr.create(this, a[0], a[1], a[4]);
      }
      return null;
    }
    setAttributeNode(attr) {
      const a = attrState(attr);
      if (a.owner !== null && a.owner !== this) {
        throw domException('InUseAttributeError', 'The attribute is in use by another element.');
      }
      const old = a.ns === null ? this.getAttributeNode(a.name) : this.getAttributeNodeNS(a.ns, splitName(a.name)[1]);
      if (a.ns === null) N.setAttr(this, a.name, a.value);
      else N.setAttrNS(this, a.ns, a.name, a.value);
      a.owner = this;
      return old;
    }
    setAttributeNodeNS(attr) {
      return this.setAttributeNode(attr);
    }
    removeAttributeNode(attr) {
      const a = attrState(attr);
      if (a.owner !== this) throw domException('NotFoundError', 'The attribute is not owned by this element.');
      a.value = attr.value;
      if (a.ns === null) N.removeAttr(this, a.name);
      else N.removeAttrNS(this, a.ns, splitName(a.name)[1]);
      a.owner = null;
      return attr;
    }
    get innerHTML() {
      return N.innerHTML(this);
    }
    set innerHTML(value) {
      N.setInnerHTML(this, value === null ? '' : str(value));
    }
    get outerHTML() {
      return N.outerHTML(this);
    }
    set outerHTML(value) {
      N.setOuterHTML(this, value === null ? '' : str(value));
    }
    insertAdjacentHTML(position, text) {
      requireArgs(arguments, 2, "'insertAdjacentHTML' on 'Element'");
      N.insertAdjacentHTML(this, str(position), str(text));
    }
    insertAdjacentElement(where, element) {
      requireArgs(arguments, 2, "'insertAdjacentElement' on 'Element'");
      return insertAdjacent(this, str(where), element);
    }
    insertAdjacentText(where, data) {
      requireArgs(arguments, 2, "'insertAdjacentText' on 'Element'");
      insertAdjacent(this, str(where), N.createText(str(data)));
    }
    getElementsByTagName(qualifiedName) {
      return htmlCollection(this, KIND.tag, str(qualifiedName), null);
    }
    getElementsByTagNameNS(namespace, localName) {
      return htmlCollection(this, KIND.tagNS, nullableStr(namespace), str(localName));
    }
    getElementsByClassName(classNames) {
      return htmlCollection(this, KIND.className, str(classNames), null);
    }
    matches(selectors) {
      requireArgs(arguments, 1, "'matches' on 'Element'");
      return N.matches(this, str(selectors));
    }
    webkitMatchesSelector(selectors) {
      return N.matches(this, str(selectors));
    }
    closest(selectors) {
      requireArgs(arguments, 1, "'closest' on 'Element'");
      return N.closest(this, str(selectors));
    }
    get shadowRoot() {
      const root = shadowOf.get(this);
      return root !== undefined && shadowState.get(root).mode === 'open' ? root : null;
    }
    get assignedSlot() {
      return null;
    }
    getBoundingClientRect() {
      const b = clientBox(this);
      return new DOMRect(b[0], b[1], b[2], b[3]);
    }
    getClientRects() {
      const b = clientBox(this);
      return new DOMRectList(KEY, b[2] === 0 && b[3] === 0 ? [] : [new DOMRect(b[0], b[1], b[2], b[3])]);
    }
    checkVisibility() {
      return N.matches(this, ':visible');
    }
    get clientTop() { return 0; }
    get clientLeft() { return 0; }
    get clientWidth() {
      return isRootScroller(this) && N.elementName(this)[0] === 'html' ? env.width : box(this)[2];
    }
    get clientHeight() {
      return isRootScroller(this) && N.elementName(this)[0] === 'html' ? env.height : box(this)[3];
    }
    get scrollWidth() { return box(this)[2]; }
    get scrollHeight() { return box(this)[3]; }
    get scrollTop() {
      return isRootScroller(this) ? internals.scrollPosition()[1] : state(this).scrollTop || 0;
    }
    set scrollTop(value) {
      scrollElement(this, this.scrollLeft, value);
    }
    get scrollLeft() {
      return isRootScroller(this) ? internals.scrollPosition()[0] : state(this).scrollLeft || 0;
    }
    set scrollLeft(value) {
      scrollElement(this, value, this.scrollTop);
    }
    scroll(...args) {
      scrollArgs(this, args, false);
    }
    scrollTo(...args) {
      scrollArgs(this, args, false);
    }
    scrollBy(...args) {
      scrollArgs(this, args, true);
    }
    scrollIntoView(arg) {
      let block = 'start';
      if (arg === false) block = 'end';
      else if (arg !== null && typeof arg === 'object' && arg.block !== undefined) block = str(arg.block);
      scrollIntoView(this, block, false);
    }
    scrollIntoViewIfNeeded(center = true) {
      scrollIntoView(this, center ? 'center' : 'nearest', true);
    }
    attachShadow(init) {
      requireArgs(arguments, 1, "'attachShadow' on 'Element'");
      return attachShadow(this, init);
    }
    requestFullscreen() {
      return Promise.reject(domException('NotSupportedError', 'Fullscreen is not supported.'));
    }
    requestPointerLock() {}
    setPointerCapture() {}
    releasePointerCapture() {}
    hasPointerCapture() {
      return false;
    }
    animate() {
      return { finished: Promise.resolve(), cancel() {}, finish() {}, play() {}, pause() {}, onfinish: null };
    }
    getAnimations() {
      return [];
    }
  }
  mixin(Element, ParentNode);
  mixin(Element, ChildNode);
  mixin(Element, NonDocumentTypeChildNode);
  tag(Element, 'Element');

  function insertAdjacent(el, where, node) {
    switch (where.toLowerCase()) {
      case 'beforebegin': {
        const parent = N.nav(el, 0);
        if (parent === null) return null;
        connected(N.insert(parent, node, el));
        return node;
      }
      case 'afterbegin':
        connected(N.insert(el, node, N.nav(el, 1)));
        return node;
      case 'beforeend':
        connected(N.insert(el, node, null));
        return node;
      case 'afterend': {
        const parent = N.nav(el, 0);
        if (parent === null) return null;
        connected(N.insert(parent, node, N.nav(el, 4)));
        return node;
      }
      default:
        throw domException('SyntaxError', "The value provided ('" + where + "') is not one of 'beforebegin', 'afterbegin', 'beforeend', or 'afterend'.");
    }
  }

  // ---------------------------------------------------------------------
  // Attribute reflection helpers for HTML element interfaces.

  function resolveURL(url) {
    const parts = internals.urlParse(url, internals.baseURL());
    return parts === undefined ? url : parts[internals.URL_PARTS.HREF];
  }

  // spec: { prop: type } or { prop: [type, attr, extra] }. Types: 'string',
  // 'url', 'bool', 'long', 'ulong', 'number', 'enum' (extra: [values,
  // missing, invalid]), 'nullable' (string or null).
  function reflect(Ctor, spec) {
    for (const prop of Object.keys(spec)) {
      let entry = spec[prop];
      if (typeof entry === 'string') entry = [entry];
      const [type, attrName = prop.toLowerCase(), extra] = entry;
      let get;
      let set = function (value) {
        N.setAttr(this, attrName, str(value));
      };
      switch (type) {
        case 'string':
          get = function () {
            const v = N.getAttr(this, attrName);
            return v === null ? (extra === undefined ? '' : extra) : v;
          };
          break;
        case 'nullable':
          get = function () {
            return N.getAttr(this, attrName);
          };
          set = function (value) {
            if (value === null) N.removeAttr(this, attrName);
            else N.setAttr(this, attrName, str(value));
          };
          break;
        case 'url':
          get = function () {
            const v = N.getAttr(this, attrName);
            return v === null ? '' : resolveURL(v);
          };
          break;
        case 'bool':
          get = function () {
            return N.hasAttr(this, attrName);
          };
          set = function (value) {
            if (value) N.setAttr(this, attrName, '');
            else N.removeAttr(this, attrName);
          };
          break;
        case 'long':
        case 'ulong': {
          const fallback = extra === undefined ? (type === 'long' ? 0 : 0) : extra;
          get = function () {
            const v = N.getAttr(this, attrName);
            if (v === null) return typeof fallback === 'function' ? fallback(this) : fallback;
            const n = parseInt(v, 10);
            if (Number.isNaN(n) || (type === 'ulong' && n < 0)) return typeof fallback === 'function' ? fallback(this) : fallback;
            return n;
          };
          set = function (value) {
            N.setAttr(this, attrName, String(type === 'ulong' ? value >>> 0 : value | 0));
          };
          break;
        }
        case 'number':
          get = function () {
            const v = N.getAttr(this, attrName);
            const n = v === null ? NaN : parseFloat(v);
            return Number.isFinite(n) ? n : extra;
          };
          set = function (value) {
            N.setAttr(this, attrName, String(+value));
          };
          break;
        case 'enum': {
          const [values, missing = '', invalid = missing] = extra;
          get = function () {
            const v = N.getAttr(this, attrName);
            if (v === null) return missing;
            const lower = v.toLowerCase();
            return values.includes(lower) ? lower : invalid;
          };
          break;
        }
        default:
          throw new Error('bad reflect type ' + type);
      }
      defineProperty(Ctor.prototype, prop, { get, set, enumerable: true, configurable: true });
    }
  }

  function define(Ctor, members) {
    mixin(Ctor, members);
  }

  // ---------------------------------------------------------------------
  // HTMLElement.

  let focused = null;

  function setFocus(el) {
    if (focused === el) return;
    const old = focused;
    focused = el;
    if (old !== null) {
      internals.fire(old, 'blur', { relatedTarget: el }, uiEvents.FocusEvent);
      internals.fire(old, 'focusout', { bubbles: true, relatedTarget: el }, uiEvents.FocusEvent);
    }
    if (el !== null) {
      internals.fire(el, 'focus', { relatedTarget: old }, uiEvents.FocusEvent);
      internals.fire(el, 'focusin', { bubbles: true, relatedTarget: old }, uiEvents.FocusEvent);
    }
  }

  class HTMLElement extends Element {
    constructor(key) {
      illegal(key);
      super(key);
    }
    get dataset() {
      return cached(this, 'dataset', () => dataset(this));
    }
    get style() {
      return inlineStyle(this);
    }
    set style(value) {
      inlineStyle(this).cssText = value;
    }
    get innerText() {
      return N.innerText(this);
    }
    set innerText(value) {
      setInnerText(this, value);
    }
    get outerText() {
      return N.innerText(this);
    }
    set outerText(value) {
      const parent = N.nav(this, 0);
      if (parent === null) throw domException('NoModificationAllowedError', 'The element has no parent.');
      const fragment = N.createFragment();
      setInnerText(fragment, value);
      connected(N.replaceChild(parent, fragment, this));
    }
    get isContentEditable() {
      for (let el = this; el !== null; el = N.nav(el, 5)) {
        const v = N.getAttr(el, 'contenteditable');
        if (v !== null) return v === '' || v.toLowerCase() === 'true' || v.toLowerCase() === 'plaintext-only';
      }
      return false;
    }
    get contentEditable() {
      const v = N.getAttr(this, 'contenteditable');
      if (v === null) return 'inherit';
      const lower = v.toLowerCase();
      return lower === '' ? 'true' : (['true', 'false', 'plaintext-only'].includes(lower) ? lower : 'inherit');
    }
    set contentEditable(value) {
      const v = str(value).toLowerCase();
      if (v === 'inherit') N.removeAttr(this, 'contenteditable');
      else if (['true', 'false', 'plaintext-only'].includes(v)) N.setAttr(this, 'contenteditable', v);
      else throw domException('SyntaxError', "The value provided ('" + value + "') is not one of 'true', 'false', 'plaintext-only', or 'inherit'.");
    }
    get tabIndex() {
      const v = N.getAttr(this, 'tabindex');
      const n = v === null ? NaN : parseInt(v, 10);
      if (!Number.isNaN(n)) return n;
      return /^(a|area|button|input|select|textarea|iframe|summary)$/.test(N.elementName(this)[0]) ? 0 : -1;
    }
    set tabIndex(value) {
      N.setAttr(this, 'tabindex', String(value | 0));
    }
    get offsetParent() {
      const name = N.elementName(this)[0];
      if (name === 'body' || name === 'html' || box(this)[2] === 0) return null;
      return N.docPart(2);
    }
    get offsetTop() { return box(this)[1]; }
    get offsetLeft() { return box(this)[0]; }
    get offsetWidth() { return box(this)[2]; }
    get offsetHeight() { return box(this)[3]; }
    click() {
      if (state(this).clicking || this.disabled === true) return;
      state(this).clicking = true;
      try {
        const event = new uiEvents.MouseEvent('click', { bubbles: true, cancelable: true, composed: true, view: globalThis, detail: 1 });
        activate(this, event);
      } finally {
        state(this).clicking = false;
      }
    }
    focus() {
      if (N.isConnected(this)) setFocus(this);
    }
    blur() {
      if (focused === this) setFocus(null);
    }
    get autofocus() {
      return N.hasAttr(this, 'autofocus');
    }
    set autofocus(v) {
      if (v) N.setAttr(this, 'autofocus', '');
      else N.removeAttr(this, 'autofocus');
    }
    get nonce() {
      const v = N.getAttr(this, 'nonce');
      return v === null ? '' : v;
    }
    set nonce(v) {
      N.setAttr(this, 'nonce', str(v));
    }
    attachInternals() {
      throw domException('NotSupportedError', 'Custom element internals are not supported.');
    }
    showPopover() {}
    hidePopover() {}
    togglePopover() {
      return false;
    }
  }
  reflect(HTMLElement, {
    title: 'string', lang: 'string', translate: ['enum', 'translate', [['yes', 'no'], 'yes', 'yes']],
    dir: ['enum', 'dir', [['ltr', 'rtl', 'auto'], '', '']], hidden: 'bool', inert: 'bool',
    accessKey: ['string', 'accesskey'], draggable: ['enum', 'draggable', [['true'], 'false', 'false']],
    spellcheck: ['enum', 'spellcheck', [['false'], 'true', 'true']], autocapitalize: 'string',
    enterKeyHint: ['string', 'enterkeyhint'], inputMode: ['string', 'inputmode'], popover: 'nullable',
  });
  // draggable and spellcheck are booleans in the IDL.
  defineProperty(HTMLElement.prototype, 'draggable', {
    get() { const v = N.getAttr(this, 'draggable'); return v !== null ? v.toLowerCase() === 'true' : N.elementName(this)[0] === 'img' || (N.elementName(this)[0] === 'a' && N.hasAttr(this, 'href')); },
    set(v) { N.setAttr(this, 'draggable', v ? 'true' : 'false'); },
    enumerable: true, configurable: true,
  });
  defineProperty(HTMLElement.prototype, 'spellcheck', {
    get() { const v = N.getAttr(this, 'spellcheck'); return v === null ? true : v.toLowerCase() !== 'false'; },
    set(v) { N.setAttr(this, 'spellcheck', v ? 'true' : 'false'); },
    enumerable: true, configurable: true,
  });
  tag(HTMLElement, 'HTMLElement');

  function setInnerText(el, value) {
    const text = value === null || value === undefined ? '' : str(value);
    const fragment = N.createFragment();
    const lines = text.split(/\r\n|\r|\n/);
    lines.forEach((line, i) => {
      if (i > 0) N.insert(fragment, N.createElement('br'), null);
      if (line !== '') N.insert(fragment, N.createText(line), null);
    });
    connected(N.replaceAll(el, fragment));
  }

  // Click activation: checkboxes and radios toggle, submit buttons submit
  // their form, labels click their control.
  function activate(el, event) {
    const name = N.elementName(el)[0];
    let undo = null;
    if (name === 'input') {
      const type = el.type;
      if (type === 'checkbox') {
        const old = el.checked;
        el.checked = !old;
        undo = () => { el.checked = old; };
      } else if (type === 'radio') {
        const old = el.checked;
        el.checked = true;
        undo = () => { el.checked = old; };
      }
    }
    const notCanceled = el.dispatchEvent(event);
    if (!notCanceled) {
      if (undo !== null) undo();
      return;
    }
    if (name === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {
      internals.fire(el, 'input', { bubbles: true, composed: true });
      internals.fire(el, 'change', { bubbles: true });
    } else if ((name === 'button' && el.type === 'submit') || (name === 'input' && (el.type === 'submit' || el.type === 'image'))) {
      const form = el.form;
      if (form !== null) form.requestSubmit(el);
    } else if (name === 'label') {
      const control = el.control;
      if (control !== null && control !== el) control.click();
    } else if (name === 'summary') {
      const details = N.nav(el, 5);
      if (details !== null && N.elementName(details)[0] === 'details' && N.nav(details, 6) === el) {
        details.open = !details.open;
      }
    }
  }

  // GlobalEventHandlers on elements and the document.
  const GLOBAL_HANDLERS = [
    'onabort', 'onauxclick', 'onbeforeinput', 'onbeforematch', 'onbeforetoggle', 'onblur', 'oncancel',
    'oncanplay', 'oncanplaythrough', 'onchange', 'onclick', 'onclose', 'oncontextlost',
    'oncontextmenu', 'oncontextrestored', 'oncopy', 'oncuechange', 'oncut', 'ondblclick', 'ondrag',
    'ondragend', 'ondragenter', 'ondragleave', 'ondragover', 'ondragstart', 'ondrop',
    'ondurationchange', 'onemptied', 'onended', 'onerror', 'onfocus', 'onformdata', 'oninput',
    'oninvalid', 'onkeydown', 'onkeypress', 'onkeyup', 'onload', 'onloadeddata', 'onloadedmetadata',
    'onloadstart', 'onmousedown', 'onmouseenter', 'onmouseleave', 'onmousemove', 'onmouseout',
    'onmouseover', 'onmouseup', 'onpaste', 'onpause', 'onplay', 'onplaying', 'onprogress',
    'onratechange', 'onreset', 'onresize', 'onscroll', 'onscrollend', 'onsecuritypolicyviolation',
    'onseeked', 'onseeking', 'onselect', 'onslotchange', 'onstalled', 'onsubmit', 'onsuspend',
    'ontimeupdate', 'ontoggle', 'onvolumechange', 'onwaiting', 'onwheel', 'onanimationstart',
    'onanimationiteration', 'onanimationend', 'onanimationcancel', 'ontransitionrun',
    'ontransitionstart', 'ontransitionend', 'ontransitioncancel', 'onpointerover',
    'onpointerenter', 'onpointerdown', 'onpointermove', 'onpointerup', 'onpointercancel',
    'onpointerout', 'onpointerleave', 'ongotpointercapture', 'onlostpointercapture',
    'onselectstart', 'onselectionchange', 'ontouchstart', 'ontouchend', 'ontouchmove',
    'ontouchcancel',
  ];

  // ---------------------------------------------------------------------
  // HTML element interfaces.

  function htmlClass(name, Base = HTMLElement) {
    const C = class extends Base {};
    defineProperty(C, 'name', { value: name });
    tag(C, name);
    return C;
  }

  const I = {};
  for (const name of [
    'HTMLHtmlElement', 'HTMLHeadElement', 'HTMLTitleElement', 'HTMLBaseElement', 'HTMLLinkElement',
    'HTMLMetaElement', 'HTMLStyleElement', 'HTMLBodyElement', 'HTMLHeadingElement',
    'HTMLParagraphElement', 'HTMLHRElement', 'HTMLPreElement', 'HTMLQuoteElement', 'HTMLOListElement',
    'HTMLUListElement', 'HTMLMenuElement', 'HTMLLIElement', 'HTMLDListElement', 'HTMLDivElement',
    'HTMLAnchorElement', 'HTMLDataElement', 'HTMLTimeElement', 'HTMLSpanElement', 'HTMLBRElement',
    'HTMLModElement', 'HTMLPictureElement', 'HTMLSourceElement', 'HTMLImageElement',
    'HTMLIFrameElement', 'HTMLEmbedElement', 'HTMLObjectElement', 'HTMLParamElement',
    'HTMLMediaElement', 'HTMLTrackElement', 'HTMLMapElement', 'HTMLAreaElement', 'HTMLTableElement',
    'HTMLTableCaptionElement', 'HTMLTableColElement', 'HTMLTableSectionElement',
    'HTMLTableRowElement', 'HTMLTableCellElement', 'HTMLFormElement', 'HTMLLabelElement',
    'HTMLInputElement', 'HTMLButtonElement', 'HTMLSelectElement', 'HTMLDataListElement',
    'HTMLOptGroupElement', 'HTMLOptionElement', 'HTMLTextAreaElement', 'HTMLOutputElement',
    'HTMLProgressElement', 'HTMLMeterElement', 'HTMLFieldSetElement', 'HTMLLegendElement',
    'HTMLDetailsElement', 'HTMLDialogElement', 'HTMLScriptElement', 'HTMLTemplateElement',
    'HTMLSlotElement', 'HTMLCanvasElement', 'HTMLUnknownElement', 'HTMLFrameSetElement',
    'HTMLFrameElement', 'HTMLFontElement', 'HTMLDirectoryElement', 'HTMLMarqueeElement',
  ]) {
    I[name] = htmlClass(name, name === 'HTMLVideoElement' || name === 'HTMLAudioElement' ? I.HTMLMediaElement : HTMLElement);
  }
  I.HTMLVideoElement = htmlClass('HTMLVideoElement', I.HTMLMediaElement);
  I.HTMLAudioElement = htmlClass('HTMLAudioElement', I.HTMLMediaElement);

  const HTML_TAGS = {
    html: 'Html', head: 'Head', title: 'Title', base: 'Base', link: 'Link', meta: 'Meta', style: 'Style',
    body: 'Body', h1: 'Heading', h2: 'Heading', h3: 'Heading', h4: 'Heading', h5: 'Heading',
    h6: 'Heading', p: 'Paragraph', hr: 'HR', pre: 'Pre', listing: 'Pre', xmp: 'Pre',
    blockquote: 'Quote', q: 'Quote', ol: 'OList', ul: 'UList', menu: 'Menu', li: 'LI', dl: 'DList',
    div: 'Div', a: 'Anchor', data: 'Data', time: 'Time', span: 'Span', br: 'BR', ins: 'Mod',
    del: 'Mod', picture: 'Picture', source: 'Source', img: 'Image', iframe: 'IFrame',
    embed: 'Embed', object: 'Object', param: 'Param', video: 'Video', audio: 'Audio',
    track: 'Track', map: 'Map', area: 'Area', table: 'Table', caption: 'TableCaption',
    colgroup: 'TableCol', col: 'TableCol', tbody: 'TableSection', thead: 'TableSection',
    tfoot: 'TableSection', tr: 'TableRow', td: 'TableCell', th: 'TableCell', form: 'Form',
    label: 'Label', input: 'Input', button: 'Button', select: 'Select', datalist: 'DataList',
    optgroup: 'OptGroup', option: 'Option', textarea: 'TextArea', output: 'Output',
    progress: 'Progress', meter: 'Meter', fieldset: 'FieldSet', legend: 'Legend',
    details: 'Details', dialog: 'Dialog', script: 'Script', template: 'Template', slot: 'Slot',
    canvas: 'Canvas', frameset: 'FrameSet', frame: 'Frame', font: 'Font', dir: 'Directory',
    marquee: 'Marquee',
  };
  // Elements whose interface is HTMLElement itself.
  const PLAIN_TAGS = [
    'abbr', 'acronym', 'address', 'article', 'aside', 'b', 'basefont', 'bdi', 'bdo', 'big', 'center',
    'cite', 'code', 'dd', 'dfn', 'dt', 'em', 'figcaption', 'figure', 'footer', 'header', 'hgroup',
    'i', 'kbd', 'main', 'mark', 'nav', 'nobr', 'noembed', 'noframes', 'noscript', 'plaintext', 'rb',
    'rp', 'rt', 'rtc', 'ruby', 's', 'samp', 'search', 'section', 'small', 'strike', 'strong', 'sub',
    'summary', 'sup', 'tt', 'u', 'var', 'wbr',
  ];

  // Common reflected attributes.
  reflect(I.HTMLHtmlElement, { version: 'string' });
  reflect(I.HTMLBaseElement, { target: 'string' });
  define(I.HTMLBaseElement, {
    get href() {
      const v = N.getAttr(this, 'href');
      if (v === null) return internals.documentURL();
      const parts = internals.urlParse(v, internals.documentURL());
      return parts === undefined ? v : parts[internals.URL_PARTS.HREF];
    },
    set href(v) { N.setAttr(this, 'href', str(v)); },
  });
  reflect(I.HTMLLinkElement, {
    href: 'url', rel: 'string', as: 'string', media: 'string', hreflang: 'string', type: 'string',
    integrity: 'string', crossOrigin: ['nullable', 'crossorigin'], referrerPolicy: ['string', 'referrerpolicy'],
    disabled: 'bool', fetchPriority: ['string', 'fetchpriority'], imageSrcset: ['string', 'imagesrcset'],
    imageSizes: ['string', 'imagesizes'], charset: 'string',
  });
  define(I.HTMLLinkElement, {
    get relList() { return cached(this, 'relList', () => tokenList(this, 'rel', ['alternate', 'dns-prefetch', 'icon', 'manifest', 'modulepreload', 'next', 'preconnect', 'prefetch', 'preload', 'prerender', 'search', 'stylesheet'])); },
    get sizes() { return cached(this, 'sizes', () => tokenList(this, 'sizes')); },
    get sheet() { return styleSheetFor(this); },
  });
  reflect(I.HTMLMetaElement, { name: 'string', content: 'string', httpEquiv: ['string', 'http-equiv'], media: 'string', scheme: 'string' });
  reflect(I.HTMLStyleElement, { media: 'string', type: 'string', disabled: 'bool' });
  define(I.HTMLStyleElement, { get sheet() { return styleSheetFor(this); } });
  define(I.HTMLTitleElement, {
    get text() { return N.textContent(this); },
    set text(v) { N.setTextContent(this, str(v)); },
  });
  reflect(I.HTMLQuoteElement, { cite: 'url' });
  reflect(I.HTMLModElement, { cite: 'url', dateTime: ['string', 'datetime'] });
  reflect(I.HTMLOListElement, { reversed: 'bool', start: ['long', 'start', 1], type: 'string', compact: 'bool' });
  reflect(I.HTMLUListElement, { compact: 'bool', type: 'string' });
  reflect(I.HTMLLIElement, { value: 'long', type: 'string' });
  reflect(I.HTMLDivElement, { align: 'string' });
  reflect(I.HTMLParagraphElement, { align: 'string' });
  reflect(I.HTMLHeadingElement, { align: 'string' });
  reflect(I.HTMLDataElement, { value: 'string' });
  reflect(I.HTMLTimeElement, { dateTime: ['string', 'datetime'] });
  reflect(I.HTMLBRElement, { clear: 'string' });
  reflect(I.HTMLHRElement, { align: 'string', color: 'string', noShade: ['bool', 'noshade'], size: 'string', width: 'string' });
  reflect(I.HTMLPreElement, { width: 'long' });
  reflect(I.HTMLFontElement, { color: 'string', face: 'string', size: 'string' });

  // Hyperlinks: href and its URL parts.
  const HYPERLINK = {
    get href() {
      const v = N.getAttr(this, 'href');
      return v === null ? '' : resolveURL(v);
    },
    set href(v) { N.setAttr(this, 'href', str(v)); },
    toString() { return this.href; },
    get origin() { return urlPart(this, 'ORIGIN'); },
    get protocol() { return urlPart(this, 'PROTOCOL', ':'); },
    set protocol(v) { setUrlPart(this, 'protocol', v); },
    get username() { return urlPart(this, 'USERNAME'); },
    set username(v) { setUrlPart(this, 'username', v); },
    get password() { return urlPart(this, 'PASSWORD'); },
    set password(v) { setUrlPart(this, 'password', v); },
    get host() { return urlPart(this, 'HOST'); },
    set host(v) { setUrlPart(this, 'host', v); },
    get hostname() { return urlPart(this, 'HOSTNAME'); },
    set hostname(v) { setUrlPart(this, 'hostname', v); },
    get port() { return urlPart(this, 'PORT'); },
    set port(v) { setUrlPart(this, 'port', v); },
    get pathname() { return urlPart(this, 'PATHNAME'); },
    set pathname(v) { setUrlPart(this, 'pathname', v); },
    get search() { return urlPart(this, 'SEARCH'); },
    set search(v) { setUrlPart(this, 'search', v); },
    get hash() { return urlPart(this, 'HASH'); },
    set hash(v) { setUrlPart(this, 'hash', v); },
  };

  function urlParts(el) {
    const v = N.getAttr(el, 'href');
    if (v === null) return undefined;
    return internals.urlParse(v, internals.baseURL());
  }

  function urlPart(el, part, fallback = '') {
    const parts = urlParts(el);
    if (parts === undefined) return part === 'PROTOCOL' ? fallback : '';
    return parts[internals.URL_PARTS[part]];
  }

  function setUrlPart(el, part, value) {
    const parts = urlParts(el);
    if (parts === undefined) return;
    const next = internals.urlSet(parts[internals.URL_PARTS.HREF], part, str(value));
    if (next !== undefined) N.setAttr(el, 'href', next[internals.URL_PARTS.HREF]);
  }

  define(I.HTMLAnchorElement, HYPERLINK);
  define(I.HTMLAreaElement, HYPERLINK);
  const LINK_ATTRS = {
    target: 'string', download: 'string', ping: 'string', rel: 'string', hreflang: 'string',
    type: 'string', referrerPolicy: ['string', 'referrerpolicy'],
  };
  reflect(I.HTMLAnchorElement, { ...LINK_ATTRS, name: 'string', coords: 'string', shape: 'string', charset: 'string', rev: 'string' });
  reflect(I.HTMLAreaElement, { ...LINK_ATTRS, alt: 'string', coords: 'string', shape: 'string', noHref: ['bool', 'nohref'] });
  for (const C of [I.HTMLAnchorElement, I.HTMLAreaElement]) {
    define(C, {
      get relList() { return cached(this, 'relList', () => tokenList(this, 'rel')); },
    });
  }
  define(I.HTMLAnchorElement, {
    get text() { return N.textContent(this); },
    set text(v) { N.setTextContent(this, str(v)); },
  });
  define(I.HTMLMapElement, {
    get areas() { return cached(this, 'areas', () => htmlCollection(this, KIND.selector, 'area', null)); },
  });
  reflect(I.HTMLMapElement, { name: 'string' });

  // Embedded content.
  const dimension = (attr) => (el) => {
    const v = N.getAttr(el, attr);
    const n = v === null ? NaN : parseInt(v, 10);
    return Number.isNaN(n) ? 0 : n;
  };
  reflect(I.HTMLImageElement, {
    src: 'url', srcset: 'string', sizes: 'string', alt: 'string', useMap: ['string', 'usemap'],
    isMap: ['bool', 'ismap'], crossOrigin: ['nullable', 'crossorigin'],
    referrerPolicy: ['string', 'referrerpolicy'], decoding: ['enum', 'decoding', [['sync', 'async', 'auto'], 'auto', 'auto']],
    loading: ['enum', 'loading', [['lazy', 'eager'], 'eager', 'eager']], fetchPriority: ['string', 'fetchpriority'],
    width: ['ulong', 'width', 0], height: ['ulong', 'height', 0], name: 'string', align: 'string',
    border: 'string', hspace: 'ulong', vspace: 'ulong', longDesc: ['url', 'longdesc'], lowsrc: 'url',
  });
  define(I.HTMLImageElement, {
    get naturalWidth() { return dimension('width')(this); },
    get naturalHeight() { return dimension('height')(this); },
    get complete() { return true; },
    get currentSrc() { return this.src; },
    get x() { return box(this)[0]; },
    get y() { return box(this)[1]; },
    decode() { return Promise.resolve(); },
  });
  reflect(I.HTMLSourceElement, { src: 'url', type: 'string', srcset: 'string', sizes: 'string', media: 'string', width: 'ulong', height: 'ulong' });
  reflect(I.HTMLIFrameElement, {
    src: 'url', srcdoc: 'string', name: 'string', allow: 'string', allowFullscreen: ['bool', 'allowfullscreen'],
    width: 'string', height: 'string', referrerPolicy: ['string', 'referrerpolicy'],
    loading: ['enum', 'loading', [['lazy', 'eager'], 'eager', 'eager']], frameBorder: ['string', 'frameborder'],
    scrolling: 'string', align: 'string',
  });
  define(I.HTMLIFrameElement, {
    get contentWindow() { return null; },
    get contentDocument() { return null; },
    get sandbox() { return cached(this, 'sandbox', () => tokenList(this, 'sandbox')); },
  });
  reflect(I.HTMLEmbedElement, { src: 'url', type: 'string', width: 'string', height: 'string', name: 'string', align: 'string' });
  reflect(I.HTMLObjectElement, { data: 'url', type: 'string', name: 'string', useMap: ['string', 'usemap'], width: 'string', height: 'string' });
  define(I.HTMLObjectElement, {
    get contentDocument() { return null; },
    get contentWindow() { return null; },
    get form() { return formOwner(this); },
  });
  reflect(I.HTMLParamElement, { name: 'string', value: 'string', type: 'string', valueType: ['string', 'valuetype'] });
  reflect(I.HTMLTrackElement, { kind: 'string', src: 'url', srclang: 'string', label: 'string', default: 'bool' });
  reflect(I.HTMLMediaElement, {
    src: 'url', crossOrigin: ['nullable', 'crossorigin'], preload: 'string', autoplay: 'bool', loop: 'bool',
    controls: 'bool', defaultMuted: ['bool', 'muted'],
  });
  define(I.HTMLMediaElement, {
    get currentSrc() { return this.src; },
    get networkState() { return 0; },
    get readyState() { return 0; },
    get seeking() { return false; },
    get currentTime() { return state(this).currentTime || 0; },
    set currentTime(v) { state(this).currentTime = +v || 0; },
    get duration() { return NaN; },
    get paused() { return state(this).paused !== false; },
    get ended() { return false; },
    get muted() { const s = state(this); return s.muted === undefined ? N.hasAttr(this, 'muted') : s.muted; },
    set muted(v) { state(this).muted = !!v; },
    get volume() { const s = state(this); return s.volume === undefined ? 1 : s.volume; },
    set volume(v) { state(this).volume = +v; },
    get playbackRate() { return 1; },
    set playbackRate(v) { void v; },
    get defaultPlaybackRate() { return 1; },
    get buffered() { return { length: 0, start() { return 0; }, end() { return 0; } }; },
    get played() { return { length: 0, start() { return 0; }, end() { return 0; } }; },
    get seekable() { return { length: 0, start() { return 0; }, end() { return 0; } }; },
    get error() { return null; },
    get textTracks() { return []; },
    load() {},
    play() {
      state(this).paused = false;
      return Promise.resolve();
    },
    pause() { state(this).paused = true; },
    canPlayType() { return ''; },
    addTextTrack() { return {}; },
  });
  reflect(I.HTMLVideoElement, { poster: 'url', width: ['ulong', 'width', 0], height: ['ulong', 'height', 0], playsInline: ['bool', 'playsinline'] });
  define(I.HTMLVideoElement, {
    get videoWidth() { return 0; },
    get videoHeight() { return 0; },
    getVideoPlaybackQuality() { return { droppedVideoFrames: 0, totalVideoFrames: 0, creationTime: 0 }; },
    requestPictureInPicture() { return Promise.reject(domException('NotSupportedError', 'Picture-in-picture is not supported.')); },
  });
  reflect(I.HTMLCanvasElement, { width: ['ulong', 'width', 300], height: ['ulong', 'height', 150] });
  define(I.HTMLCanvasElement, {
    getContext() { return null; },
    toDataURL() { return 'data:,'; },
    toBlob(callback) { queueTask(() => callback(null)); },
    captureStream() { throw domException('NotSupportedError', 'Canvas capture is not supported.'); },
    transferControlToOffscreen() { throw domException('NotSupportedError', 'OffscreenCanvas is not supported.'); },
  });

  // Tables.
  function firstChildNamed(el, name) {
    for (let c = N.nav(el, 6); c !== null; c = N.nav(c, 9)) {
      if (N.elementName(c)[0] === name && N.elementName(c)[1] === HTML_NS) return c;
    }
    return null;
  }
  define(I.HTMLTableElement, {
    get caption() { return firstChildNamed(this, 'caption'); },
    get tHead() { return firstChildNamed(this, 'thead'); },
    get tFoot() { return firstChildNamed(this, 'tfoot'); },
    get tBodies() { return cached(this, 'tBodies', () => htmlCollection(this, KIND.selector, ':scope > tbody', null)); },
    get rows() {
      return cached(this, 'rows', () => htmlCollection(this, KIND.selector,
        ':scope > thead > tr, :scope > tbody > tr, :scope > tr, :scope > tfoot > tr', null));
    },
    createTHead() { return this.tHead || this.insertBefore(N.createElement('thead'), N.nav(this, 6)); },
    createTFoot() { return this.tFoot || this.appendChild(N.createElement('tfoot')); },
    createTBody() { return this.appendChild(N.createElement('tbody')); },
    createCaption() { return this.caption || this.insertBefore(N.createElement('caption'), N.nav(this, 1)); },
    deleteCaption() { const c = this.caption; if (c) c.remove(); },
    deleteTHead() { const c = this.tHead; if (c) c.remove(); },
    deleteTFoot() { const c = this.tFoot; if (c) c.remove(); },
    insertRow(index = -1) {
      const rows = this.rows;
      const row = N.createElement('tr');
      if (rows.length === 0) {
        let body = this.tBodies[this.tBodies.length - 1];
        if (!body) body = this.appendChild(N.createElement('tbody'));
        body.appendChild(row);
      } else if (index === -1 || index === rows.length) {
        const last = rows[rows.length - 1];
        last.parentNode.appendChild(row);
      } else {
        const at = rows[index];
        if (!at) throw domException('IndexSizeError', 'The index is out of range.');
        at.parentNode.insertBefore(row, at);
      }
      return row;
    },
    deleteRow(index) {
      const rows = this.rows;
      const row = index === -1 ? rows[rows.length - 1] : rows[index];
      if (row) row.remove();
    },
  });
  reflect(I.HTMLTableElement, { align: 'string', border: 'string', frame: 'string', rules: 'string', summary: 'string', width: 'string', bgColor: ['string', 'bgcolor'], cellPadding: ['string', 'cellpadding'], cellSpacing: ['string', 'cellspacing'] });
  function rowsOf(section) {
    return cached(section, 'rows', () => htmlCollection(section, KIND.selector, ':scope > tr', null));
  }
  define(I.HTMLTableSectionElement, {
    get rows() { return rowsOf(this); },
    insertRow(index = -1) {
      const rows = this.rows;
      const row = N.createElement('tr');
      if (index === -1 || index === rows.length) this.appendChild(row);
      else this.insertBefore(row, rows[index]);
      return row;
    },
    deleteRow(index) {
      const rows = this.rows;
      const row = index === -1 ? rows[rows.length - 1] : rows[index];
      if (row) row.remove();
    },
  });
  define(I.HTMLTableRowElement, {
    get cells() { return cached(this, 'cells', () => htmlCollection(this, KIND.selector, ':scope > td, :scope > th', null)); },
    get rowIndex() {
      for (let t = N.nav(this, 5); t !== null; t = N.nav(t, 5)) {
        if (N.elementName(t)[0] === 'table') return Array.prototype.indexOf.call(Array.from(t.rows), this);
      }
      return -1;
    },
    get sectionRowIndex() {
      const parent = N.nav(this, 5);
      return parent === null ? -1 : Array.from(rowsOf(parent)).indexOf(this);
    },
    insertCell(index = -1) {
      const cells = this.cells;
      const cell = N.createElement('td');
      if (index === -1 || index === cells.length) this.appendChild(cell);
      else this.insertBefore(cell, cells[index]);
      return cell;
    },
    deleteCell(index) {
      const cells = this.cells;
      const cell = index === -1 ? cells[cells.length - 1] : cells[index];
      if (cell) cell.remove();
    },
  });
  reflect(I.HTMLTableRowElement, { align: 'string', vAlign: ['string', 'valign'], bgColor: ['string', 'bgcolor'] });
  reflect(I.HTMLTableCellElement, {
    colSpan: ['ulong', 'colspan', 1], rowSpan: ['ulong', 'rowspan', 1], headers: 'string', scope: 'string',
    abbr: 'string', align: 'string', vAlign: ['string', 'valign'], width: 'string', height: 'string',
    noWrap: ['bool', 'nowrap'], bgColor: ['string', 'bgcolor'],
  });
  define(I.HTMLTableCellElement, {
    get cellIndex() {
      const row = N.nav(this, 5);
      return row === null || N.elementName(row)[0] !== 'tr' ? -1 : Array.from(row.cells).indexOf(this);
    },
  });
  reflect(I.HTMLTableColElement, { span: ['ulong', 'span', 1], align: 'string', width: 'string' });
  reflect(I.HTMLTableCaptionElement, { align: 'string' });

  // Interactive and scripting.
  reflect(I.HTMLDetailsElement, { open: 'bool', name: 'string' });
  reflect(I.HTMLDialogElement, { open: 'bool' });
  define(I.HTMLDialogElement, {
    get returnValue() { return state(this).returnValue || ''; },
    set returnValue(v) { state(this).returnValue = str(v); },
    show() { N.setAttr(this, 'open', ''); },
    showModal() { N.setAttr(this, 'open', ''); },
    close(returnValue) {
      if (!N.hasAttr(this, 'open')) return;
      if (returnValue !== undefined) state(this).returnValue = str(returnValue);
      N.removeAttr(this, 'open');
      queueTask(() => internals.fire(this, 'close'));
    },
    requestClose(returnValue) { this.close(returnValue); },
  });
  reflect(I.HTMLScriptElement, {
    src: 'url', type: 'string', noModule: ['bool', 'nomodule'], defer: 'bool',
    crossOrigin: ['nullable', 'crossorigin'], integrity: 'string', referrerPolicy: ['string', 'referrerpolicy'],
    charset: 'string', event: 'string', htmlFor: ['string', 'for'], fetchPriority: ['string', 'fetchpriority'],
  });
  define(I.HTMLScriptElement, {
    // Scripts made by script are async unless told otherwise.
    get async() {
      return state(this).forceAsync === true || N.hasAttr(this, 'async');
    },
    set async(v) {
      state(this).forceAsync = false;
      if (v) N.setAttr(this, 'async', '');
      else N.removeAttr(this, 'async');
    },
    get text() {
      let out = '';
      for (let c = N.nav(this, 1); c !== null; c = N.nav(c, 4)) if (N.nodeType(c) === 3) out += N.data(c);
      return out;
    },
    set text(v) { N.setTextContent(this, str(v)); },
  });
  defineProperty(I.HTMLScriptElement, 'supports', {
    value: function supports(type) { return type === 'classic' || type === 'module'; },
    writable: true, configurable: true,
  });
  define(I.HTMLTemplateElement, {
    get content() { return N.templateContent(this); },
  });
  reflect(I.HTMLTemplateElement, { shadowRootMode: ['string', 'shadowrootmode'] });
  reflect(I.HTMLSlotElement, { name: 'string' });
  define(I.HTMLSlotElement, {
    assignedNodes() { return []; },
    assignedElements() { return []; },
    assign() {},
  });

  // Forms.
  const LISTED = 'button, fieldset, input, object, output, select, textarea';

  function formOwner(el) {
    const id = N.getAttr(el, 'form');
    if (id !== null) {
      const f = N.isConnected(el) ? N.byId(documentNode, id) : null;
      return f !== null && N.elementName(f)[0] === 'form' ? f : null;
    }
    return N.closest(el, 'form');
  }

  function labelsOf(el) {
    const id = N.getAttr(el, 'id');
    const out = [];
    for (const label of N.queryAll(documentNode, 'label')) if (label.control === el) out.push(label);
    void id;
    return staticNodeList(out);
  }

  const VALIDITY = Object.freeze({
    valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false,
    rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false,
    customError: false, valid: true,
  });

  const FORM_CONTROL = {
    get form() { return formOwner(this); },
    get labels() { return labelsOf(this); },
    get validity() { return VALIDITY; },
    get validationMessage() { return ''; },
    get willValidate() { return !N.hasAttr(this, 'disabled'); },
    checkValidity() { return true; },
    reportValidity() { return true; },
    setCustomValidity(message) { void message; },
  };

  define(I.HTMLFormElement, {
    get elements() {
      return cached(this, 'elements', () => htmlCollection(this, KIND.selector, LISTED, null, HTMLFormControlsCollection.prototype));
    },
    get length() { return this.elements.length; },
    get relList() { return cached(this, 'relList', () => tokenList(this, 'rel')); },
    get action() {
      const v = N.getAttr(this, 'action');
      return v === null || v === '' ? internals.documentURL() : resolveURL(v);
    },
    set action(v) { N.setAttr(this, 'action', str(v)); },
    submit() {
      const hook = internals.submitForm;
      if (typeof hook === 'function') hook(this, null);
    },
    requestSubmit(submitter = null) {
      const SubmitEvent = uiEvents.SubmitEvent;
      if (internals.fire(this, 'submit', { bubbles: true, cancelable: true, submitter }, SubmitEvent)) {
        const hook = internals.submitForm;
        if (typeof hook === 'function') hook(this, submitter);
      }
    },
    reset() {
      if (!internals.fire(this, 'reset', { bubbles: true, cancelable: true })) return;
      for (const el of N.queryAll(this, 'input, select, textarea, option')) {
        const s = state(el);
        delete s.value;
        delete s.checked;
        delete s.selected;
      }
    },
    checkValidity() { return true; },
    reportValidity() { return true; },
  });
  reflect(I.HTMLFormElement, {
    acceptCharset: ['string', 'accept-charset'], autocomplete: ['enum', 'autocomplete', [['on', 'off'], 'on', 'on']],
    enctype: ['enum', 'enctype', [['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain'], 'application/x-www-form-urlencoded', 'application/x-www-form-urlencoded']],
    method: ['enum', 'method', [['get', 'post', 'dialog'], 'get', 'get']], name: 'string',
    noValidate: ['bool', 'novalidate'], target: 'string', rel: 'string',
  });
  defineProperty(I.HTMLFormElement.prototype, 'encoding', Object.getOwnPropertyDescriptor(I.HTMLFormElement.prototype, 'enctype'));

  define(I.HTMLLabelElement, {
    get control() {
      const id = N.getAttr(this, 'for');
      if (id !== null) {
        const el = N.isConnected(this) ? N.byId(documentNode, id) : null;
        return el !== null && isLabelable(el) ? el : null;
      }
      for (const el of N.queryAll(this, 'button, input, meter, output, progress, select, textarea')) {
        if (isLabelable(el)) return el;
      }
      return null;
    },
    get form() { return formOwner(this); },
  });
  reflect(I.HTMLLabelElement, { htmlFor: ['string', 'for'] });

  function isLabelable(el) {
    const name = N.elementName(el);
    if (name[1] !== HTML_NS) return false;
    if (name[0] === 'input') return (N.getAttr(el, 'type') || '').toLowerCase() !== 'hidden';
    return ['button', 'meter', 'output', 'progress', 'select', 'textarea'].includes(name[0]);
  }

  const INPUT_TYPES = [
    'hidden', 'text', 'search', 'tel', 'url', 'email', 'password', 'date', 'month', 'week', 'time',
    'datetime-local', 'number', 'range', 'color', 'checkbox', 'radio', 'file', 'submit', 'image',
    'reset', 'button',
  ];

  function inputType(el) {
    const v = N.getAttr(el, 'type');
    if (v === null) return 'text';
    const lower = v.toLowerCase();
    return INPUT_TYPES.includes(lower) ? lower : 'text';
  }

  // The value modes of input types.
  function valueMode(type) {
    if (['hidden', 'submit', 'image', 'reset', 'button'].includes(type)) return 'default';
    if (type === 'checkbox' || type === 'radio') return 'default/on';
    if (type === 'file') return 'filename';
    return 'value';
  }

  function sanitize(type, value) {
    if (['text', 'search', 'tel', 'password'].includes(type)) return value.replace(/[\r\n]/g, '');
    if (type === 'url' || type === 'email') return value.replace(/[\r\n]/g, '').trim();
    if (type === 'number') return value !== '' && Number.isFinite(+value) ? value : '';
    if (type === 'color') return /^#[0-9a-f]{6}$/i.test(value) ? value.toLowerCase() : '#000000';
    if (type === 'range') {
      const n = +value;
      return value !== '' && Number.isFinite(n) ? value : '50';
    }
    return value;
  }

  define(I.HTMLInputElement, FORM_CONTROL);
  define(I.HTMLInputElement, {
    get type() { return inputType(this); },
    set type(v) { N.setAttr(this, 'type', str(v)); },
    get value() {
      const type = inputType(this);
      const mode = valueMode(type);
      if (mode === 'value') {
        const s = state(this);
        if (s.value !== undefined) return s.value;
        const v = N.getAttr(this, 'value');
        return sanitize(type, v === null ? '' : v);
      }
      if (mode === 'filename') return '';
      const v = N.getAttr(this, 'value');
      return v === null ? (mode === 'default/on' ? 'on' : '') : v;
    },
    set value(v) {
      const type = inputType(this);
      const mode = valueMode(type);
      const value = v === null ? '' : str(v);
      if (mode === 'value') state(this).value = sanitize(type, value);
      else if (mode === 'filename') {
        if (value !== '') throw domException('InvalidStateError', 'This input element accepts a filename, which may only be programmatically set to the empty string.');
      } else N.setAttr(this, 'value', value);
    },
    get valueAsNumber() {
      const n = parseFloat(this.value);
      return Number.isFinite(n) ? n : NaN;
    },
    set valueAsNumber(v) { this.value = Number.isFinite(v) ? String(v) : ''; },
    get valueAsDate() {
      const d = new Date(this.value);
      return Number.isNaN(d.getTime()) ? null : d;
    },
    set valueAsDate(v) { this.value = v === null ? '' : v.toISOString().slice(0, 10); },
    get checked() {
      const s = state(this);
      return s.checked !== undefined ? s.checked : N.hasAttr(this, 'checked');
    },
    set checked(v) {
      state(this).checked = !!v;
      if (v && inputType(this) === 'radio') uncheckGroup(this);
    },
    get indeterminate() { return !!state(this).indeterminate; },
    set indeterminate(v) { state(this).indeterminate = !!v; },
    get files() { return null; },
    get list() {
      const id = N.getAttr(this, 'list');
      const el = id === null ? null : N.byId(documentNode, id);
      return el !== null && N.elementName(el)[0] === 'datalist' ? el : null;
    },
    get selectionStart() { return this.value.length; },
    set selectionStart(v) { void v; },
    get selectionEnd() { return this.value.length; },
    set selectionEnd(v) { void v; },
    get selectionDirection() { return 'none'; },
    select() {},
    setSelectionRange() {},
    setRangeText(text) { this.value = str(text); },
    stepUp(n = 1) { this.valueAsNumber = (this.valueAsNumber || 0) + n * (parseFloat(this.step) || 1); },
    stepDown(n = 1) { this.valueAsNumber = (this.valueAsNumber || 0) - n * (parseFloat(this.step) || 1); },
    showPicker() {},
  });
  reflect(I.HTMLInputElement, {
    accept: 'string', alt: 'string', autocomplete: 'string', defaultChecked: ['bool', 'checked'],
    defaultValue: ['string', 'value'], dirName: ['string', 'dirname'], disabled: 'bool',
    formAction: ['url', 'formaction'], formEnctype: ['string', 'formenctype'],
    formMethod: ['string', 'formmethod'], formNoValidate: ['bool', 'formnovalidate'],
    formTarget: ['string', 'formtarget'], height: 'ulong', max: 'string', maxLength: ['long', 'maxlength', -1],
    min: 'string', minLength: ['long', 'minlength', -1], multiple: 'bool', name: 'string',
    pattern: 'string', placeholder: 'string', readOnly: ['bool', 'readonly'], required: 'bool',
    size: ['ulong', 'size', 20], src: 'url', step: 'string', width: 'ulong', align: 'string',
    useMap: ['string', 'usemap'], popoverTargetAction: ['string', 'popovertargetaction'],
  });

  function uncheckGroup(input) {
    const name = N.getAttr(input, 'name');
    if (name === null || name === '') return;
    const form = formOwner(input);
    const root = form !== null ? form : N.rootNode(input);
    for (const other of N.queryAll(root, 'input')) {
      if (other !== input && inputType(other) === 'radio' && N.getAttr(other, 'name') === name &&
        formOwner(other) === form) {
        state(other).checked = false;
      }
    }
  }

  define(I.HTMLButtonElement, FORM_CONTROL);
  define(I.HTMLButtonElement, {
    get type() {
      const v = (N.getAttr(this, 'type') || '').toLowerCase();
      return v === 'reset' || v === 'button' ? v : 'submit';
    },
    set type(v) { N.setAttr(this, 'type', str(v)); },
  });
  reflect(I.HTMLButtonElement, {
    disabled: 'bool', formAction: ['url', 'formaction'], formEnctype: ['string', 'formenctype'],
    formMethod: ['string', 'formmethod'], formNoValidate: ['bool', 'formnovalidate'],
    formTarget: ['string', 'formtarget'], name: 'string', value: 'string',
    popoverTargetAction: ['string', 'popovertargetaction'],
  });

  // Selects and options.
  function optionsOf(select) {
    return N.queryAll(select, ':scope > option, :scope > optgroup > option');
  }

  function isSingleSelect(select) {
    if (N.hasAttr(select, 'multiple')) return false;
    const size = parseInt(N.getAttr(select, 'size') || '', 10);
    return !(size > 1);
  }

  function ownerSelect(option) {
    let p = N.nav(option, 5);
    if (p !== null && N.elementName(p)[0] === 'optgroup') p = N.nav(p, 5);
    return p !== null && N.elementName(p)[0] === 'select' ? p : null;
  }

  function explicitSelected(option) {
    const s = state(option);
    return s.selected !== undefined ? s.selected : N.hasAttr(option, 'selected');
  }

  // Which options of a select are selected, with a single-select's default
  // of its first enabled option.
  function selectedOptions(select) {
    const options = optionsOf(select);
    const chosen = options.filter(explicitSelected);
    if (isSingleSelect(select)) {
      if (chosen.length > 0) return [chosen[chosen.length - 1]];
      const first = options.find((o) => !N.hasAttr(o, 'disabled'));
      return first === undefined ? [] : [first];
    }
    return chosen;
  }

  function selectOption(select, option) {
    for (const o of optionsOf(select)) state(o).selected = o === option;
  }

  define(I.HTMLSelectElement, FORM_CONTROL);
  define(I.HTMLSelectElement, {
    get type() { return isSingleSelect(this) && !N.hasAttr(this, 'multiple') ? 'select-one' : 'select-multiple'; },
    get options() {
      return cached(this, 'options', () => htmlCollection(this, KIND.selector,
        ':scope > option, :scope > optgroup > option', null, HTMLOptionsCollection.prototype));
    },
    get length() { return optionsOf(this).length; },
    set length(n) {
      const options = optionsOf(this);
      n >>>= 0;
      for (let i = options.length - 1; i >= n; i--) options[i].remove();
      for (let i = options.length; i < n; i++) this.appendChild(N.createElement('option'));
    },
    item(index) { return optionsOf(this)[index >>> 0] || null; },
    namedItem(name) { return this.options.namedItem(name); },
    add(element, before = null) {
      if (typeof before === 'number') before = optionsOf(this)[before] || null;
      if (before === null) this.appendChild(element);
      else before.parentNode.insertBefore(element, before);
    },
    remove(...args) {
      if (args.length === 0) {
        N.remove(this);
        return;
      }
      const o = optionsOf(this)[args[0] >>> 0];
      if (o) o.remove();
    },
    get selectedOptions() { return staticNodeList(selectedOptions(this)); },
    get selectedIndex() {
      const sel = selectedOptions(this);
      return sel.length === 0 ? -1 : optionsOf(this).indexOf(sel[0]);
    },
    set selectedIndex(i) { selectOption(this, optionsOf(this)[i | 0] || null); },
    get value() {
      const sel = selectedOptions(this);
      return sel.length === 0 ? '' : sel[0].value;
    },
    set value(v) {
      v = str(v);
      selectOption(this, optionsOf(this).find((o) => o.value === v) || null);
    },
    showPicker() {},
  });
  reflect(I.HTMLSelectElement, { autocomplete: 'string', disabled: 'bool', multiple: 'bool', name: 'string', required: 'bool', size: ['ulong', 'size', 0] });
  defineProperty(HTMLOptionsCollection.prototype, 'selectedIndex', {
    get() { return mapSelect(this).selectedIndex; },
    set(v) { mapSelect(this).selectedIndex = v; },
    enumerable: true, configurable: true,
  });
  function mapSelect(collection) {
    return N.collItem(collection, 0) === null ? { selectedIndex: -1 } : ownerSelect(N.collItem(collection, 0)) || { selectedIndex: -1 };
  }
  for (const m of ['add', 'remove']) {
    defineProperty(HTMLOptionsCollection.prototype, m, {
      value(...args) {
        const first = N.collItem(this, 0);
        const select = first === null ? null : ownerSelect(first);
        if (select !== null) return select[m](...args);
      },
      writable: true, enumerable: true, configurable: true,
    });
  }

  define(I.HTMLOptionElement, {
    get value() {
      const v = N.getAttr(this, 'value');
      return v === null ? this.text : v;
    },
    set value(v) { N.setAttr(this, 'value', str(v)); },
    get text() { return N.textContent(this).replace(/[\t\n\f\r ]+/g, ' ').trim(); },
    set text(v) { N.setTextContent(this, str(v)); },
    get label() {
      const v = N.getAttr(this, 'label');
      return v === null ? this.text : v;
    },
    set label(v) { N.setAttr(this, 'label', str(v)); },
    get selected() {
      const select = ownerSelect(this);
      return select === null ? explicitSelected(this) : selectedOptions(select).includes(this);
    },
    set selected(v) {
      const select = ownerSelect(this);
      if (v && select !== null && isSingleSelect(select)) selectOption(select, this);
      else state(this).selected = !!v;
    },
    get index() {
      const select = ownerSelect(this);
      return select === null ? 0 : optionsOf(select).indexOf(this);
    },
    get form() {
      const select = ownerSelect(this);
      return select === null ? null : formOwner(select);
    },
  });
  reflect(I.HTMLOptionElement, { disabled: 'bool', defaultSelected: ['bool', 'selected'] });
  reflect(I.HTMLOptGroupElement, { disabled: 'bool', label: 'string' });
  define(I.HTMLDataListElement, {
    get options() { return cached(this, 'options', () => htmlCollection(this, KIND.selector, 'option', null)); },
  });

  define(I.HTMLTextAreaElement, FORM_CONTROL);
  define(I.HTMLTextAreaElement, {
    get type() { return 'textarea'; },
    get defaultValue() { return N.textContent(this); },
    set defaultValue(v) { N.setTextContent(this, str(v)); },
    get value() {
      const s = state(this);
      return s.value !== undefined ? s.value : N.textContent(this).replace(/\r\n?/g, '\n');
    },
    set value(v) { state(this).value = v === null ? '' : str(v).replace(/\r\n?/g, '\n'); },
    get textLength() { return this.value.length; },
    get selectionStart() { return this.value.length; },
    set selectionStart(v) { void v; },
    get selectionEnd() { return this.value.length; },
    set selectionEnd(v) { void v; },
    get selectionDirection() { return 'none'; },
    select() {},
    setSelectionRange() {},
    setRangeText(text) { this.value = str(text); },
  });
  reflect(I.HTMLTextAreaElement, {
    autocomplete: 'string', cols: ['ulong', 'cols', 20], dirName: ['string', 'dirname'], disabled: 'bool',
    maxLength: ['long', 'maxlength', -1], minLength: ['long', 'minlength', -1], name: 'string',
    placeholder: 'string', readOnly: ['bool', 'readonly'], required: 'bool', rows: ['ulong', 'rows', 2],
    wrap: 'string',
  });

  define(I.HTMLOutputElement, FORM_CONTROL);
  define(I.HTMLOutputElement, {
    get type() { return 'output'; },
    get value() { return N.textContent(this); },
    set value(v) { N.setTextContent(this, str(v)); },
    get defaultValue() { const s = state(this); return s.defaultValue !== undefined ? s.defaultValue : N.textContent(this); },
    set defaultValue(v) { state(this).defaultValue = str(v); },
    get htmlFor() { return cached(this, 'htmlFor', () => tokenList(this, 'for')); },
  });
  reflect(I.HTMLOutputElement, { name: 'string' });
  define(I.HTMLFieldSetElement, FORM_CONTROL);
  define(I.HTMLFieldSetElement, {
    get type() { return 'fieldset'; },
    get elements() {
      return cached(this, 'elements', () => htmlCollection(this, KIND.selector, LISTED, null, HTMLFormControlsCollection.prototype));
    },
  });
  reflect(I.HTMLFieldSetElement, { disabled: 'bool', name: 'string' });
  define(I.HTMLLegendElement, {
    get form() {
      const p = N.nav(this, 5);
      return p !== null && N.elementName(p)[0] === 'fieldset' ? formOwner(p) : null;
    },
  });
  reflect(I.HTMLLegendElement, { align: 'string' });
  define(I.HTMLProgressElement, {
    get labels() { return labelsOf(this); },
    get position() { return N.hasAttr(this, 'value') ? this.value / this.max : -1; },
  });
  reflect(I.HTMLProgressElement, { value: ['number', 'value', 0], max: ['number', 'max', 1] });
  define(I.HTMLMeterElement, { get labels() { return labelsOf(this); } });
  reflect(I.HTMLMeterElement, {
    value: ['number', 'value', 0], min: ['number', 'min', 0], max: ['number', 'max', 1],
    low: ['number', 'low', 0], high: ['number', 'high', 1], optimum: ['number', 'optimum', 0.5],
  });

  // Form control values survive cloning, as in browsers.
  internals.cloningSteps = (source, copy) => {
    const name = N.elementName(copy);
    if (name[1] !== HTML_NS) return;
    if (name[0] === 'input' || name[0] === 'textarea') {
      const from = state(source);
      const to = state(copy);
      if (from.value !== undefined) to.value = from.value;
      if (from.checked !== undefined) to.checked = from.checked;
    }
  };

  // The body's window-reflecting event handlers.
  const WINDOW_HANDLERS = [
    'onafterprint', 'onbeforeprint', 'onbeforeunload', 'onhashchange', 'onlanguagechange',
    'onmessage', 'onmessageerror', 'onoffline', 'ononline', 'onpagehide', 'onpageshow',
    'onpopstate', 'onrejectionhandled', 'onstorage', 'onunhandledrejection', 'onunload',
  ];
  for (const C of [I.HTMLBodyElement, I.HTMLFrameSetElement]) {
    for (const name of [...WINDOW_HANDLERS, 'onblur', 'onerror', 'onfocus', 'onload', 'onresize', 'onscroll']) {
      defineProperty(C.prototype, name, {
        get() { return globalThis[name]; },
        set(v) { globalThis[name] = v; },
        enumerable: true,
        configurable: true,
      });
    }
  }
  reflect(I.HTMLBodyElement, { text: 'string', link: 'string', vLink: ['string', 'vlink'], aLink: ['string', 'alink'], bgColor: ['string', 'bgcolor'], background: 'string' });

  // Image, Audio and Option constructors.
  function Image(width, height) {
    if (new.target === undefined) throw new TypeError("Failed to construct 'Image': Please use the 'new' operator.");
    const img = N.createElement('img');
    if (width !== undefined) N.setAttr(img, 'width', String(width >>> 0));
    if (height !== undefined) N.setAttr(img, 'height', String(height >>> 0));
    return img;
  }
  Image.prototype = I.HTMLImageElement.prototype;
  function Audio(src) {
    if (new.target === undefined) throw new TypeError("Failed to construct 'Audio': Please use the 'new' operator.");
    const audio = N.createElement('audio');
    N.setAttr(audio, 'preload', 'auto');
    if (src !== undefined) N.setAttr(audio, 'src', str(src));
    return audio;
  }
  Audio.prototype = I.HTMLAudioElement.prototype;
  function Option(text = '', value, defaultSelected = false, selected = false) {
    if (new.target === undefined) throw new TypeError("Failed to construct 'Option': Please use the 'new' operator.");
    const option = N.createElement('option');
    if (text !== '') N.insert(option, N.createText(str(text)), null);
    if (value !== undefined) N.setAttr(option, 'value', str(value));
    if (defaultSelected) N.setAttr(option, 'selected', '');
    if (selected) state(option).selected = true;
    return option;
  }
  Option.prototype = I.HTMLOptionElement.prototype;

  // ---------------------------------------------------------------------
  // SVG and MathML.

  class SVGElement extends Element {
    get dataset() {
      return cached(this, 'dataset', () => dataset(this));
    }
    get style() {
      return inlineStyle(this);
    }
    set style(value) {
      inlineStyle(this).cssText = value;
    }
    get className() {
      const el = this;
      return cached(this, 'svgClassName', () => ({
        get baseVal() { const v = N.getAttr(el, 'class'); return v === null ? '' : v; },
        set baseVal(v) { N.setAttr(el, 'class', str(v)); },
        get animVal() { const v = N.getAttr(el, 'class'); return v === null ? '' : v; },
      }));
    }
    set className(v) {
      N.setAttr(this, 'class', str(v));
    }
    get ownerSVGElement() {
      for (let p = N.nav(this, 5); p !== null; p = N.nav(p, 5)) {
        const name = N.elementName(p);
        if (name[1] === SVG_NS && name[0] === 'svg') return p;
      }
      return null;
    }
    get viewportElement() {
      return this.ownerSVGElement;
    }
    get tabIndex() {
      const n = parseInt(N.getAttr(this, 'tabindex') || '', 10);
      return Number.isNaN(n) ? -1 : n;
    }
    set tabIndex(v) {
      N.setAttr(this, 'tabindex', String(v | 0));
    }
    focus() {
      if (N.isConnected(this)) setFocus(this);
    }
    blur() {
      if (focused === this) setFocus(null);
    }
  }
  tag(SVGElement, 'SVGElement');

  class SVGGraphicsElement extends SVGElement {
    getBBox() {
      const b = box(this);
      return new DOMRect(0, 0, b[2], b[3]);
    }
    getCTM() {
      return null;
    }
    getScreenCTM() {
      return null;
    }
  }
  tag(SVGGraphicsElement, 'SVGGraphicsElement');

  class SVGSVGElement extends SVGGraphicsElement {
    createSVGPoint() {
      return { x: 0, y: 0, matrixTransform() { return this; } };
    }
    getElementById(id) {
      return N.query(this, '#' + CSS.escape(str(id)));
    }
    suspendRedraw() { return 0; }
    unsuspendRedraw() {}
    forceRedraw() {}
  }
  tag(SVGSVGElement, 'SVGSVGElement');

  const SVG_TAGS = {
    svg: SVGSVGElement,
  };
  for (const name of ['g', 'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
    'text', 'tspan', 'use', 'image', 'foreignObject', 'a', 'switch', 'textPath']) {
    if (!SVG_TAGS[name]) SVG_TAGS[name] = SVGGraphicsElement;
  }

  class MathMLElement extends Element {
    get dataset() {
      return cached(this, 'dataset', () => dataset(this));
    }
    get style() {
      return inlineStyle(this);
    }
  }
  tag(MathMLElement, 'MathMLElement');

  // ---------------------------------------------------------------------
  // Style sheets: enough for CSS-in-JS libraries that insert rules into a
  // <style> element's sheet. Rules are kept as text and never applied.

  class StyleSheetList {
    #sheets;
    constructor(key, sheets) {
      illegal(key);
      this.#sheets = sheets;
      sheets.forEach((s, i) => defineProperty(this, i, { value: s, enumerable: true }));
    }
    get length() { return this.#sheets.length; }
    item(i) { return this.#sheets[i >>> 0] || null; }
    [Symbol.iterator]() { return this.#sheets.values(); }
  }
  tag(StyleSheetList, 'StyleSheetList');

  class CSSRule {
    #text;
    constructor(key, text) {
      illegal(key);
      this.#text = text;
    }
    get cssText() { return this.#text; }
    get type() { return 1; }
    get selectorText() {
      const i = this.#text.indexOf('{');
      return i < 0 ? '' : this.#text.slice(0, i).trim();
    }
    get parentRule() { return null; }
    get parentStyleSheet() { return null; }
  }
  tag(CSSRule, 'CSSRule');

  class CSSStyleSheet {
    #owner;
    #rules = [];
    constructor(options) {
      void options;
      this.#owner = null;
    }
    static {
      CSSStyleSheet.forOwner = (owner) => {
        const sheet = new CSSStyleSheet();
        sheet.#owner = owner;
        return sheet;
      };
    }
    get ownerNode() { return this.#owner; }
    get href() { return this.#owner !== null && N.elementName(this.#owner)[0] === 'link' ? this.#owner.href : null; }
    get title() { return this.#owner === null ? null : N.getAttr(this.#owner, 'title'); }
    get media() { return []; }
    get type() { return 'text/css'; }
    get disabled() { return false; }
    get parentStyleSheet() { return null; }
    get ownerRule() { return null; }
    get cssRules() { return this.#rules.slice(); }
    get rules() { return this.cssRules; }
    insertRule(rule, index = 0) {
      index >>>= 0;
      if (index > this.#rules.length) throw domException('IndexSizeError', 'The index is out of range.');
      this.#rules.splice(index, 0, new CSSRule(KEY, str(rule)));
      return index;
    }
    addRule(selector, style, index = this.#rules.length) {
      this.insertRule(selector + ' { ' + style + ' }', index);
      return -1;
    }
    deleteRule(index) {
      index >>>= 0;
      if (index >= this.#rules.length) throw domException('IndexSizeError', 'The index is out of range.');
      this.#rules.splice(index, 1);
    }
    removeRule(index = 0) { this.deleteRule(index); }
    replace(text) { this.replaceSync(text); return Promise.resolve(this); }
    replaceSync(text) { this.#rules = [new CSSRule(KEY, str(text))]; }
  }
  const sheetForOwner = CSSStyleSheet.forOwner;
  delete CSSStyleSheet.forOwner;
  tag(CSSStyleSheet, 'CSSStyleSheet');

  function styleSheetFor(el) {
    const name = N.elementName(el)[0];
    if (!N.isConnected(el)) return null;
    if (name === 'link' && !/(^|\s)stylesheet(\s|$)/i.test(N.getAttr(el, 'rel') || '')) return null;
    return cached(el, 'sheet', () => sheetForOwner(el));
  }

  // ---------------------------------------------------------------------
  // getComputedStyle: the inline style over defaults by element type.

  const DISPLAY = {
    block: 'address article aside blockquote body center details dialog dd div dl dt fieldset figcaption figure footer form h1 h2 h3 h4 h5 h6 header hgroup hr html legend listing main menu nav ol p plaintext pre search section summary ul xmp',
    'list-item': 'li',
    table: 'table', 'table-row': 'tr', 'table-cell': 'td th', 'table-row-group': 'tbody',
    'table-header-group': 'thead', 'table-footer-group': 'tfoot', 'table-caption': 'caption',
    'table-column': 'col', 'table-column-group': 'colgroup', 'inline-block': 'button input select textarea img video canvas iframe meter progress',
    none: 'area base datalist head link meta noscript script style template title param source track',
  };
  const displayOf = new Map();
  for (const [display, names] of Object.entries(DISPLAY)) for (const n of names.split(' ')) displayOf.set(n, display);

  function getComputedStyle(element, pseudoElement) {
    void pseudoElement;
    if (!(element instanceof Element)) {
      throw new TypeError("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.");
    }
    const name = N.elementName(element);
    const b = box(element);
    const defaults = [
      ['display', name[1] === HTML_NS ? displayOf.get(name[0]) || 'inline' : 'inline'],
      ['visibility', 'visible'], ['opacity', '1'], ['position', 'static'], ['float', 'none'],
      ['overflow', 'visible'], ['overflow-x', 'visible'], ['overflow-y', 'visible'],
      ['width', b[2] + 'px'], ['height', b[3] + 'px'], ['top', 'auto'], ['left', 'auto'],
      ['right', 'auto'], ['bottom', 'auto'], ['z-index', 'auto'], ['font-size', '16px'],
      ['font-family', 'Arial, sans-serif'], ['font-weight', '400'], ['font-style', 'normal'],
      ['line-height', 'normal'], ['color', 'rgb(0, 0, 0)'], ['background-color', 'rgba(0, 0, 0, 0)'],
      ['background-image', 'none'], ['box-sizing', 'content-box'], ['transform', 'none'],
      ['pointer-events', 'auto'], ['cursor', 'auto'], ['white-space', 'normal'],
      ['text-align', 'start'], ['text-decoration', 'none'], ['direction', 'ltr'],
      ['content', 'normal'], ['clip', 'auto'], ['clip-path', 'none'], ['filter', 'none'],
      ['transition', 'all 0s ease 0s'], ['animation-name', 'none'], ['animation-duration', '0s'],
      ['transition-duration', '0s'], ['scroll-behavior', 'auto'],
    ];
    for (const side of ['top', 'right', 'bottom', 'left']) {
      defaults.push(['margin-' + side, '0px'], ['padding-' + side, '0px'], ['border-' + side + '-width', '0px']);
    }
    const decls = defaults.map(([n, v]) => [n, v, false]);
    const inline = N.getAttr(element, 'style');
    if (inline !== null) {
      for (const d of parseDeclarations(inline)) {
        const i = decls.findIndex((x) => x[0] === d[0]);
        if (i >= 0) decls[i] = [d[0], d[1], false];
        else decls.push([d[0], d[1], false]);
      }
    }
    // Hidden by an attribute or an ancestor: report display: none.
    if (!N.matches(element, ':visible')) {
      const visibility = decls.find((x) => x[0] === 'visibility');
      if (visibility[1] === 'visible') decls.find((x) => x[0] === 'display')[1] = 'none';
    }
    return makeStyle(null, decls, true);
  }

  // ---------------------------------------------------------------------
  // Traversal: TreeWalker and NodeIterator.

  const NodeFilter = {
    FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
    SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4,
    SHOW_CDATA_SECTION: 0x8, SHOW_ENTITY_REFERENCE: 0x10, SHOW_ENTITY: 0x20,
    SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80, SHOW_DOCUMENT: 0x100,
    SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400, SHOW_NOTATION: 0x800,
  };

  function filterNode(walker, node) {
    const show = walker.whatToShow;
    if (!((1 << (N.nodeType(node) - 1)) & show)) return NodeFilter.FILTER_SKIP;
    const filter = walker.filter;
    if (filter === null) return NodeFilter.FILTER_ACCEPT;
    const fn = typeof filter === 'function' ? filter : filter.acceptNode;
    return ReflectApply(fn, filter, [node]);
  }

  class TreeWalker {
    #current;
    constructor(key, root, whatToShow, filter) {
      illegal(key);
      defineProperty(this, 'root', { value: root, enumerable: true });
      defineProperty(this, 'whatToShow', { value: whatToShow >>> 0, enumerable: true });
      defineProperty(this, 'filter', { value: filter === undefined ? null : filter, enumerable: true });
      this.#current = root;
    }
    get currentNode() { return this.#current; }
    set currentNode(node) { this.#current = node; }
    parentNode() {
      let node = this.#current;
      while (node !== null && node !== this.root) {
        node = N.nav(node, 0);
        if (node !== null && filterNode(this, node) === NodeFilter.FILTER_ACCEPT) {
          this.#current = node;
          return node;
        }
      }
      return null;
    }
    #children(first) {
      let node = N.nav(this.#current, first ? 1 : 2);
      while (node !== null) {
        const r = filterNode(this, node);
        if (r === NodeFilter.FILTER_ACCEPT) {
          this.#current = node;
          return node;
        }
        if (r === NodeFilter.FILTER_SKIP) {
          const child = N.nav(node, first ? 1 : 2);
          if (child !== null) {
            node = child;
            continue;
          }
        }
        while (node !== null) {
          const sibling = N.nav(node, first ? 4 : 3);
          if (sibling !== null) {
            node = sibling;
            break;
          }
          const parent = N.nav(node, 0);
          if (parent === null || parent === this.root || parent === this.#current) return null;
          node = parent;
        }
      }
      return null;
    }
    firstChild() { return this.#children(true); }
    lastChild() { return this.#children(false); }
    #siblings(next) {
      let node = this.#current;
      if (node === this.root) return null;
      for (;;) {
        let sibling = N.nav(node, next ? 4 : 3);
        while (sibling !== null) {
          node = sibling;
          const r = filterNode(this, node);
          if (r === NodeFilter.FILTER_ACCEPT) {
            this.#current = node;
            return node;
          }
          sibling = N.nav(node, next ? 1 : 2);
          if (r === NodeFilter.FILTER_REJECT || sibling === null) sibling = N.nav(node, next ? 4 : 3);
        }
        node = N.nav(node, 0);
        if (node === null || node === this.root) return null;
        if (filterNode(this, node) === NodeFilter.FILTER_ACCEPT) return null;
      }
    }
    nextSibling() { return this.#siblings(true); }
    previousSibling() { return this.#siblings(false); }
    previousNode() {
      let node = this.#current;
      while (node !== this.root) {
        let sibling = N.nav(node, 3);
        while (sibling !== null) {
          node = sibling;
          let r = filterNode(this, node);
          while (r !== NodeFilter.FILTER_REJECT && N.nav(node, 1) !== null) {
            node = N.nav(node, 2);
            r = filterNode(this, node);
          }
          if (r === NodeFilter.FILTER_ACCEPT) {
            this.#current = node;
            return node;
          }
          sibling = N.nav(node, 3);
        }
        if (node === this.root || N.nav(node, 0) === null) return null;
        node = N.nav(node, 0);
        if (filterNode(this, node) === NodeFilter.FILTER_ACCEPT) {
          this.#current = node;
          return node;
        }
      }
      return null;
    }
    nextNode() {
      let node = this.#current;
      let r = NodeFilter.FILTER_ACCEPT;
      for (;;) {
        while (r !== NodeFilter.FILTER_REJECT && N.nav(node, 1) !== null) {
          node = N.nav(node, 1);
          r = filterNode(this, node);
          if (r === NodeFilter.FILTER_ACCEPT) {
            this.#current = node;
            return node;
          }
        }
        let following = null;
        for (let n = node; n !== null && n !== this.root; n = N.nav(n, 0)) {
          const s = N.nav(n, 4);
          if (s !== null) {
            following = s;
            break;
          }
        }
        if (following === null) return null;
        node = following;
        r = filterNode(this, node);
        if (r === NodeFilter.FILTER_ACCEPT) {
          this.#current = node;
          return node;
        }
      }
    }
  }
  tag(TreeWalker, 'TreeWalker');

  class NodeIterator {
    #walker;
    #pointerBefore = true;
    constructor(key, root, whatToShow, filter) {
      illegal(key);
      this.#walker = new TreeWalker(KEY, root, whatToShow, filter);
    }
    get root() { return this.#walker.root; }
    get whatToShow() { return this.#walker.whatToShow; }
    get filter() { return this.#walker.filter; }
    get referenceNode() { return this.#walker.currentNode; }
    get pointerBeforeReferenceNode() { return this.#pointerBefore; }
    nextNode() {
      const w = this.#walker;
      if (this.#pointerBefore) {
        this.#pointerBefore = false;
        if (filterNode(w, w.currentNode) === NodeFilter.FILTER_ACCEPT) return w.currentNode;
      }
      return w.nextNode();
    }
    previousNode() {
      const w = this.#walker;
      if (!this.#pointerBefore) {
        this.#pointerBefore = true;
        if (filterNode(w, w.currentNode) === NodeFilter.FILTER_ACCEPT) return w.currentNode;
      }
      return w.previousNode();
    }
    detach() {}
  }
  tag(NodeIterator, 'NodeIterator');

  // ---------------------------------------------------------------------
  // UI events.

  const uiEvents = {};
  function eventIfMissing(name, defaults, Base) {
    const existing = globalThis[name];
    uiEvents[name] = typeof existing === 'function' ? existing : eventClass(name, defaults, Base);
    return uiEvents[name];
  }
  const UIEvent = eventIfMissing('UIEvent', { view: null, detail: 0, which: 0 });
  UIEvent.prototype.initUIEvent = function initUIEvent(type, bubbles, cancelable) {
    this.initEvent(type, bubbles, cancelable);
  };
  const MODIFIERS = { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false };
  function getModifierState(key) {
    switch (key) {
      case 'Control': return this.ctrlKey;
      case 'Shift': return this.shiftKey;
      case 'Alt': return this.altKey;
      case 'Meta': return this.metaKey;
      default: return false;
    }
  }
  const MOUSE = {
    ...MODIFIERS, screenX: 0, screenY: 0, clientX: 0, clientY: 0, button: 0, buttons: 0,
    relatedTarget: null, movementX: 0, movementY: 0,
  };
  const MouseEvent = eventIfMissing('MouseEvent', MOUSE, UIEvent);
  for (const [name, source] of [['pageX', 'clientX'], ['pageY', 'clientY'], ['x', 'clientX'], ['y', 'clientY'], ['offsetX', 'clientX'], ['offsetY', 'clientY']]) {
    if (!(name in MouseEvent.prototype)) {
      defineProperty(MouseEvent.prototype, name, { get() { return this[source]; }, enumerable: true, configurable: true });
    }
  }
  MouseEvent.prototype.getModifierState = getModifierState;
  MouseEvent.prototype.initMouseEvent = function initMouseEvent(type, bubbles, cancelable) {
    this.initEvent(type, bubbles, cancelable);
  };
  eventIfMissing('PointerEvent', {
    pointerId: 0, width: 1, height: 1, pressure: 0, tangentialPressure: 0, tiltX: 0, tiltY: 0,
    twist: 0, altitudeAngle: 0, azimuthAngle: 0, pointerType: '', isPrimary: false,
  }, MouseEvent);
  eventIfMissing('WheelEvent', { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 }, MouseEvent);
  eventIfMissing('DragEvent', { dataTransfer: null }, MouseEvent);
  const KeyboardEvent = eventIfMissing('KeyboardEvent', {
    ...MODIFIERS, key: '', code: '', location: 0, repeat: false, isComposing: false, charCode: 0,
    keyCode: 0,
  }, UIEvent);
  KeyboardEvent.prototype.getModifierState = getModifierState;
  KeyboardEvent.prototype.initKeyboardEvent = function initKeyboardEvent(type, bubbles, cancelable) {
    this.initEvent(type, bubbles, cancelable);
  };
  eventIfMissing('FocusEvent', { relatedTarget: null }, UIEvent);
  eventIfMissing('InputEvent', { data: null, isComposing: false, inputType: '', dataTransfer: null }, UIEvent);
  eventIfMissing('CompositionEvent', { data: '' }, UIEvent);
  eventIfMissing('TouchEvent', { ...MODIFIERS, touches: [], targetTouches: [], changedTouches: [] }, UIEvent);
  eventIfMissing('SubmitEvent', { submitter: null });
  eventIfMissing('FormDataEvent', { formData: null });
  eventIfMissing('AnimationEvent', { animationName: '', elapsedTime: 0, pseudoElement: '' });
  eventIfMissing('TransitionEvent', { propertyName: '', elapsedTime: 0, pseudoElement: '' });
  eventIfMissing('ClipboardEvent', { clipboardData: null });
  eventIfMissing('ToggleEvent', { oldState: '', newState: '' });

  const CREATE_EVENT = {
    event: 'Event', events: 'Event', htmlevents: 'Event', svgevents: 'Event',
    customevent: 'CustomEvent', uievent: 'UIEvent', uievents: 'UIEvent', mouseevent: 'MouseEvent',
    mouseevents: 'MouseEvent', keyboardevent: 'KeyboardEvent', keyevents: 'KeyboardEvent',
    focusevent: 'FocusEvent', compositionevent: 'CompositionEvent', dragevent: 'DragEvent',
    messageevent: 'MessageEvent', storageevent: 'StorageEvent', hashchangeevent: 'HashChangeEvent',
    errorevent: 'ErrorEvent', progressevent: 'ProgressEvent', textevent: 'CompositionEvent',
    touchevent: 'TouchEvent', beforeunloadevent: 'Event', popstateevent: 'PopStateEvent',
    pagetransitionevent: 'PageTransitionEvent', wheelevent: 'WheelEvent', pointerevent: 'PointerEvent',
  };

  // ---------------------------------------------------------------------
  // Document.

  let readyState = 'complete';
  let currentScript = null;
  // What the page loader knows about the document (see setDocumentInfo).
  const documentInfo = { characterSet: 'UTF-8', contentType: 'text/html', referrer: '' };

  // A document's document element (0), head (1), body (2) or doctype (3).
  // Documents made by createHTMLDocument share the page's arena, as trees
  // of their own, so their parts are found here rather than natively.
  function docPart(doc, which) {
    if (doc === documentNode) return N.docPart(which);
    if (which === 3) {
      for (let n = N.nav(doc, 1); n !== null; n = N.nav(n, 4)) if (N.nodeType(n) === 10) return n;
      return null;
    }
    const root = N.nav(doc, 6);
    if (which === 0 || root === null) return root;
    const name = N.elementName(root);
    if (name[1] !== HTML_NS || name[0] !== 'html') return null;
    for (let el = N.nav(root, 6); el !== null; el = N.nav(el, 9)) {
      const [local, ns] = N.elementName(el);
      if (ns !== HTML_NS) continue;
      if (which === 1 ? local === 'head' : local === 'body' || local === 'frameset') return el;
    }
    return null;
  }

  class Document extends Node {
    constructor() {
      throw domException('NotSupportedError', 'dasher has a single document per page; new documents cannot be created.');
    }
    get implementation() {
      return implementation;
    }
    get URL() {
      return this === documentNode ? internals.documentURL() : 'about:blank';
    }
    get documentURI() {
      return this.URL;
    }
    get location() {
      return this === documentNode ? globalThis.location : null;
    }
    set location(v) {
      globalThis.location.href = v;
    }
    get domain() {
      return globalThis.location.hostname;
    }
    set domain(v) {
      void v;
    }
    get referrer() {
      return documentInfo.referrer;
    }
    get cookie() {
      return this === documentNode ? N.cookie() : '';
    }
    set cookie(v) {
      if (this === documentNode) N.setCookie(str(v));
    }
    get lastModified() {
      const d = new Date();
      const pad = (n) => String(n).padStart(2, '0');
      return pad(d.getMonth() + 1) + '/' + pad(d.getDate()) + '/' + d.getFullYear() + ' ' +
        pad(d.getHours()) + ':' + pad(d.getMinutes()) + ':' + pad(d.getSeconds());
    }
    get readyState() {
      return readyState;
    }
    get compatMode() {
      return N.compatMode();
    }
    get characterSet() {
      return documentInfo.characterSet;
    }
    get charset() {
      return documentInfo.characterSet;
    }
    get inputEncoding() {
      return documentInfo.characterSet;
    }
    get contentType() {
      return documentInfo.contentType;
    }
    get doctype() {
      return docPart(this, 3);
    }
    get documentElement() {
      return docPart(this, 0);
    }
    get head() {
      return docPart(this, 1);
    }
    get body() {
      return docPart(this, 2);
    }
    set body(value) {
      if (!(value instanceof I.HTMLBodyElement) && !(value instanceof I.HTMLFrameSetElement)) {
        throw domException('HierarchyRequestError', "Failed to set the 'body' property on 'Document': The new body element is of type '" + (value && value.tagName) + "'. It must be either a 'BODY' or 'FRAMESET' element.");
      }
      const old = docPart(this, 2);
      if (old === value) return;
      const root = docPart(this, 0);
      if (root === null) throw domException('HierarchyRequestError', 'There is no document element.');
      if (old !== null) connected(N.replaceChild(root, value, old));
      else connected(N.insert(root, value, null));
    }
    get title() {
      const root = docPart(this, 0);
      if (root !== null && N.elementName(root)[1] === SVG_NS) {
        const t = N.query(root, ':scope > title');
        return t === null ? '' : N.textContent(t);
      }
      if (this !== documentNode) {
        const t = N.query(this, 'title');
        return t === null ? '' : N.textContent(t).replace(/[\t\n\f\r ]+/g, ' ').trim();
      }
      return N.title();
    }
    set title(value) {
      let title = N.query(this, 'title');
      if (title === null) {
        const head = docPart(this, 1);
        if (head === null) return;
        title = N.createElement('title');
        N.insert(head, title, null);
      }
      N.setTextContent(title, str(value));
    }
    get dir() {
      const root = docPart(this, 0);
      return root === null ? '' : root.dir;
    }
    set dir(v) {
      const root = docPart(this, 0);
      if (root !== null) root.dir = v;
    }
    get defaultView() {
      return this === documentNode ? globalThis : null;
    }
    get activeElement() {
      if (focused !== null && N.isConnected(focused)) return focused;
      return docPart(this, 2) || docPart(this, 0);
    }
    hasFocus() {
      return true;
    }
    get visibilityState() {
      return 'visible';
    }
    get hidden() {
      return false;
    }
    get webkitVisibilityState() {
      return 'visible';
    }
    get webkitHidden() {
      return false;
    }
    get prerendering() {
      return false;
    }
    get wasDiscarded() {
      return false;
    }
    get fullscreenEnabled() {
      return false;
    }
    get fullscreenElement() {
      return null;
    }
    get fullscreen() {
      return false;
    }
    exitFullscreen() {
      return Promise.resolve();
    }
    get pointerLockElement() {
      return null;
    }
    exitPointerLock() {}
    get pictureInPictureEnabled() {
      return false;
    }
    get pictureInPictureElement() {
      return null;
    }
    get scrollingElement() {
      return docPart(this, 0);
    }
    get designMode() {
      return 'off';
    }
    set designMode(v) {
      void v;
    }
    execCommand() {
      return false;
    }
    queryCommandSupported() {
      return false;
    }
    queryCommandEnabled() {
      return false;
    }
    queryCommandState() {
      return false;
    }
    queryCommandValue() {
      return '';
    }
    get currentScript() {
      return currentScript;
    }
    get images() {
      return cached(this, 'images', () => htmlCollection(this, KIND.selector, 'img', null));
    }
    get embeds() {
      return cached(this, 'embeds', () => htmlCollection(this, KIND.selector, 'embed', null));
    }
    get plugins() {
      return this.embeds;
    }
    get links() {
      return cached(this, 'links', () => htmlCollection(this, KIND.selector, 'a[href], area[href]', null));
    }
    get forms() {
      return cached(this, 'forms', () => htmlCollection(this, KIND.selector, 'form', null));
    }
    get scripts() {
      return cached(this, 'scripts', () => htmlCollection(this, KIND.selector, 'script', null));
    }
    get anchors() {
      return cached(this, 'anchors', () => htmlCollection(this, KIND.selector, 'a[name]', null));
    }
    get all() {
      return cached(this, 'all', () => htmlCollection(this, KIND.tag, '*', null));
    }
    get styleSheets() {
      const sheets = [];
      for (const el of N.queryAll(this, 'style, link[rel~=stylesheet i]')) {
        const sheet = styleSheetFor(el);
        if (sheet !== null) sheets.push(sheet);
      }
      return new StyleSheetList(KEY, sheets);
    }
    get adoptedStyleSheets() {
      const s = state(this);
      return s.adoptedStyleSheets || (s.adoptedStyleSheets = []);
    }
    set adoptedStyleSheets(v) {
      state(this).adoptedStyleSheets = Array.from(v);
    }
    get fonts() {
      return fonts;
    }
    get timeline() {
      return { currentTime: performance.now() };
    }
    getElementById(elementId) {
      return N.byId(this, str(elementId));
    }
    getElementsByTagName(qualifiedName) {
      return htmlCollection(this, KIND.tag, str(qualifiedName), null);
    }
    getElementsByTagNameNS(namespace, localName) {
      return htmlCollection(this, KIND.tagNS, nullableStr(namespace), str(localName));
    }
    getElementsByClassName(classNames) {
      return htmlCollection(this, KIND.className, str(classNames), null);
    }
    getElementsByName(elementName) {
      return nodeList(this, KIND.name, str(elementName), null);
    }
    createElement(localName, options) {
      requireArgs(arguments, 1, "'createElement' on 'Document'");
      void options;
      return created(N.createElement(str(localName)));
    }
    createElementNS(namespace, qualifiedName, options) {
      requireArgs(arguments, 2, "'createElementNS' on 'Document'");
      void options;
      return created(N.createElementNS(nullableStr(namespace), str(qualifiedName)));
    }
    createDocumentFragment() {
      return N.createFragment();
    }
    createTextNode(data) {
      requireArgs(arguments, 1, "'createTextNode' on 'Document'");
      return N.createText(str(data));
    }
    createCDATASection() {
      throw domException('NotSupportedError', 'This operation is not supported for HTML documents.');
    }
    createComment(data) {
      requireArgs(arguments, 1, "'createComment' on 'Document'");
      return N.createComment(str(data));
    }
    createProcessingInstruction() {
      throw domException('NotSupportedError', 'Processing instructions are not supported.');
    }
    createAttribute(localName) {
      requireArgs(arguments, 1, "'createAttribute' on 'Document'");
      const name = str(localName).toLowerCase();
      if (!/^[^\s/>="']+$/.test(name)) {
        throw domException('InvalidCharacterError', "The localName provided ('" + localName + "') contains an invalid character.");
      }
      return Attr.create(null, name, null, '');
    }
    createAttributeNS(namespace, qualifiedName) {
      return Attr.create(null, str(qualifiedName), nullableStr(namespace) || null, '');
    }
    createEvent(interfaceName) {
      requireArgs(arguments, 1, "'createEvent' on 'Document'");
      const ctorName = CREATE_EVENT[str(interfaceName).toLowerCase()];
      const Ctor = ctorName === undefined ? undefined : (uiEvents[ctorName] || globalThis[ctorName]);
      if (typeof Ctor !== 'function') {
        throw domException('NotSupportedError', "The provided event type ('" + interfaceName + "') is invalid.");
      }
      return new Ctor('');
    }
    createRange() {
      throw domException('NotSupportedError', 'Ranges are not supported.');
    }
    createTreeWalker(root, whatToShow = 0xFFFFFFFF, filter = null) {
      return new TreeWalker(KEY, root, whatToShow, filter);
    }
    createNodeIterator(root, whatToShow = 0xFFFFFFFF, filter = null) {
      return new NodeIterator(KEY, root, whatToShow, filter);
    }
    importNode(node, deep = false) {
      if (node instanceof Document) throw domException('NotSupportedError', 'A document cannot be imported.');
      return cloneWithState(node, !!deep);
    }
    adoptNode(node) {
      if (node instanceof Document) throw domException('NotSupportedError', 'A document cannot be adopted.');
      if (node instanceof Attr) {
        const owner = node.ownerElement;
        if (owner !== null) owner.removeAttributeNode(node);
        return node;
      }
      N.remove(node);
      return node;
    }
    elementFromPoint() {
      return docPart(this, 2) || docPart(this, 0);
    }
    elementsFromPoint() {
      return [docPart(this, 2), docPart(this, 0)].filter((x) => x !== null);
    }
    caretRangeFromPoint() {
      return null;
    }
    caretPositionFromPoint() {
      return null;
    }
    getSelection() {
      return typeof globalThis.getSelection === 'function' ? globalThis.getSelection() : null;
    }
    open() {
      return this;
    }
    close() {}
    write(...text) {
      documentWrite(text.map((t) => str(t)).join(''));
    }
    writeln(...text) {
      documentWrite(text.map((t) => str(t)).join('') + '\n');
    }
    startViewTransition(callback) {
      const done = Promise.resolve().then(() => (typeof callback === 'function' ? callback() : undefined));
      return { finished: done, ready: done, updateCallbackDone: done, skipTransition() {} };
    }
    hasStorageAccess() {
      return Promise.resolve(true);
    }
    requestStorageAccess() {
      return Promise.resolve();
    }
  }
  mixin(Document, ParentNode);
  tag(Document, 'Document');

  // Scripts created by script are async unless told otherwise.
  function created(el) {
    if (el instanceof I.HTMLScriptElement) state(el).forceAsync = true;
    return el;
  }

  class HTMLDocument extends Document {}
  tag(HTMLDocument, 'HTMLDocument');
  class XMLDocument extends Document {}
  tag(XMLDocument, 'XMLDocument');

  // document.write(): the page loader takes the markup while the parser
  // is running (the hook returns true). Otherwise the markup goes after the
  // running script, or at the end of the body.
  function documentWrite(html) {
    const hook = internals.documentWrite;
    if (typeof hook === 'function' && hook(html)) return;
    const script = currentScript;
    if (script !== null && N.isConnected(script)) {
      N.insertAdjacentHTML(script, 'afterend', html);
      return;
    }
    const body = N.docPart(2);
    if (body !== null) N.insertAdjacentHTML(body, 'beforeend', html);
  }

  class DOMImplementation {
    constructor(key) {
      illegal(key);
    }
    hasFeature() {
      return true;
    }
    createDocumentType(qualifiedName, publicId, systemId) {
      return N.createDoctype(str(qualifiedName), str(publicId), str(systemId));
    }
    // A document with no browsing context, for parsing and building markup
    // off the page (jQuery parses HTML strings in one). It lives in the
    // page's arena as a tree of its own: its scripts never run, it has no
    // URL, location, cookies or window, and nodes moved between it and the
    // page need no adopting.
    createHTMLDocument(title = undefined) {
      const doc = N.createDocument();
      otherDocuments = true;
      N.insert(doc, N.createDoctype('html', '', ''), null);
      const html = N.createElement('html');
      N.insert(doc, html, null);
      const head = N.createElement('head');
      N.insert(html, head, null);
      if (title !== undefined) {
        const titleElement = N.createElement('title');
        N.insert(titleElement, N.createText(str(title)), null);
        N.insert(head, titleElement, null);
      }
      N.insert(html, N.createElement('body'), null);
      return doc;
    }
    createDocument() {
      throw domException('NotSupportedError', 'dasher has a single document per page; new documents cannot be created.');
    }
  }
  tag(DOMImplementation, 'DOMImplementation');
  const implementation = new DOMImplementation(KEY);

  // document.fonts: every font counts as loaded.
  class FontFaceSet extends EventTarget {
    constructor(key) {
      illegal(key);
      super();
    }
    get ready() { return Promise.resolve(this); }
    get status() { return 'loaded'; }
    get size() { return 0; }
    check() { return true; }
    load() { return Promise.resolve([]); }
    add() { return this; }
    delete() { return false; }
    clear() {}
    has() { return false; }
    forEach() {}
    [Symbol.iterator]() { return [][Symbol.iterator](); }
  }
  tag(FontFaceSet, 'FontFaceSet');
  const fonts = new FontFaceSet(KEY);

  internals.defineEventHandlers(HTMLElement.prototype, GLOBAL_HANDLERS);
  internals.defineEventHandlers(SVGElement.prototype, GLOBAL_HANDLERS);
  internals.defineEventHandlers(MathMLElement.prototype, GLOBAL_HANDLERS);
  internals.defineEventHandlers(Document.prototype, [
    ...GLOBAL_HANDLERS, 'onreadystatechange', 'onvisibilitychange', 'onfullscreenchange',
    'onfullscreenerror', 'onpointerlockchange', 'onpointerlockerror', 'onfreeze', 'onresume',
    'onprerenderingchange',
  ]);

  // ---------------------------------------------------------------------
  // Register the prototypes with Rust and create the document.

  const htmlProtos = {};
  for (const [tagName, short] of Object.entries(HTML_TAGS)) htmlProtos[tagName] = I['HTML' + short + 'Element'].prototype;
  for (const tagName of PLAIN_TAGS) htmlProtos[tagName] = HTMLElement.prototype;
  const svgProtos = {};
  for (const [tagName, C] of Object.entries(SVG_TAGS)) svgProtos[tagName] = C.prototype;

  N.init({
    Element: Element.prototype,
    HTMLElement: HTMLElement.prototype,
    HTMLUnknownElement: I.HTMLUnknownElement.prototype,
    SVGElement: SVGElement.prototype,
    MathMLElement: MathMLElement.prototype,
    Text: Text.prototype,
    Comment: Comment.prototype,
    DocumentType: DocumentType.prototype,
    DocumentFragment: DocumentFragment.prototype,
    Document: HTMLDocument.prototype,
    html: htmlProtos,
    svg: svgProtos,
  }, (name, message) => domException(name, message));
  documentNode = N.document();

  // ---------------------------------------------------------------------
  // Hooks for the other glue and the page loader.

  internals.eventParent = (target, event) => {
    if (!(target instanceof Node) || target instanceof Attr) return null;
    const parent = N.nav(target, 0);
    if (parent !== null) return parent;
    if (target === documentNode) return event.type === 'load' ? null : globalThis;
    // Composed events leave a shadow tree for its host.
    const shadow = shadowState.get(target);
    if (shadow !== undefined && event.composed) return shadow.host;
    return null;
  };
  internals.scrollTarget = () => documentNode;

  // The document base URL, honouring <base href>.
  const documentURLOf = internals.documentURL;
  const base = { version: -1, url: '', href: null };
  internals.baseURL = () => {
    const url = documentURLOf();
    const v = N.version();
    if (base.version !== v || base.url !== url) {
      base.version = v;
      base.url = url;
      const href = N.baseHref();
      const parts = href === null || href === undefined ? undefined : internals.urlParse(href, url);
      base.href = parts === undefined ? url : parts[internals.URL_PARTS.HREF];
    }
    return base.href;
  };

  // For IntersectionObserver.
  internals.geometry = {
    scrolled: SCROLLED,
    rendered,
    clientBox,
    isElement: (value) => value instanceof Element,
    contains: (a, b) => N.contains(a, b),
  };

  Object.assign(internals, {
    document: documentNode,
    isNode: (value) => value instanceof Node,
    staticNodeList,
    // Page lifecycle: document.readyState and document.currentScript.
    setReadyState(value) {
      if (readyState === value) return;
      readyState = value;
      internals.fire(documentNode, 'readystatechange');
    },
    // Sets document.currentScript, returning the previous value.
    setCurrentScript(script) {
      const previous = currentScript;
      currentScript = script;
      return previous;
    },
    // characterSet, contentType and referrer, from the response.
    setDocumentInfo(info) {
      Object.assign(documentInfo, info);
    },
    // Marks a script element as started, so it is not run again.
    markScriptStarted: N.markScriptStarted,
    scriptType,
    getComputedStyle,
    NodeFilter,
    uiEvents,
  });

  internals.expose({
    Node, NodeList, HTMLCollection, HTMLFormControlsCollection, HTMLOptionsCollection,
    CharacterData, Text, CDATASection, Comment, ProcessingInstruction, DocumentType,
    DocumentFragment, ShadowRoot, Attr, NamedNodeMap, DOMTokenList, DOMStringMap,
    CSSStyleDeclaration, CSSStyleSheet, CSSRule, StyleSheetList, Element, HTMLElement, SVGElement,
    SVGGraphicsElement, SVGSVGElement, MathMLElement, Document, HTMLDocument, XMLDocument,
    DOMImplementation, FontFaceSet, DOMRect, DOMRectReadOnly, DOMRectList, TreeWalker,
    NodeIterator, NodeFilter, Image, Audio, Option, getComputedStyle, ...I, ...uiEvents,
  });
  defineProperty(globalThis, 'document', { get: () => documentNode, enumerable: true, configurable: false });
})
