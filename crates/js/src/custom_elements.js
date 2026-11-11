// Custom elements: customElements, the HTMLElement constructor protocol,
// upgrades, lifecycle callbacks and ElementInternals.
// https://html.spec.whatwg.org/multipage/custom-elements.html
//
// Definitions live here. Rust (src/dom/custom.rs) is told the names that
// are defined and which elements are custom, and reports back each change
// that calls for a reaction: an element becoming connected (a custom one,
// or one a definition can now upgrade), a custom one becoming disconnected,
// and an attribute of a custom one changing. Reports arrive at the end of
// the DOM call that caused them, which is when the standard runs reactions.
//
// Elements the page loader's parser creates are upgraded after each parsing
// step (internals.loader.upgradeCustomElements), not one by one as they are
// created; elements that innerHTML and the like create are upgraded when
// they become connected, as in browsers.
//
// The state each element needs is kept in its node state: `ceDefinition`
// (set once it is being or has been upgraded), `ceCustom` (once its
// constructor has run) and `ceFailed`.
(function (native, internals) {
  'use strict';

  const N = native.dom;
  const state = internals.nodeState;
  const domException = internals.domException;
  const HTML_NS = 'http://www.w3.org/1999/xhtml';
  const setPrototypeOf = Object.setPrototypeOf;
  const ReflectApply = Reflect.apply;
  const ReflectConstruct = Reflect.construct;
  const KEY = Symbol('custom elements');

  const definitions = new Map();
  const byConstructor = new Map();
  // Promises from whenDefined() for names not defined yet.
  const waiting = new Map();
  let defining = false;
  const ALREADY_CONSTRUCTED = Symbol('already constructed');

  const LIFECYCLE = ['connectedCallback', 'disconnectedCallback', 'adoptedCallback', 'attributeChangedCallback'];
  const FORM = ['formAssociatedCallback', 'formResetCallback', 'formDisabledCallback', 'formStateRestoreCallback'];

  function call(element, callback, args) {
    if (callback === undefined) return;
    try {
      ReflectApply(callback, element, args);
    } catch (error) {
      native.reportError(error);
    }
  }

  function isConstructor(value) {
    if (typeof value !== 'function') return false;
    try {
      ReflectConstruct(String, [], value);
      return true;
    } catch {
      return false;
    }
  }

  // The definition for an element named `localName` with `is` value `is`.
  // https://html.spec.whatwg.org/#look-up-a-custom-element-definition
  function lookup(localName, is) {
    const autonomous = definitions.get(localName);
    if (autonomous !== undefined && autonomous.localName === localName) return autonomous;
    if (is !== null && is !== undefined) {
      const builtIn = definitions.get(is);
      if (builtIn !== undefined && builtIn.localName === localName) return builtIn;
    }
    return undefined;
  }

  function prototypeFor(newTarget) {
    const proto = newTarget.prototype;
    return proto !== null && (typeof proto === 'object' || typeof proto === 'function') ? proto : HTMLElement.prototype;
  }

  // https://html.spec.whatwg.org/#concept-upgrade-an-element
  function upgrade(element, definition) {
    const s = state(element);
    if (s.ceDefinition !== undefined || s.ceFailed) return;
    s.ceDefinition = definition;
    definition.stack.push(element);
    try {
      const result = ReflectConstruct(definition.constructor, []);
      if (result !== element) {
        throw new TypeError("Failed to construct '" + definition.name + "': The result must be the element being upgraded.");
      }
    } catch (error) {
      s.ceDefinition = undefined;
      s.ceFailed = true;
      native.reportError(error);
      return;
    } finally {
      definition.stack.pop();
    }
    s.ceCustom = true;
    N.setCustom(element, true);
    if (definition.attributeChanged !== undefined) {
      for (let i = 0, n = N.attrCount(element); i < n; i++) {
        const [, namespace, , local, value] = N.attr(element, i);
        if (definition.observed.has(local)) call(element, definition.attributeChanged, [local, null, value, namespace]);
      }
    }
    if (N.isConnected(element)) call(element, definition.connected, []);
  }

  // https://html.spec.whatwg.org/#concept-try-upgrade
  function tryUpgrade(element) {
    const name = N.elementName(element);
    if (name[1] !== HTML_NS) return;
    const definition = lookup(name[0], N.getAttr(element, 'is'));
    if (definition !== undefined) upgrade(element, definition);
  }

  function upgradeAll(root) {
    if (definitions.size === 0) return;
    for (const element of N.upgradeCandidates(root, null, null)) tryUpgrade(element);
  }

  // Reactions reported by Rust: 0 connected, 1 disconnected, 2 attribute
  // changed.
  N.setCustomElementHook((kind, element, name, oldValue, newValue, namespace) => {
    const s = state(element);
    if (kind === 0) {
      if (s.ceCustom) call(element, s.ceDefinition.connected, []);
      else tryUpgrade(element);
    } else if (s.ceCustom) {
      const definition = s.ceDefinition;
      if (kind === 1) {
        call(element, definition.disconnected, []);
      } else if (definition.attributeChanged !== undefined && definition.observed.has(name)) {
        call(element, definition.attributeChanged, [name, oldValue, newValue, namespace]);
      }
    }
  });

  // `new MyElement()` and upgrades both end up here, through super().
  // https://html.spec.whatwg.org/#html-element-constructors
  internals.constructHTMLElement = (newTarget) => {
    const definition = byConstructor.get(newTarget);
    if (definition === undefined) throw new TypeError("Failed to construct 'HTMLElement': Illegal constructor");
    const stack = definition.stack;
    if (stack.length === 0) {
      const element = N.createElement(definition.localName);
      setPrototypeOf(element, prototypeFor(newTarget));
      const s = state(element);
      s.ceDefinition = definition;
      s.ceCustom = true;
      N.setCustom(element, true);
      return element;
    }
    const element = stack[stack.length - 1];
    if (element === ALREADY_CONSTRUCTED) {
      throw domException('InvalidStateError', "Failed to construct 'HTMLElement': This instance is already constructed");
    }
    setPrototypeOf(element, prototypeFor(newTarget));
    stack[stack.length - 1] = ALREADY_CONSTRUCTED;
    return element;
  };

  // document.createElement(name, { is }), and createElementNS in the HTML
  // namespace (`exact`: keep the name's case).
  internals.createElement = (name, is, exact) => {
    const make = () => (exact ? N.createElementNS(HTML_NS, name) : N.createElement(name));
    if (definitions.size === 0) return make();
    const localName = exact ? name : name.toLowerCase();
    const definition = lookup(localName, is);
    if (definition !== undefined && definition.localName === definition.name) {
      const element = ReflectConstruct(definition.constructor, []);
      if (!internals.isNode(element) || N.nodeType(element) !== 1) {
        throw new TypeError("Failed to execute 'createElement' on 'Document': The result must implement HTMLElement interface");
      }
      const actual = N.elementName(element);
      if (actual[0] !== localName || actual[1] !== HTML_NS || N.attrCount(element) !== 0 ||
          N.nav(element, 1) !== null || N.nav(element, 0) !== null) {
        throw domException('NotSupportedError', "Failed to execute 'createElement' on 'Document': The result must be a new element named '" + localName + "' with no attributes or children");
      }
      return element;
    }
    const element = make();
    if (is !== null && is !== undefined) N.setAttr(element, 'is', is);
    if (definition !== undefined) upgrade(element, definition);
    return element;
  };

  internals.upgradeCustomElementsIn = upgradeAll;
  internals.customElementDefinition = (element) => state(element).ceDefinition;

  class CustomElementRegistry {
    constructor(key) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
    }

    define(name, constructor, options) {
      const what = "Failed to execute 'define' on 'CustomElementRegistry': ";
      if (arguments.length < 2) {
        throw new TypeError(what + '2 arguments required, but only ' + arguments.length + ' present.');
      }
      name = String(name);
      if (!isConstructor(constructor)) throw new TypeError(what + 'The provided callback is not a constructor.');
      if (!N.validCustomElementName(name)) {
        throw domException('SyntaxError', what + '"' + name + '" is not a valid custom element name');
      }
      if (definitions.has(name)) {
        throw domException('NotSupportedError', what + 'the name "' + name + '" has already been used with this registry');
      }
      if (byConstructor.has(constructor)) {
        throw domException('NotSupportedError', what + 'this constructor has already been used with this registry');
      }
      let localName = name;
      const extended = options !== null && typeof options === 'object' ? options.extends : undefined;
      if (extended !== undefined && extended !== null) {
        localName = String(extended);
        if (N.validCustomElementName(localName) || !internals.isKnownHTMLTag(localName)) {
          throw domException('NotSupportedError', what + '"' + localName + '" is not a valid name to extend');
        }
      }
      if (defining) throw domException('NotSupportedError', what + 'this registry is already defining an element');
      defining = true;
      let definition;
      try {
        const proto = constructor.prototype;
        if (proto === null || (typeof proto !== 'object' && typeof proto !== 'function')) {
          throw new TypeError(what + "The 'prototype' property of the constructor is not an object.");
        }
        const callbacks = {};
        for (const key of LIFECYCLE) {
          const value = proto[key];
          if (value !== undefined && typeof value !== 'function') {
            throw new TypeError(what + "The '" + key + "' property of the prototype is not a function.");
          }
          callbacks[key] = value;
        }
        let observed = new Set();
        if (callbacks.attributeChangedCallback !== undefined) {
          const list = constructor.observedAttributes;
          if (list !== undefined) observed = new Set(Array.from(list, String));
        }
        const disabledList = constructor.disabledFeatures;
        const disabled = disabledList === undefined ? [] : Array.from(disabledList, String);
        const formAssociated = !!constructor.formAssociated;
        if (formAssociated) {
          for (const key of FORM) {
            const value = proto[key];
            if (value !== undefined && typeof value !== 'function') {
              throw new TypeError(what + "The '" + key + "' property of the prototype is not a function.");
            }
          }
        }
        definition = {
          name,
          localName,
          constructor,
          observed,
          connected: callbacks.connectedCallback,
          disconnected: callbacks.disconnectedCallback,
          attributeChanged: callbacks.attributeChangedCallback,
          disableInternals: disabled.includes('internals'),
          disableShadow: disabled.includes('shadow'),
          formAssociated,
          // Elements being upgraded, innermost last.
          stack: [],
        };
      } finally {
        defining = false;
      }
      definitions.set(name, definition);
      byConstructor.set(constructor, definition);
      N.defineCustomElement(name);
      // Upgrade what is already in the document, in shadow-including tree
      // order.
      for (const element of N.upgradeCandidates(internals.document, name, localName)) upgrade(element, definition);
      const pending = waiting.get(name);
      if (pending !== undefined) {
        waiting.delete(name);
        pending.resolve(constructor);
      }
    }

    get(name) {
      const definition = definitions.get(String(name));
      return definition === undefined ? undefined : definition.constructor;
    }

    getName(constructor) {
      const definition = byConstructor.get(constructor);
      return definition === undefined ? null : definition.name;
    }

    whenDefined(name) {
      name = String(name);
      if (!N.validCustomElementName(name)) {
        return Promise.reject(domException('SyntaxError', "Failed to execute 'whenDefined' on 'CustomElementRegistry': \"" + name + '" is not a valid custom element name'));
      }
      const definition = definitions.get(name);
      if (definition !== undefined) return Promise.resolve(definition.constructor);
      let pending = waiting.get(name);
      if (pending === undefined) {
        let resolve;
        const promise = new Promise((r) => { resolve = r; });
        pending = { promise, resolve };
        waiting.set(name, pending);
      }
      return pending.promise;
    }

    upgrade(root) {
      if (!internals.isNode(root)) {
        throw new TypeError("Failed to execute 'upgrade' on 'CustomElementRegistry': parameter 1 is not of type 'Node'.");
      }
      upgradeAll(root);
    }
  }
  Object.defineProperty(CustomElementRegistry.prototype, Symbol.toStringTag, { value: 'CustomElementRegistry', configurable: true });

  // ElementInternals, for components that use it to take part in forms or
  // to reach their own closed shadow root. Nothing is validated or
  // submitted: every control counts as valid.
  class CustomStateSet extends Set {}
  Object.defineProperty(CustomStateSet.prototype, Symbol.toStringTag, { value: 'CustomStateSet', configurable: true });

  const VALIDITY = Object.freeze({
    valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false,
    rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: false,
    valid: true,
  });

  class ElementInternals {
    #element;
    #states = new CustomStateSet();
    #value = null;
    constructor(key, element) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
      this.#element = element;
    }
    get shadowRoot() {
      return N.shadowRoot(this.#element);
    }
    get states() {
      return this.#states;
    }
    get form() {
      return state(this.#element).ceDefinition.formAssociated ? this.#element.closest('form') : null;
    }
    get labels() {
      return internals.staticNodeList([]);
    }
    get willValidate() {
      return false;
    }
    get validity() {
      return VALIDITY;
    }
    get validationMessage() {
      return '';
    }
    setFormValue(value) {
      this.#value = value;
    }
    setValidity() {}
    checkValidity() {
      return true;
    }
    reportValidity() {
      return true;
    }
  }
  Object.defineProperty(ElementInternals.prototype, Symbol.toStringTag, { value: 'ElementInternals', configurable: true });

  internals.attachInternals = (element) => {
    const what = "Failed to execute 'attachInternals' on 'HTMLElement': ";
    const s = state(element);
    const definition = s.ceDefinition;
    if (definition === undefined || definition.localName !== definition.name) {
      throw domException('NotSupportedError', what + 'Unable to attach ElementInternals to non-custom elements.');
    }
    if (definition.disableInternals) {
      throw domException('NotSupportedError', what + 'ElementInternals is disabled by disabledFeature static field.');
    }
    if (s.ceInternals !== undefined) {
      throw domException('NotSupportedError', what + 'ElementInternals for the specified element was already attached.');
    }
    s.ceInternals = new ElementInternals(KEY, element);
    return s.ceInternals;
  };

  // For the page loader, after each parsing step.
  internals.upgradeCustomElements = () => upgradeAll(internals.document);

  internals.expose({
    CustomElementRegistry,
    ElementInternals,
    CustomStateSet,
    customElements: new CustomElementRegistry(KEY),
  });
})
