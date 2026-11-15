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
// Nothing is laid out. Geometry is the synthetic layout of dom/layout.rs,
// which makes every rendered element a box as wide as the viewport.
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

  function tag(ctor, name) {
    defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  const KEY = Symbol('dom');
  function illegal(key) {
    if (key !== KEY) throw new TypeError('Illegal constructor');
  }

  // The stand-in for a member Rust implements: N.init puts a native
  // function in its place on the prototype (see "Native members" in
  // dom.rs), so the stand-in only gives the property its place and
  // attributes. The list of such members is MEMBERS in dom/members.rs.
  function nativeMember() {
    throw new TypeError('Illegal invocation');
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
  // Collections. Live ones are native objects (dom/lists.rs) that answer
  // indexed (and, for HTMLCollection, named) properties themselves; their
  // `length`, `item` and `namedItem` are native members. Static NodeLists
  // are arrays with NodeList.prototype. As in browsers, the iteration
  // members are Array.prototype's, which work on both.

  function listItem(list, i) {
    if (ArrayIsArray(list)) {
      const v = list[i];
      return v === undefined ? null : v;
    }
    return N.collItem(list, i);
  }

  const KIND = { childNodes: 0, children: 1, tag: 2, tagNS: 3, className: 4, name: 5, selector: 6 };

  function nodeList(root, kind, a, b) {
    return N.collection(root, kind, a, b, NodeList.prototype, false);
  }

  function htmlCollection(root, kind, a, b, proto = HTMLCollection.prototype) {
    return N.collection(root, kind, a, b, proto, true);
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

  // Array.prototype's iteration members, in place of the stand-ins below.
  function arrayIteration(Ctor, keys) {
    for (const key of keys) {
      const desc = Object.getOwnPropertyDescriptor(Ctor.prototype, key);
      desc.value = Array.prototype[key === Symbol.iterator ? 'values' : key];
      defineProperty(Ctor.prototype, key, desc);
    }
  }

  class NodeList {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return nativeMember();
    }
    item(index) {
      return listItem(this, index >>> 0);
    }
    forEach() {}
    entries() {}
    keys() {}
    values() {}
    [Symbol.iterator]() {}
  }
  arrayIteration(NodeList, ['forEach', 'entries', 'keys', 'values', Symbol.iterator]);
  tag(NodeList, 'NodeList');

  class HTMLCollection {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return nativeMember();
    }
    item(index) {
      return nativeMember(index);
    }
    namedItem(name) {
      return nativeMember(name);
    }
    [Symbol.iterator]() {}
  }
  arrayIteration(HTMLCollection, [Symbol.iterator]);
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
      return nativeMember();
    }
    get nodeName() {
      return nativeMember();
    }
    get baseURI() {
      return internals.baseURL();
    }
    get isConnected() {
      // A shadow tree is connected when its host is.
      return nativeMember();
    }
    get ownerDocument() {
      return nativeMember();
    }
    getRootNode(options) {
      let root = N.rootNode(this);
      if (options !== null && typeof options === 'object' && options.composed) {
        for (let host = N.shadowHost(root); host !== null; host = N.shadowHost(root)) root = N.rootNode(host);
      }
      return root;
    }
    get parentNode() {
      return nativeMember();
    }
    get parentElement() {
      return nativeMember();
    }
    hasChildNodes() {
      return nativeMember();
    }
    get childNodes() {
      return nativeMember();
    }
    get firstChild() {
      return nativeMember();
    }
    get lastChild() {
      return nativeMember();
    }
    get previousSibling() {
      return nativeMember();
    }
    get nextSibling() {
      return nativeMember();
    }
    get nodeValue() {
      return nativeMember();
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
      return nativeMember(other);
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
  // `into`: the document the copy belongs to, if not the source's.
  function cloneWithState(node, deep, into) {
    const copy = N.clone(node, deep);
    const hook = internals.cloningSteps;
    if (typeof hook === 'function') hook(node, copy);
    if (into !== undefined) N.setOwner(copy, into);
    // Copies of custom elements are upgraded before the copy is returned,
    // in the page's document (the only one with custom elements).
    if (N.ownerDocument(copy) === documentNode) internals.upgradeCustomElementsIn(copy);
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
      return nativeMember();
    },
    get firstElementChild() {
      return nativeMember();
    },
    get lastElementChild() {
      return nativeMember();
    },
    get childElementCount() {
      return nativeMember();
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
      return nativeMember();
    },
    get nextElementSibling() {
      return nativeMember();
    },
  };

  // The mixin members the DOM marks [Unscopable], which `with` scopes (such
  // as inline event handlers' element scope) skip, so `remove()` in an
  // `onclick` attribute does not mean the element's.
  const UNSCOPABLE = new Map([
    [ParentNode, ['prepend', 'append', 'replaceChildren']],
    [ChildNode, ['before', 'after', 'replaceWith', 'remove']],
  ]);

  function mixin(target, source) {
    const proto = target.prototype;
    for (const key of Reflect.ownKeys(source)) {
      const desc = Object.getOwnPropertyDescriptor(source, key);
      desc.enumerable = true;
      defineProperty(proto, key, desc);
    }
    const unscopable = UNSCOPABLE.get(source);
    if (unscopable === undefined) return;
    if (!Object.prototype.hasOwnProperty.call(proto, Symbol.unscopables)) {
      defineProperty(proto, Symbol.unscopables, { value: Object.create(null), configurable: true });
    }
    for (const name of unscopable) proto[Symbol.unscopables][name] = true;
  }

  // ---------------------------------------------------------------------
  // CharacterData, Text, Comment, DocumentType, DocumentFragment.

  class CharacterData extends Node {
    get data() {
      return nativeMember();
    }
    set data(value) {
      N.setData(this, value === null ? '' : str(value));
    }
    get length() {
      return nativeMember();
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
      return assignedSlot(this);
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

  // Shadow roots. A shadow root is a document fragment in the arena tied to
  // its host, as template contents are to their template; its wrapper gets
  // ShadowRoot.prototype. Nothing is rendered, so shadow DOM matters to
  // script (components build their content in a shadow root and find it
  // again through `host.shadowRoot`) and to the host, which serializes
  // shadow trees into the page's HTML as declarative shadow DOM.
  //
  // Slots are assigned on demand: a slot's assigned nodes are worked out
  // from the host's children each time they are asked for.
  const SHADOW_HOSTS = new Set(['article', 'aside', 'blockquote', 'body', 'div', 'footer', 'h1', 'h2',
    'h3', 'h4', 'h5', 'h6', 'header', 'main', 'nav', 'p', 'section', 'span']);
  const isValidCustomElementName = N.validCustomElementName;
  // Replaced by the custom element registry.
  internals.customElementDefinition = () => undefined;

  // The shadow root's settings as bits (see N.shadowInit).
  function shadowBits(root) {
    const bits = N.shadowInit(root);
    if (bits < 0) throw new TypeError('Illegal invocation');
    return bits;
  }

  function getHTML(node, options) {
    let serializable = false;
    let roots = [];
    if (options !== null && options !== undefined) {
      if (typeof options !== 'object') throw new TypeError("Failed to execute 'getHTML': The provided value is not of type 'GetHTMLOptions'.");
      serializable = !!options.serializableShadowRoots;
      if (options.shadowRoots !== undefined) roots = Array.from(options.shadowRoots);
    }
    return N.getHTML(node, serializable, roots);
  }

  class ShadowRoot extends DocumentFragment {
    // eslint-disable-next-line constructor-super
    constructor() {
      throw new TypeError('Illegal constructor');
    }
    get mode() {
      return shadowBits(this) & 1 ? 'closed' : 'open';
    }
    get host() {
      shadowBits(this);
      return N.shadowHost(this);
    }
    get delegatesFocus() {
      return (shadowBits(this) & 2) !== 0;
    }
    get slotAssignment() {
      shadowBits(this);
      return state(this).slotAssignment || 'named';
    }
    get clonable() {
      return (shadowBits(this) & 4) !== 0;
    }
    get serializable() {
      return (shadowBits(this) & 8) !== 0;
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
    getHTML(options) {
      return getHTML(this, options);
    }
    get adoptedStyleSheets() {
      const s = state(this);
      return s.adoptedStyleSheets || (s.adoptedStyleSheets = []);
    }
    set adoptedStyleSheets(v) {
      state(this).adoptedStyleSheets = Array.from(v);
    }
    getAnimations() {
      return [];
    }
    elementFromPoint() {
      return null;
    }
    elementsFromPoint() {
      return [];
    }
    getSelection() {
      return globalThis.getSelection ? globalThis.getSelection() : null;
    }
  }
  tag(ShadowRoot, 'ShadowRoot');

  // Parses `html` as a template's contents would be, then moves the result
  // into the shadow root.
  function setShadowHTML(root, html) {
    shadowBits(root);
    const template = N.createElement('template');
    N.setInnerHTML(template, html);
    connected(N.replaceAll(root, template.content));
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
    const definition = internals.customElementDefinition(host);
    if (definition !== undefined && definition.disableShadow) {
      throw domException('NotSupportedError', what + 'attachShadow() is disabled by disabledFeatures static field.');
    }
    let root;
    try {
      root = N.attachShadow(host, mode === 'closed', !!init.delegatesFocus, !!init.clonable, !!init.serializable);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'NotSupportedError') {
        throw domException('NotSupportedError', what + 'Shadow root cannot be created on a host which already hosts a shadow tree.');
      }
      throw e;
    }
    if (init.slotAssignment === 'manual') state(root).slotAssignment = 'manual';
    return root;
  }

  // Slots. https://dom.spec.whatwg.org/#find-a-slot
  function isSlottable(node) {
    const type = N.nodeType(node);
    return type === 1 || type === 3;
  }

  // The slot `node` is assigned to, or null. `open` limits it to slots in
  // open shadow trees, as `assignedSlot` is.
  function findSlot(node, open) {
    if (!isSlottable(node)) return null;
    const parent = N.nav(node, 0);
    if (parent === null || N.nodeType(parent) !== 1) return null;
    const root = N.shadowRoot(parent);
    if (root === null || (open && (shadowBits(root) & 1))) return null;
    if (state(root).slotAssignment === 'manual') {
      const slot = state(node).manualSlot;
      return slot !== undefined && slot !== null && N.rootNode(slot) === root ? slot : null;
    }
    const name = N.nodeType(node) === 1 ? (N.getAttr(node, 'slot') || '') : '';
    for (const slot of N.queryAll(root, 'slot')) {
      const n = N.getAttr(slot, 'name');
      if ((n === null ? '' : n) === name) return slot;
    }
    return null;
  }

  function assignedSlot(node) {
    return findSlot(node, true);
  }

  // The nodes assigned to `slot`, following nested slots if `flatten`.
  function assignedNodes(slot, flatten) {
    const root = N.rootNode(slot);
    const host = N.shadowHost(root);
    const out = [];
    if (host !== null) {
      for (let c = N.nav(host, 1); c !== null; c = N.nav(c, 4)) {
        if (findSlot(c, false) === slot) out.push(c);
      }
    }
    if (!flatten) return out;
    const flat = [];
    const source = out.length > 0 || host === null ? out : [];
    if (out.length === 0) {
      // Fallback content.
      for (let c = N.nav(slot, 1); c !== null; c = N.nav(c, 4)) if (isSlottable(c)) source.push(c);
    }
    for (const n of source) {
      if (N.nodeType(n) === 1 && N.elementName(n)[0] === 'slot' && N.shadowHost(N.rootNode(n)) !== null) {
        for (const m of assignedNodes(n, true)) flat.push(m);
      } else {
        flat.push(n);
      }
    }
    return flat;
  }

  // ---------------------------------------------------------------------
  // Attributes: Attr and NamedNodeMap.

  // Attributes are not tree nodes in the arena; Attr objects are views of
  // an element's attribute, made on demand (so not identity-stable).
  class Attr extends Node {}
  function createAttr(owner, name, ns, value) {
    const attr = internals.createTarget(Attr.prototype);
    new AttrState(attr, owner, name, ns, value);
    return attr;
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
    get ownerDocument() { const o = attrState(this).owner; return o === null ? documentNode : o.ownerDocument; },
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
    cloneNode() { const a = attrState(this); return createAttr(null, a.name, a.ns, this.value); },
  };
  mixin(Attr, attrMembers);
  tag(Attr, 'Attr');

  function attrAt(el, i) {
    const a = N.attr(el, i);
    return a === null ? null : createAttr(el, a[0], a[1], a[4]);
  }

  function attrNamed(el, name) {
    const v = N.getAttr(el, name);
    if (v === null) return null;
    const lower = N.elementName(el)[1] === HTML_NS ? name.toLowerCase() : name;
    const n = N.attrCount(el);
    for (let i = 0; i < n; i++) {
      const a = N.attr(el, i);
      if (a[0] === lower) return createAttr(el, a[0], a[1], a[4]);
    }
    return null;
  }

  // `el.attributes` is a native object (dom/lists.rs), whose indexed and
  // named properties come from attrAt and attrNamed.
  const mapOwner = N.mapOwner;

  class NamedNodeMap {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return nativeMember();
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


  // ---------------------------------------------------------------------
  // DOMTokenList (classList, relList), CSSStyleDeclaration (el.style,
  // getComputedStyle) and DOMStringMap (dataset) are host objects made in
  // Rust (dom/host.rs), which answers their indexed and named properties;
  // their members are natives (see nativeMember() above). What stays here
  // only builds on the natives, which it calls as the prototypes had them
  // when N.init ran (see `natives` below), not as page script may change
  // them.

  const natives = {};

  class DOMTokenList {
    constructor(key) {
      illegal(key);
    }
    get length() {
      return nativeMember();
    }
    item(index) {
      return nativeMember();
    }
    contains(token) {
      return nativeMember();
    }
    add(...tokens) {
      return nativeMember();
    }
    remove(...tokens) {
      return nativeMember();
    }
    toggle(token, force) {
      return nativeMember();
    }
    replace(token, newToken) {
      return nativeMember();
    }
    supports(token) {
      return nativeMember();
    }
    get value() {
      return nativeMember();
    }
    set value(value) {
      nativeMember();
    }
    toString() {
      return nativeMember();
    }
    forEach(callback, thisArg) {
      const list = tokenArray(this);
      for (let i = 0; i < list.length; i++) ReflectApply(callback, thisArg, [list[i], i, this]);
    }
    entries() {
      return tokenArray(this).entries();
    }
    keys() {
      return tokenArray(this).keys();
    }
    values() {
      return tokenArray(this).values();
    }
    [Symbol.iterator]() {
      return tokenArray(this).values();
    }
  }
  tag(DOMTokenList, 'DOMTokenList');

  function tokenArray(list) {
    const n = ReflectApply(natives.tokensLength, list, []);
    const out = [];
    for (let i = 0; i < n; i++) out.push(ReflectApply(natives.tokensItem, list, [i]));
    return out;
  }

  function tokenList(el, attr, supported = null) {
    return N.tokenList(el, attr, supported === null ? undefined : supported);
  }

  class CSSStyleDeclaration {
    constructor(key) {
      illegal(key);
    }
    get cssText() {
      return nativeMember();
    }
    set cssText(value) {
      nativeMember();
    }
    get length() {
      return nativeMember();
    }
    item(index) {
      return nativeMember();
    }
    getPropertyValue(property) {
      return nativeMember();
    }
    getPropertyPriority(property) {
      return nativeMember();
    }
    setProperty(property, value, priority = '') {
      return nativeMember();
    }
    removeProperty(property) {
      return nativeMember();
    }
    get cssFloat() {
      return nativeMember();
    }
    set cssFloat(value) {
      nativeMember();
    }
    get parentRule() {
      return null;
    }
    *[Symbol.iterator]() {
      const n = ReflectApply(natives.styleLength, this, []);
      for (let i = 0; i < n; i++) yield ReflectApply(natives.styleItem, this, [i]);
    }
  }
  tag(CSSStyleDeclaration, 'CSSStyleDeclaration');

  class DOMStringMap {
    constructor(key) {
      illegal(key);
    }
  }
  tag(DOMStringMap, 'DOMStringMap');

  // el.style = text.
  function setStyleText(el, value) {
    ReflectApply(natives.setCssText, ReflectApply(natives.style, el, []), [value]);
  }

  // ---------------------------------------------------------------------
  // Geometry. Nothing is laid out (see Visibility in window.rs): every
  // rendered element is a box as wide as the viewport, hidden ones are
  // empty. Under 'all' each box is one 20px line at the top of the
  // viewport, so anything gated on visibility counts as visible and in
  // view. Under the default 'loaded' the same holds for what the document
  // held at the last reveal (the load event, then each auto-scroll), but
  // elements inserted since take a line each in a stack from the top
  // (N.loadedLines), pushing what follows them down and out of view once
  // they fill a viewport. Boxes stay put when scrolling under both. Under
  // 'viewport' the elements under <body> are stacked in document order, a
  // line each, every element spanning its descendants (N.layoutLines), and
  // boxes move as the page scrolls. Either way the root element is as tall
  // as the content, at least the viewport.
  //
  // The boxes are computed in Rust (DomState::element_box in
  // dom/layout.rs): N.elementBox(el) gives an element's [x, y, width,
  // height] in document coordinates, and clientWidth, offsetTop and the
  // like are native getters reading the same boxes.

  const LINE = 20;
  const SCROLLED = env.visibility === 'viewport';
  const LOADED = env.visibility === 'loaded';

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

  // The 'loaded' box for the [line, lines] pair at lines[i]: at least a
  // line tall, so elements that take no room in the stack are not empty.
  function loadedBox(lines, i) {
    return [0, Math.max(0, lines[i]) * LINE, env.width, Math.max(1, lines[i + 1]) * LINE];
  }
  if (SCROLLED || LOADED) {
    internals.scrollLimit = () => [0, Math.max(0, N.rootHeight() - env.height)];
  }

  function box(el) {
    return N.elementBox(el);
  }

  // An element's box relative to the viewport, as getBoundingClientRect()
  // reports it. Under 'all' and 'loaded' boxes stay put when scrolling.
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
      return nativeMember();
    }
    get prefix() {
      return N.elementName(this)[2];
    }
    get localName() {
      return nativeMember();
    }
    get tagName() {
      return nativeMember();
    }
    get id() {
      return nativeMember();
    }
    set id(value) {
      N.setAttr(this, 'id', str(value));
    }
    get className() {
      return nativeMember();
    }
    set className(value) {
      N.setAttr(this, 'class', str(value));
    }
    get classList() {
      return nativeMember();
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
    // [PutForwards=value], as for the other token lists below.
    set part(value) {
      N.setAttr(this, 'part', str(value));
    }
    get attributes() {
      return nativeMember();
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
        if (a[1] === namespace && a[3] === localName) return createAttr(this, a[0], a[1], a[4]);
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
      return nativeMember();
    }
    get assignedSlot() {
      return assignedSlot(this);
    }
    getHTML(options) {
      return getHTML(this, options);
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
      return nativeMember();
    }
    get clientHeight() {
      return nativeMember();
    }
    get scrollWidth() {
      return nativeMember();
    }
    get scrollHeight() {
      return nativeMember();
    }
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
    return internals.urlHref(url, internals.baseURL()) ?? url;
  }

  // The accessors reflect() defined, for N.init.
  const reflected = [];

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
      // N.init makes the accessor native if it is still this one then (see
      // "Reflected attributes" in dom/attrs.rs).
      reflected.push([Ctor.prototype, prop, type, attrName, extra, get]);
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

  // Custom element constructors reach here through super(); the registry
  // (custom_elements.js) makes or finds the element they construct.
  internals.constructHTMLElement = () => {
    throw new TypeError("Failed to construct 'HTMLElement': Illegal constructor");
  };
  internals.attachInternals = () => {
    throw domException('NotSupportedError', "Failed to execute 'attachInternals' on 'HTMLElement': Unable to attach ElementInternals to non-custom elements.");
  };
  internals.upgradeCustomElementsIn = () => {};
  internals.createElement = (name, is) => {
    void is;
    return N.createElement(name);
  };

  class HTMLElement extends Element {
    // eslint-disable-next-line constructor-super
    constructor(key) {
      if (key !== KEY) return internals.constructHTMLElement(new.target);
      super(key);
    }
    get dataset() {
      return nativeMember();
    }
    get style() {
      return nativeMember();
    }
    set style(value) {
      setStyleText(this, value);
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
    get offsetTop() {
      return nativeMember();
    }
    get offsetLeft() {
      return nativeMember();
    }
    get offsetWidth() {
      return nativeMember();
    }
    get offsetHeight() {
      return nativeMember();
    }
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
      return internals.attachInternals(this);
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
      return internals.urlHref(v, internals.documentURL()) ?? v;
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
    set relList(value) { N.setAttr(this, 'rel', str(value)); },
    get sizes() { return cached(this, 'sizes', () => tokenList(this, 'sizes')); },
    set sizes(value) { N.setAttr(this, 'sizes', str(value)); },
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
  // The rel keywords of a, area and form that change what a browser does
  // (HTML, "Link types"): what `relList.supports()` answers true for.
  const ANCHOR_REL = ['noopener', 'noreferrer', 'opener'];
  const LINK_ATTRS = {
    target: 'string', download: 'string', ping: 'string', rel: 'string', hreflang: 'string',
    type: 'string', referrerPolicy: ['string', 'referrerpolicy'],
  };
  reflect(I.HTMLAnchorElement, { ...LINK_ATTRS, name: 'string', coords: 'string', shape: 'string', charset: 'string', rev: 'string' });
  reflect(I.HTMLAreaElement, { ...LINK_ATTRS, alt: 'string', coords: 'string', shape: 'string', noHref: ['bool', 'nohref'] });
  for (const C of [I.HTMLAnchorElement, I.HTMLAreaElement]) {
    define(C, {
      get relList() { return cached(this, 'relList', () => tokenList(this, 'rel', ANCHOR_REL)); },
      set relList(value) { N.setAttr(this, 'rel', str(value)); },
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
    // No supported tokens, so `sandbox.supports()` throws, unlike Chrome's:
    // answering it lets Google Publisher Tag render ads in SafeFrames,
    // which cost timesofindia.com 3.2 CPU s and 70 MiB of peak memory for
    // the same text.
    get sandbox() { return cached(this, 'sandbox', () => tokenList(this, 'sandbox')); },
    set sandbox(value) { N.setAttr(this, 'sandbox', str(value)); },
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
  // The track lists of a media element: always empty, but event targets
  // as in browsers (media players listen for `change` on them).
  const trackList = (name) => {
    const C = {
      [name]: class extends EventTarget {
        constructor(key) {
          illegal(key);
          super();
        }
        get length() { return 0; }
        getTrackById() { return null; }
        [Symbol.iterator]() { return [][Symbol.iterator](); }
      },
    }[name];
    tag(C, name);
    internals.defineEventHandlers(C.prototype, ['onchange', 'onaddtrack', 'onremovetrack']);
    return C;
  };
  const TextTrackList = trackList('TextTrackList');
  const AudioTrackList = trackList('AudioTrackList');
  const VideoTrackList = trackList('VideoTrackList');
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
    get textTracks() { return cached(this, 'textTracks', () => new TextTrackList(KEY)); },
    get audioTracks() { return cached(this, 'audioTracks', () => new AudioTrackList(KEY)); },
    get videoTracks() { return cached(this, 'videoTracks', () => new VideoTrackList(KEY)); },
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
    getContext(type) {
      if (arguments.length === 0) {
        throw new TypeError("Failed to execute 'getContext' on 'HTMLCanvasElement': 1 argument required, but only 0 present.");
      }
      // A 2D context that draws nothing (stubs.js); no other kind.
      return internals.canvasContext(this, type);
    },
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
    assignedNodes(options) {
      return assignedNodes(this, options !== null && typeof options === 'object' && !!options.flatten);
    },
    assignedElements(options) {
      return this.assignedNodes(options).filter((n) => N.nodeType(n) === 1);
    },
    assign(...nodes) {
      for (const n of state(this).manualNodes || []) if (state(n).manualSlot === this) state(n).manualSlot = null;
      state(this).manualNodes = nodes;
      for (const n of nodes) state(n).manualSlot = this;
    },
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
    get relList() { return cached(this, 'relList', () => tokenList(this, 'rel', ANCHOR_REL)); },
    set relList(value) { N.setAttr(this, 'rel', str(value)); },
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
    // The selected files of a file input: always none, in one list per
    // element. Null for every other type.
    get files() {
      if (inputType(this) !== 'file') return null;
      return cached(this, 'files', () => internals.makeFileList([]));
    },
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
    set htmlFor(value) { N.setAttr(this, 'for', str(value)); },
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
  const BODY_WINDOW_HANDLERS = [...WINDOW_HANDLERS, 'onblur', 'onerror', 'onfocus', 'onload', 'onresize', 'onscroll'];
  for (const C of [I.HTMLBodyElement, I.HTMLFrameSetElement]) {
    for (const name of BODY_WINDOW_HANDLERS) {
      defineProperty(C.prototype, name, {
        get() { return globalThis[name]; },
        set(v) { globalThis[name] = v; },
        enumerable: true,
        configurable: true,
      });
    }
  }
  reflect(I.HTMLBodyElement, { text: 'string', link: 'string', vLink: ['string', 'vlink'], aLink: ['string', 'alink'], bgColor: ['string', 'bgcolor'], background: 'string' });

  // ---------------------------------------------------------------------
  // Event handler content attributes: `<button onclick="...">`,
  // `<body onload="...">`. events.js keeps the handlers; this decides which
  // attributes set one, and compiles them.
  // https://html.spec.whatwg.org/#event-handler-content-attributes

  const ELEMENT_HANDLER_TYPES = new Set(GLOBAL_HANDLERS.map((name) => name.slice(2)));
  const WINDOW_HANDLER_TYPES = new Set(BODY_WINDOW_HANDLERS.map((name) => name.slice(2)));

  function isBodyLike(el) {
    return el instanceof I.HTMLBodyElement || el instanceof I.HTMLFrameSetElement;
  }

  // What the handler that the content attribute `on<type>` of `el` sets
  // belongs to: the element, or the window for the body's window-reflecting
  // ones (none for a body of an extra document, which has no window). Null
  // if the attribute sets no handler.
  function handlerTarget(el, type) {
    if (WINDOW_HANDLER_TYPES.has(type) && isBodyLike(el)) {
      return el.ownerDocument === documentNode ? globalThis : null;
    }
    if (ELEMENT_HANDLER_TYPES.has(type) &&
        (el instanceof HTMLElement || el instanceof SVGElement || el instanceof MathMLElement)) {
      return el;
    }
    return null;
  }

  // Which content attributes set handlers, for the native lookup of an
  // attribute an element was made with (see src/events/dispatch.rs).
  internals.handlerTypes([...WINDOW_HANDLER_TYPES], [...ELEMENT_HANDLER_TYPES]);

  // Elements whose form owner is on a handler's scope chain.
  const FORM_ASSOCIATED = [
    I.HTMLButtonElement, I.HTMLFieldSetElement, I.HTMLInputElement, I.HTMLObjectElement,
    I.HTMLOutputElement, I.HTMLSelectElement, I.HTMLTextAreaElement,
  ];
  const EMPTY_SCOPE = Object.freeze(Object.create(null));

  // Compiles the body of the `on<type>` attribute of `el` into the
  // handler function, with the element, its form owner and the document on
  // its scope chain. Elements of documents other than the page's get none.
  // https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler
  internals.compileHandler = (el, type, source, forWindow) => {
    if (el.ownerDocument !== documentNode) return null;
    let form = null;
    if (FORM_ASSOCIATED.some((C) => el instanceof C)) form = el.form;
    const params = forWindow && type === 'error' ? 'event, source, lineno, colno, error' : 'event';
    // Sloppy code, as `with` needs; the scopes come in through `arguments`
    // so no extra name is visible to the body.
    const scoped = new Function(
      'with (arguments[0]) with (arguments[1]) with (arguments[2]) return function on' + type +
      '(' + params + ') {\n' + native.modules.rewrite(source) + '\n};');
    return scoped(documentNode, form === null ? EMPTY_SCOPE : form, el);
  };

  // Whether `name` starts with "on", ignoring ASCII case.
  function onPrefixed(name) {
    return name.length > 2 && (name.charCodeAt(0) | 32) === 111 && (name.charCodeAt(1) | 32) === 110;
  }

  // If setting or removing the attribute `name` (no namespace) of `el`
  // changes an event handler, returns [target, type] after picking up the
  // handler the attribute set before (so it keeps its place). `exact` is
  // for the namespace-aware methods, which do not lower-case the name.
  function handlerChange(el, name, exact) {
    if (!onPrefixed(name)) return null;
    if (!exact && N.elementName(el)[1] === HTML_NS) name = name.toLowerCase();
    if (!name.startsWith('on')) return null;
    const type = name.slice(2);
    const target = handlerTarget(el, type);
    if (target === null) return null;
    internals.existingHandler(target, type);
    return [target, type];
  }

  // Every attribute change goes through these natives, so wrapping them
  // catches every change to a handler attribute.
  {
    const { setAttr, removeAttr, setAttrNS, removeAttrNS } = N;
    const changed = internals.attributeHandlerChanged;
    N.setAttr = (el, name, value) => {
      const h = handlerChange(el, name, false);
      setAttr(el, name, value);
      if (h !== null) changed(h[0], h[1], el, value);
    };
    N.removeAttr = (el, name) => {
      const h = handlerChange(el, name, false);
      const removed = removeAttr(el, name);
      if (h !== null && removed) changed(h[0], h[1], el, null);
      return removed;
    };
    N.setAttrNS = (el, ns, name, value) => {
      const h = ns === null ? handlerChange(el, name, true) : null;
      setAttrNS(el, ns, name, value);
      if (h !== null) changed(h[0], h[1], el, value);
    };
    N.removeAttrNS = (el, ns, local) => {
      const h = ns === null ? handlerChange(el, local, true) : null;
      const removed = removeAttrNS(el, ns, local);
      if (h !== null && removed) changed(h[0], h[1], el, null);
      return removed;
    };
  }

  // The parser inserted the body: its window-reflecting handler attributes
  // (`<body onload>`) take over the window's handlers now, replacing any a
  // script set before, as if set at this point.
  internals.bodyInserted = (body) => {
    if (!isBodyLike(body)) return;
    const count = N.attrCount(body);
    for (let i = 0; i < count; i++) {
      const [name, ns, , , value] = N.attr(body, i);
      if (ns !== null || !name.startsWith('on')) continue;
      const type = name.slice(2);
      if (WINDOW_HANDLER_TYPES.has(type)) internals.attributeHandlerChanged(globalThis, type, body, value);
    }
  };

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
      return nativeMember();
    }
    get style() {
      return nativeMember();
    }
    set style(value) {
      setStyleText(this, value);
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

  // Scripts and styles in SVG are not graphics. Pages test nodes against
  // these by name (`node instanceof window[name]`), which throws when the
  // name is missing.
  class SVGScriptElement extends SVGElement {}
  tag(SVGScriptElement, 'SVGScriptElement');
  class SVGStyleElement extends SVGElement {}
  tag(SVGStyleElement, 'SVGStyleElement');

  const SVG_TAGS = {
    svg: SVGSVGElement,
    script: SVGScriptElement,
    style: SVGStyleElement,
  };
  for (const name of ['g', 'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
    'text', 'tspan', 'use', 'image', 'foreignObject', 'a', 'switch', 'textPath']) {
    if (!SVG_TAGS[name]) SVG_TAGS[name] = SVGGraphicsElement;
  }

  class MathMLElement extends Element {
    get dataset() {
      return nativeMember();
    }
    get style() {
      return nativeMember();
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

  // A style rule's declarations, for `rule.style`. Rules are kept as text
  // (nothing is cascaded), so this only parses and rewrites that text.
  class CSSRuleStyle {
    #props = new Map();
    #rule;
    constructor(key, rule, body) {
      illegal(key);
      this.#rule = rule;
      this.#parse(body);
    }
    #parse(body) {
      this.#props.clear();
      for (const part of String(body).split(';')) {
        const colon = part.indexOf(':');
        if (colon < 0) continue;
        const name = cssName(part.slice(0, colon));
        let value = part.slice(colon + 1).trim();
        let priority = '';
        const bang = /\s*!\s*important\s*$/i.exec(value);
        if (bang) {
          value = value.slice(0, bang.index);
          priority = 'important';
        }
        if (name && value) this.#props.set(name, [value, priority]);
      }
    }
    get cssText() {
      return [...this.#props].map(([n, [v, p]]) => n + ': ' + v + (p ? ' !important' : '') + ';').join(' ');
    }
    set cssText(text) { this.#parse(text); }
    get length() { return this.#props.size; }
    get parentRule() { return this.#rule; }
    item(index) { return [...this.#props.keys()][index >>> 0] ?? ''; }
    getPropertyValue(name) { return this.#props.get(cssName(name))?.[0] ?? ''; }
    getPropertyPriority(name) { return this.#props.get(cssName(name))?.[1] ?? ''; }
    setProperty(name, value, priority = '') {
      name = cssName(name);
      if (!name) return;
      value = value === null || value === undefined ? '' : String(value).trim();
      if (value === '') {
        this.#props.delete(name);
      } else {
        this.#props.set(name, [value, String(priority).toLowerCase() === 'important' ? 'important' : '']);
      }
    }
    removeProperty(name) {
      const old = this.getPropertyValue(name);
      this.#props.delete(cssName(name));
      return old;
    }
  }
  tag(CSSRuleStyle, 'CSSStyleDeclaration');
  // Custom properties keep their case; others are ASCII case-insensitive.
  const cssName = (name) => {
    const n = String(name).trim();
    return n.startsWith('--') ? n : n.toLowerCase();
  };

  class CSSRule {
    #text;
    #style = null;
    constructor(key, text) {
      illegal(key);
      this.#text = text;
    }
    get cssText() {
      return this.#style === null ? this.#text : this.selectorText + ' { ' + this.#style.cssText + ' }';
    }
    get type() { return 1; }
    get selectorText() {
      const i = this.#text.indexOf('{');
      return i < 0 ? '' : this.#text.slice(0, i).trim();
    }
    get style() {
      if (this.#style === null) {
        const open = this.#text.indexOf('{');
        const close = this.#text.lastIndexOf('}');
        const body = open < 0 ? '' : this.#text.slice(open + 1, close > open ? close : undefined);
        this.#style = new CSSRuleStyle(KEY, this, body);
      }
      return this.#style;
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
  // getComputedStyle is native (dom/host.rs): set after N.init below.

  let getComputedStyle;

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
  // Range. https://dom.spec.whatwg.org/#ranges
  //
  // A range here is not live: it does not follow changes other code makes
  // to the tree. Its own methods (deleteContents, insertNode, ...) leave
  // it where the specification says. Its rectangles are those of the
  // closest element, as there is no text layout to measure.

  // The data of a text or comment node, null for any other node.
  const charData = (node) => N.data(node);

  function nodeLength(node) {
    const data = charData(node);
    if (data !== null) return data.length;
    let n = 0;
    for (let child = N.nav(node, 1); child !== null; child = N.nav(child, 4)) n++;
    return n;
  }

  function nodeIndex(node) {
    let n = 0;
    for (let prev = N.nav(node, 3); prev !== null; prev = N.nav(prev, 3)) n++;
    return n;
  }

  function childAt(node, index) {
    let child = N.nav(node, 1);
    for (let i = 0; i < index && child !== null; i++) child = N.nav(child, 4);
    return child;
  }

  // The child of `ancestor` that `node` is in, or is.
  function childToward(ancestor, node) {
    for (let parent = N.nav(node, 0); parent !== ancestor; parent = N.nav(node, 0)) node = parent;
    return node;
  }

  // The node after `node` in tree order that is not one of its descendants.
  function nodeAfter(node) {
    for (; node !== null; node = N.nav(node, 0)) {
      const next = N.nav(node, 4);
      if (next !== null) return next;
    }
    return null;
  }

  // The position of boundary point A relative to B, both in one tree:
  // -1 before, 0 equal, 1 after.
  function comparePoints(nodeA, offsetA, nodeB, offsetB) {
    if (nodeA === nodeB) return offsetA === offsetB ? 0 : offsetA < offsetB ? -1 : 1;
    const position = N.compare(nodeA, nodeB);
    // B comes before A in tree order (it may be A's ancestor).
    if (position & 2) return -comparePoints(nodeB, offsetB, nodeA, offsetA);
    // B is inside A: compare with the child of A that holds it.
    if (position & 16 && nodeIndex(childToward(nodeA, nodeB)) < offsetA) return 1;
    return -1;
  }

  function commonAncestor(a, b) {
    while (!N.contains(a, b)) a = N.nav(a, 0);
    return a;
  }

  const RANGE_DELETE = 0;
  const RANGE_EXTRACT = 1;
  const RANGE_CLONE = 2;

  // Deletes, extracts or clones what lies between two boundary points.
  // Returns a fragment holding it, or null when deleting.
  function rangeContents(sc, so, ec, eo, mode) {
    const fragment = mode === RANGE_DELETE ? null : N.createFragment();
    const keep = (node) => N.insert(fragment, node, null);
    // Takes data[from, to) of a text or comment node.
    const takeData = (node, from, to) => {
      const data = charData(node);
      if (fragment !== null) {
        const copy = cloneWithState(node, false);
        N.setData(copy, data.slice(from, to));
        keep(copy);
      }
      if (mode !== RANGE_CLONE) N.setData(node, data.slice(0, from) + data.slice(to));
    };
    // Takes the part of `child` between the two points, inside a copy of it.
    const takePart = (child, startNode, startOffset, endNode, endOffset) => {
      const inner = rangeContents(startNode, startOffset, endNode, endOffset, mode);
      if (fragment === null) return;
      const copy = cloneWithState(child, false);
      keep(copy);
      N.insert(copy, inner, null);
    };

    if (sc === ec && so === eo) return fragment;
    if (sc === ec && charData(sc) !== null) {
      takeData(sc, so, eo);
      return fragment;
    }
    const common = commonAncestor(sc, ec);
    // The children of `common` that the range covers in part (the ones
    // holding its start and its end) and in full (the ones between).
    const first = sc === common ? null : childToward(common, sc);
    const last = ec === common ? null : childToward(common, ec);
    const stop = last === null ? childAt(common, eo) : last;
    const contained = [];
    for (let n = first === null ? childAt(common, so) : N.nav(first, 4); n !== null && n !== stop; n = N.nav(n, 4)) {
      if (fragment !== null && N.nodeType(n) === 10) {
        throw domException('HierarchyRequestError', 'A doctype cannot be put in a document fragment.');
      }
      contained.push(n);
    }
    if (first !== null) {
      if (charData(first) !== null) takeData(first, so, Infinity);
      else takePart(first, sc, so, first, nodeLength(first));
    }
    for (const node of contained) {
      if (mode === RANGE_DELETE) N.remove(node);
      else keep(mode === RANGE_CLONE ? cloneWithState(node, true) : node);
    }
    if (last !== null) {
      if (charData(last) !== null) takeData(last, 0, eo);
      else takePart(last, last, 0, ec, eo);
    }
    return fragment;
  }

  let placeRange;

  class Range {
    #sc = documentNode;
    #so = 0;
    #ec = documentNode;
    #eo = 0;
    static {
      placeRange = (range, sc, so, ec, eo) => {
        range.#sc = sc;
        range.#so = so;
        range.#ec = ec;
        range.#eo = eo;
        return range;
      };
    }
    get startContainer() { return this.#sc; }
    get startOffset() { return this.#so; }
    get endContainer() { return this.#ec; }
    get endOffset() { return this.#eo; }
    get collapsed() { return this.#sc === this.#ec && this.#so === this.#eo; }
    get commonAncestorContainer() { return commonAncestor(this.#sc, this.#ec); }

    #setBoundary(node, offset, start, what) {
      if (N.nodeType(node) === 10) throw domException('InvalidNodeTypeError', "Failed to execute '" + what + "' on 'Range': the node is a doctype.");
      offset >>>= 0;
      if (offset > nodeLength(node)) throw domException('IndexSizeError', "Failed to execute '" + what + "' on 'Range': the offset " + offset + ' is larger than the node\'s length.');
      const otherTree = N.rootNode(node) !== N.rootNode(this.#sc);
      if (start) {
        if (otherTree || comparePoints(node, offset, this.#ec, this.#eo) > 0) {
          this.#ec = node;
          this.#eo = offset;
        }
        this.#sc = node;
        this.#so = offset;
      } else {
        if (otherTree || comparePoints(node, offset, this.#sc, this.#so) < 0) {
          this.#sc = node;
          this.#so = offset;
        }
        this.#ec = node;
        this.#eo = offset;
      }
    }
    #setBeside(node, after, start, what) {
      const parent = N.nav(node, 0);
      if (parent === null) throw domException('InvalidNodeTypeError', "Failed to execute '" + what + "' on 'Range': the node has no parent.");
      this.#setBoundary(parent, nodeIndex(node) + (after ? 1 : 0), start, what);
    }
    setStart(node, offset) {
      requireArgs(arguments, 2, "'setStart' on 'Range'");
      this.#setBoundary(node, offset, true, 'setStart');
    }
    setEnd(node, offset) {
      requireArgs(arguments, 2, "'setEnd' on 'Range'");
      this.#setBoundary(node, offset, false, 'setEnd');
    }
    setStartBefore(node) {
      requireArgs(arguments, 1, "'setStartBefore' on 'Range'");
      this.#setBeside(node, false, true, 'setStartBefore');
    }
    setStartAfter(node) {
      requireArgs(arguments, 1, "'setStartAfter' on 'Range'");
      this.#setBeside(node, true, true, 'setStartAfter');
    }
    setEndBefore(node) {
      requireArgs(arguments, 1, "'setEndBefore' on 'Range'");
      this.#setBeside(node, false, false, 'setEndBefore');
    }
    setEndAfter(node) {
      requireArgs(arguments, 1, "'setEndAfter' on 'Range'");
      this.#setBeside(node, true, false, 'setEndAfter');
    }
    collapse(toStart = false) {
      if (toStart) placeRange(this, this.#sc, this.#so, this.#sc, this.#so);
      else placeRange(this, this.#ec, this.#eo, this.#ec, this.#eo);
    }
    selectNode(node) {
      requireArgs(arguments, 1, "'selectNode' on 'Range'");
      const parent = N.nav(node, 0);
      if (parent === null) throw domException('InvalidNodeTypeError', "Failed to execute 'selectNode' on 'Range': the node has no parent.");
      const index = nodeIndex(node);
      placeRange(this, parent, index, parent, index + 1);
    }
    selectNodeContents(node) {
      requireArgs(arguments, 1, "'selectNodeContents' on 'Range'");
      if (N.nodeType(node) === 10) throw domException('InvalidNodeTypeError', "Failed to execute 'selectNodeContents' on 'Range': the node is a doctype.");
      placeRange(this, node, 0, node, nodeLength(node));
    }
    compareBoundaryPoints(how, sourceRange) {
      requireArgs(arguments, 2, "'compareBoundaryPoints' on 'Range'");
      how = +how;
      if (how !== 0 && how !== 1 && how !== 2 && how !== 3) {
        throw domException('NotSupportedError', "Failed to execute 'compareBoundaryPoints' on 'Range': the comparison method is not one of the four constants.");
      }
      if (N.rootNode(this.#sc) !== N.rootNode(sourceRange.#sc)) {
        throw domException('WrongDocumentError', "Failed to execute 'compareBoundaryPoints' on 'Range': the ranges are in different trees.");
      }
      // START_TO_END compares this range's end with the other's start,
      // END_TO_START the reverse.
      const mine = how === 0 || how === 3;
      const theirs = how === 0 || how === 1;
      return comparePoints(
        mine ? this.#sc : this.#ec, mine ? this.#so : this.#eo,
        theirs ? sourceRange.#sc : sourceRange.#ec, theirs ? sourceRange.#so : sourceRange.#eo,
      );
    }
    cloneRange() {
      return placeRange(new Range(), this.#sc, this.#so, this.#ec, this.#eo);
    }
    detach() {}
    #checkPoint(node, offset, what) {
      if (N.nodeType(node) === 10) throw domException('InvalidNodeTypeError', "Failed to execute '" + what + "' on 'Range': the node is a doctype.");
      if (offset > nodeLength(node)) throw domException('IndexSizeError', "Failed to execute '" + what + "' on 'Range': the offset " + offset + ' is larger than the node\'s length.');
    }
    isPointInRange(node, offset) {
      requireArgs(arguments, 2, "'isPointInRange' on 'Range'");
      offset >>>= 0;
      if (N.rootNode(node) !== N.rootNode(this.#sc)) return false;
      this.#checkPoint(node, offset, 'isPointInRange');
      return comparePoints(node, offset, this.#sc, this.#so) >= 0 && comparePoints(node, offset, this.#ec, this.#eo) <= 0;
    }
    comparePoint(node, offset) {
      requireArgs(arguments, 2, "'comparePoint' on 'Range'");
      offset >>>= 0;
      if (N.rootNode(node) !== N.rootNode(this.#sc)) {
        throw domException('WrongDocumentError', "Failed to execute 'comparePoint' on 'Range': the node is in a different tree.");
      }
      this.#checkPoint(node, offset, 'comparePoint');
      if (comparePoints(node, offset, this.#sc, this.#so) < 0) return -1;
      return comparePoints(node, offset, this.#ec, this.#eo) > 0 ? 1 : 0;
    }
    intersectsNode(node) {
      requireArgs(arguments, 1, "'intersectsNode' on 'Range'");
      if (N.rootNode(node) !== N.rootNode(this.#sc)) return false;
      const parent = N.nav(node, 0);
      if (parent === null) return true;
      const index = nodeIndex(node);
      return comparePoints(parent, index, this.#ec, this.#eo) < 0 && comparePoints(parent, index + 1, this.#sc, this.#so) > 0;
    }

    // Where the range ends up once its contents are gone.
    #pointAfterRemoval() {
      if (N.contains(this.#sc, this.#ec)) return [this.#sc, this.#so];
      let node = this.#sc;
      let parent = N.nav(node, 0);
      while (!N.contains(parent, this.#ec)) {
        node = parent;
        parent = N.nav(node, 0);
      }
      return [parent, nodeIndex(node) + 1];
    }
    #remove(mode) {
      const [node, offset] = this.#pointAfterRemoval();
      const fragment = rangeContents(this.#sc, this.#so, this.#ec, this.#eo, mode);
      placeRange(this, node, offset, node, offset);
      return fragment;
    }
    deleteContents() {
      this.#remove(RANGE_DELETE);
    }
    extractContents() {
      return this.#remove(RANGE_EXTRACT);
    }
    cloneContents() {
      return rangeContents(this.#sc, this.#so, this.#ec, this.#eo, RANGE_CLONE);
    }
    // What a live range does when `count` nodes go into `parent` at `index`.
    #shift(parent, index, count) {
      if (this.#sc === parent && this.#so > index) this.#so += count;
      if (this.#ec === parent && this.#eo > index) this.#eo += count;
    }
    insertNode(node) {
      requireArgs(arguments, 1, "'insertNode' on 'Range'");
      const start = this.#sc;
      const type = N.nodeType(start);
      const isText = type === 3 || type === 4;
      if (type === 7 || type === 8 || (isText && N.nav(start, 0) === null) || start === node) {
        throw domException('HierarchyRequestError', "Failed to execute 'insertNode' on 'Range': a node cannot be inserted at the start of the range.");
      }
      const parent = isText ? N.nav(start, 0) : start;
      if (N.contains(node, parent)) {
        throw domException('HierarchyRequestError', "Failed to execute 'insertNode' on 'Range': the new node contains the insertion point.");
      }
      const collapsed = this.collapsed;
      let reference;
      if (isText) {
        // Split the text; the part of the range past the split moves to
        // the new text node.
        const index = nodeIndex(start);
        const offset = this.#so;
        const data = N.data(start);
        reference = N.createText(data.slice(offset));
        N.insert(parent, reference, N.nav(start, 4));
        N.setData(start, data.slice(0, offset));
        if (this.#ec === start && this.#eo > offset) {
          this.#ec = reference;
          this.#eo -= offset;
        }
        this.#shift(parent, index, 1);
      } else {
        reference = childAt(start, this.#so);
      }
      if (node === reference) reference = N.nav(reference, 4);
      if (N.nav(node, 0) !== null) N.remove(node);
      const index = reference === null ? nodeLength(parent) : nodeIndex(reference);
      const count = N.nodeType(node) === 11 ? nodeLength(node) : 1;
      connected(N.insert(parent, node, reference));
      this.#shift(parent, index, count);
      if (collapsed) {
        this.#ec = parent;
        this.#eo = index + count;
      }
    }
    surroundContents(newParent) {
      requireArgs(arguments, 1, "'surroundContents' on 'Range'");
      // A node other than text that the range covers only part of.
      const holder = (node) => {
        const type = N.nodeType(node);
        return type === 3 || type === 4 ? N.nav(node, 0) : node;
      };
      if (holder(this.#sc) !== holder(this.#ec)) {
        throw domException('InvalidStateError', "Failed to execute 'surroundContents' on 'Range': the range partially selects a node that is not text.");
      }
      const type = N.nodeType(newParent);
      if (type === 9 || type === 10 || type === 11) {
        throw domException('InvalidNodeTypeError', "Failed to execute 'surroundContents' on 'Range': the new parent is a document, doctype or fragment.");
      }
      const fragment = this.extractContents();
      while (N.nav(newParent, 1) !== null) N.remove(N.nav(newParent, 1));
      this.insertNode(newParent);
      connected(N.insert(newParent, fragment, null));
      this.selectNode(newParent);
    }

    // Parses markup as the children of the range's start element. Unlike
    // innerHTML, the scripts in the result run when it is inserted.
    createContextualFragment(string) {
      requireArgs(arguments, 1, "'createContextualFragment' on 'Range'");
      const start = this.#sc;
      const type = N.nodeType(start);
      let context = null;
      if (type === 1) context = start;
      else if (type === 3 || type === 4 || type === 8) context = N.nav(start, 5);
      return N.contextualFragment(context, str(string));
    }

    toString() {
      const sc = this.#sc;
      const ec = this.#ec;
      const isText = (node) => {
        const type = N.nodeType(node);
        return type === 3 || type === 4;
      };
      if (sc === ec && isText(sc)) return N.data(sc).slice(this.#so, this.#eo);
      let out = '';
      let node;
      if (isText(sc)) out += N.data(sc).slice(this.#so);
      if (charData(sc) !== null) node = nodeAfter(sc);
      else node = childAt(sc, this.#so) || nodeAfter(sc);
      const stop = charData(ec) !== null ? ec : childAt(ec, this.#eo) || nodeAfter(ec);
      while (node !== null && node !== stop) {
        if (isText(node)) out += N.data(node);
        node = N.nav(node, 1) || nodeAfter(node);
      }
      if (isText(ec)) out += N.data(ec).slice(0, this.#eo);
      return out;
    }

    #box() {
      if (this.collapsed) return [0, 0, 0, 0];
      const node = commonAncestor(this.#sc, this.#ec);
      const el = N.nodeType(node) === 1 ? node : N.nav(node, 5);
      return el === null ? [0, 0, 0, 0] : clientBox(el);
    }
    getBoundingClientRect() {
      const b = this.#box();
      return new DOMRect(b[0], b[1], b[2], b[3]);
    }
    getClientRects() {
      const b = this.#box();
      return new DOMRectList(KEY, b[2] === 0 && b[3] === 0 ? [] : [new DOMRect(b[0], b[1], b[2], b[3])]);
    }
  }
  for (const [name, value] of [['START_TO_START', 0], ['START_TO_END', 1], ['END_TO_END', 2], ['END_TO_START', 3]]) {
    defineProperty(Range, name, { value, enumerable: true });
    defineProperty(Range.prototype, name, { value, enumerable: true });
  }
  tag(Range, 'Range');

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

  // Documents script makes besides the page's live in the page's arena as
  // trees of their own: inert (no scripts run, no window, cookies or
  // loading) but otherwise ordinary. Their URL, content type and compat
  // mode are kept here.
  const extraDocuments = new WeakMap();
  const XML_TYPES = new Set(['text/xml', 'application/xml', 'application/xhtml+xml', 'image/svg+xml']);
  const PARSER_ERROR_NS = 'http://www.mozilla.org/newlayout/xml/parsererror.xml';

  function newDocument(contentType, proto, url) {
    const doc = N.createDocument();
    if (getPrototypeOf(doc) !== proto) setPrototypeOf(doc, proto);
    extraDocuments.set(doc, { contentType, url, compatMode: 'CSS1Compat', written: undefined });
    return doc;
  }

  // What is known about an extra document. One whose wrapper was remade
  // (after the document became unreachable) has forgotten it.
  function extraInfo(doc) {
    let info = extraDocuments.get(doc);
    if (info === undefined) {
      info = { contentType: 'text/html', url: 'about:blank', compatMode: 'CSS1Compat', written: undefined };
      extraDocuments.set(doc, info);
    }
    return info;
  }

  // A document parsed from `text`, as DOMParser and responseXML make them.
  // XML that is not well-formed gives null, or with `errorDocument` a
  // document holding a <parsererror> element, as browsers make for
  // DOMParser.
  function parseDocument(text, contentType, url, errorDocument) {
    const html = contentType === 'text/html';
    const doc = newDocument(contentType, html ? HTMLDocument.prototype : XMLDocument.prototype, url);
    const outcome = N.parseInto(doc, text, !html);
    if (html) {
      extraInfo(doc).compatMode = outcome;
    } else if (outcome !== null) {
      if (!errorDocument) return null;
      const error = N.createElementNS(PARSER_ERROR_NS, 'parsererror');
      N.insert(error, N.createText('XML Parsing Error: ' + outcome + '\nLocation: ' + url), null);
      N.insert(doc, error, null);
    }
    return doc;
  }

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
    // A new, empty XML document. The object is a native wrapper, returned
    // in place of `this`.
    constructor() {
      return newDocument('application/xml', new.target.prototype, internals.documentURL());
    }
    get implementation() {
      return implementation;
    }
    get URL() {
      return this === documentNode ? internals.documentURL() : extraInfo(this).url;
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
      return this === documentNode ? documentInfo.referrer : '';
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
      return this === documentNode ? readyState : 'complete';
    }
    get compatMode() {
      return this === documentNode ? N.compatMode() : extraInfo(this).compatMode;
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
      return this === documentNode ? documentInfo.contentType : extraInfo(this).contentType;
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
      return this === documentNode ? currentScript : null;
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
    getAnimations() {
      return [];
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
      if (this !== documentNode) {
        // XML documents keep the name's case, and only XHTML ones put
        // elements in the HTML namespace.
        const type = extraInfo(this).contentType;
        if (type !== 'text/html') {
          const name = str(localName);
          if (name.includes(':')) {
            throw domException('InvalidCharacterError', "The tag name provided ('" + name + "') is not a valid name.");
          }
          return owned(this, N.createElementNS(type === 'application/xhtml+xml' ? HTML_NS : null, name));
        }
        // Documents without a window have no custom elements.
        return owned(this, created(N.createElement(str(localName))));
      }
      return created(internals.createElement(str(localName), isOption(options)));
    }
    createElementNS(namespace, qualifiedName, options) {
      requireArgs(arguments, 2, "'createElementNS' on 'Document'");
      const ns = nullableStr(namespace);
      const name = str(qualifiedName);
      if (this === documentNode && ns === HTML_NS && !name.includes(':')) {
        return created(internals.createElement(name, isOption(options), true));
      }
      return owned(this, created(N.createElementNS(ns, name)));
    }
    createDocumentFragment() {
      return owned(this, N.createFragment());
    }
    createTextNode(data) {
      requireArgs(arguments, 1, "'createTextNode' on 'Document'");
      return owned(this, N.createText(str(data)));
    }
    createCDATASection() {
      throw domException('NotSupportedError', 'This operation is not supported for HTML documents.');
    }
    createComment(data) {
      requireArgs(arguments, 1, "'createComment' on 'Document'");
      return owned(this, N.createComment(str(data)));
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
      return createAttr(null, name, null, '');
    }
    createAttributeNS(namespace, qualifiedName) {
      return createAttr(null, str(qualifiedName), nullableStr(namespace) || null, '');
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
      return placeRange(new Range(), this, 0, this, 0);
    }
    createTreeWalker(root, whatToShow = 0xFFFFFFFF, filter = null) {
      return new TreeWalker(KEY, root, whatToShow, filter);
    }
    createNodeIterator(root, whatToShow = 0xFFFFFFFF, filter = null) {
      return new NodeIterator(KEY, root, whatToShow, filter);
    }
    importNode(node, deep = false) {
      if (node instanceof Document) throw domException('NotSupportedError', 'A document cannot be imported.');
      if (node instanceof Attr) return createAttr(null, node.name, node.namespaceURI, node.value);
      return cloneWithState(node, !!deep, this);
    }
    adoptNode(node) {
      if (node instanceof Document) throw domException('NotSupportedError', 'A document cannot be adopted.');
      if (node instanceof Attr) {
        const owner = node.ownerElement;
        if (owner !== null) owner.removeAttributeNode(node);
        return node;
      }
      if (node instanceof ShadowRoot) throw domException('HierarchyRequestError', 'A shadow root cannot be adopted.');
      N.remove(node);
      N.setOwner(node, this);
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
      if (this !== documentNode) {
        const info = extraInfo(this);
        info.written = '';
        N.parseInto(this, '', false);
      }
      return this;
    }
    close() {
      if (this !== documentNode) extraInfo(this).written = undefined;
    }
    write(...text) {
      if (this === documentNode) documentWrite(text.map((t) => str(t)).join(''));
      else writeExtra(this, text.map((t) => str(t)).join(''));
    }
    writeln(...text) {
      if (this === documentNode) documentWrite(text.map((t) => str(t)).join('') + '\n');
      else writeExtra(this, text.map((t) => str(t)).join('') + '\n');
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

  // Gives a new node `doc` made its node document. Nodes belong to the
  // page's document unless told otherwise.
  function owned(doc, node) {
    if (doc !== documentNode) N.setOwner(node, doc);
    return node;
  }

  // Scripts created by script are async unless told otherwise.
  // The `is` of createElement's options: a customized built-in element.
  function isOption(options) {
    if (typeof options === 'string') return options;
    if (options !== null && typeof options === 'object' && options.is !== undefined) return str(options.is);
    return null;
  }

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

  // document.write() on a document script made: everything written since
  // open() is parsed again, so the tree always shows it all.
  function writeExtra(doc, html) {
    const info = extraInfo(doc);
    if (info.contentType !== 'text/html') {
      throw domException('InvalidStateError', 'write() is only supported on HTML documents.');
    }
    info.written = (info.written === undefined ? '' : info.written) + html;
    info.compatMode = N.parseInto(doc, info.written, false);
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
      const doc = newDocument('text/html', HTMLDocument.prototype, 'about:blank');
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
    // An XML document, with a root element if `qualifiedName` is given.
    createDocument(namespace, qualifiedName, doctype = null) {
      requireArgs(arguments, 2, "'createDocument' on 'DOMImplementation'");
      const ns = nullableStr(namespace) || null;
      const type = ns === HTML_NS ? 'application/xhtml+xml' : ns === SVG_NS ? 'image/svg+xml' : 'application/xml';
      const name = qualifiedName === null ? '' : str(qualifiedName);
      // Check the names before making anything.
      const root = name === '' ? null : N.createElementNS(ns, name);
      const doc = newDocument(type, XMLDocument.prototype, 'about:blank');
      if (doctype !== null && doctype !== undefined) {
        if (!(doctype instanceof DocumentType)) throw new TypeError("Failed to execute 'createDocument' on 'DOMImplementation': parameter 3 is not of type 'DocumentType'.");
        N.insert(doc, doctype, null);
      }
      if (root !== null) N.insert(doc, root, null);
      return doc;
    }
  }
  tag(DOMImplementation, 'DOMImplementation');
  const implementation = new DOMImplementation(KEY);

  class DOMParser {
    parseFromString(string, type) {
      requireArgs(arguments, 2, "'parseFromString' on 'DOMParser'");
      const text = str(string);
      const t = str(type);
      if (t !== 'text/html' && !XML_TYPES.has(t)) {
        throw new TypeError("Failed to execute 'parseFromString' on 'DOMParser': The provided value '" + t + "' is not a valid enum value of type DOMParserSupportedType.");
      }
      return parseDocument(text, t, internals.documentURL(), true);
    }
  }
  tag(DOMParser, 'DOMParser');

  // XMLSerializer, close to the XML serialization: the HTML serializer's
  // markup, with the namespace declared on the outermost element. Void
  // elements are not self-closed.
  // https://w3c.github.io/DOM-Parsing/#the-xmlserializer-interface
  const escapeXmlText = (t) => t.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  function serializeXml(node) {
    switch (node.nodeType) {
      case 1: {
        const html = node.outerHTML;
        const ns = node.namespaceURI;
        if (ns === null || node.hasAttribute('xmlns')) return html;
        const end = html.search(/[\s/>]/);
        return html.slice(0, end) + ' xmlns="' + ns + '"' + html.slice(end);
      }
      case 3: return escapeXmlText(node.data);
      case 4: return '<![CDATA[' + node.data + ']]>';
      case 7: return '<?' + node.target + ' ' + node.data + '?>';
      case 8: return '<!--' + node.data + '-->';
      case 9: case 11: {
        let out = '';
        for (let c = node.firstChild; c !== null; c = c.nextSibling) out += serializeXml(c);
        return out;
      }
      case 10: return '<!DOCTYPE ' + node.name + (node.publicId ? ' PUBLIC "' + node.publicId + '"' : '') +
        (node.systemId ? (node.publicId ? '' : ' SYSTEM') + ' "' + node.systemId + '"' : '') + '>';
      default: return '';
    }
  }
  class XMLSerializer {
    serializeToString(node) {
      requireArgs(arguments, 1, "'serializeToString' on 'XMLSerializer'");
      if (!(node instanceof Node)) {
        throw new TypeError("Failed to execute 'serializeToString' on 'XMLSerializer': parameter 1 is not of type 'Node'.");
      }
      return serializeXml(node);
    }
  }
  tag(XMLSerializer, 'XMLSerializer');

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

  const nativeFunctions = {};
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
    ShadowRoot: ShadowRoot.prototype,
    Document: HTMLDocument.prototype,
    html: htmlProtos,
    svg: svgProtos,
    // Where the native members go (see nativeMember() above).
    interfaces: {
      Node: Node.prototype,
      CharacterData: CharacterData.prototype,
      Element: Element.prototype,
      Document: Document.prototype,
      DocumentFragment: DocumentFragment.prototype,
      DocumentType: DocumentType.prototype,
      NodeList: NodeList.prototype,
      HTMLCollection: HTMLCollection.prototype,
      NamedNodeMap: NamedNodeMap.prototype,
      HTMLElement: HTMLElement.prototype,
      SVGElement: SVGElement.prototype,
      MathMLElement: MathMLElement.prototype,
      CSSStyleDeclaration: CSSStyleDeclaration.prototype,
      DOMTokenList: DOMTokenList.prototype,
      DOMStringMap: DOMStringMap.prototype,
      // Native functions that are not members, filled in by N.init.
      functions: nativeFunctions,
    },
    // Hooks of the native members: what to do with the scripts an
    // insertion connected, and the base URL that reflected URLs resolve
    // against.
    hooks: {
      connected: (scripts) => connected(scripts),
      baseURL: () => internals.baseURL(),
      attrAt,
      attrNamed,
    },
    reflected,
  }, (name, message) => domException(name, message));
  documentNode = N.document();
  getComputedStyle = nativeFunctions.getComputedStyle;
  {
    const get = (C, name) => Object.getOwnPropertyDescriptor(C.prototype, name).get;
    natives.tokensLength = get(DOMTokenList, 'length');
    natives.tokensItem = DOMTokenList.prototype.item;
    natives.styleLength = get(CSSStyleDeclaration, 'length');
    natives.styleItem = CSSStyleDeclaration.prototype.item;
    natives.setCssText = Object.getOwnPropertyDescriptor(CSSStyleDeclaration.prototype, 'cssText').set;
    natives.style = get(HTMLElement, 'style');
  }

  // ---------------------------------------------------------------------
  // Hooks for the other glue and the page loader.

  // A slotted node's event path goes through its slot (the rest of the
  // path is worked out natively, see src/events/dispatch.rs).
  internals.slotFor = (node) => findSlot(node, false);
  internals.scrollTarget = () => documentNode;

  // For XMLHttpRequest's responseXML: a document parsed from a response,
  // or null if its XML is not well-formed.
  internals.parseDocument = (text, contentType, url) => parseDocument(text, contentType, url, false);

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
      const resolved = href === null || href === undefined ? undefined : internals.urlHref(href, url);
      base.href = resolved ?? url;
    }
    return base.href;
  };

  // For IntersectionObserver.
  internals.geometry = {
    scrolled: SCROLLED,
    loaded: LOADED,
    loadedBox,
    rendered,
    clientBox,
    isElement: (value) => value instanceof Element,
    contains: (a, b) => N.contains(a, b),
  };

  Object.assign(internals, {
    document: documentNode,
    nodeState: state,
    // Whether `name` is an HTML element with its own interface (not
    // HTMLUnknownElement), as customElements.define's `extends` needs.
    isKnownHTMLTag: (name) => Object.prototype.hasOwnProperty.call(htmlProtos, name),
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
    SVGGraphicsElement, SVGSVGElement, SVGScriptElement, SVGStyleElement, MathMLElement,
    Document, HTMLDocument, XMLDocument,
    DOMImplementation, DOMParser, XMLSerializer, FontFaceSet, TextTrackList, AudioTrackList,
    VideoTrackList, DOMRect, DOMRectReadOnly, DOMRectList, TreeWalker,
    NodeIterator, NodeFilter, Range, Image, Audio, Option, getComputedStyle, ...I, ...uiEvents,
  });
  defineProperty(globalThis, 'document', { get: () => documentNode, enumerable: true, configurable: false });
})
