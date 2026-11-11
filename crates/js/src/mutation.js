// MutationObserver and MutationRecord.
// https://dom.spec.whatwg.org/#mutation-observers
//
// Registrations live here, in a WeakMap from node to its registered
// observers, so they last exactly as long as the node's wrapper. Rust is
// told which nodes have any (native.dom.setObserved). For every change to
// the tree made from script whose target, or an ancestor of it, is among
// them, Rust calls queueRecord with the change and those observed nodes,
// nearest first. queueRecord queues a record for each interested observer,
// and records are delivered together in one microtask, as the standard's
// "notify mutation observers" does.
(function (native, internals) {
  'use strict';

  const N = native.dom;
  const reportError = native.reportError;
  const ReflectApply = Reflect.apply;
  const PromiseThen = Promise.prototype.then;
  const resolvedPromise = Promise.resolve();
  const WeakRefCtor = WeakRef;
  const WeakRefDeref = WeakRef.prototype.deref;
  const isNode = internals.isNode;
  const staticNodeList = internals.staticNodeList;

  const KEY = Symbol('mutation');
  const CHILD_LIST = 0;
  const ATTRIBUTES = 1;
  const CHARACTER_DATA = 2;
  const TYPES = ['childList', 'attributes', 'characterData'];
  const EMPTY = Object.freeze(staticNodeList([]));

  function tag(ctor, name) {
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  // node -> its registered observers, as { observer, options, source }.
  // `source` is null, or for a transient registration (one that follows a
  // removed node for a subtree observer until the next delivery) the
  // registration it came from.
  const registrations = new WeakMap();

  function register(node, registration) {
    let list = registrations.get(node);
    if (list === undefined) {
      list = [];
      registrations.set(node, list);
      N.setObserved(node, true);
    }
    list.push(registration);
  }

  function unregister(node, predicate) {
    const list = registrations.get(node);
    if (list === undefined) return;
    let kept = 0;
    for (let i = 0; i < list.length; i++) {
      if (!predicate(list[i])) list[kept++] = list[i];
    }
    list.length = kept;
    if (kept === 0) {
      registrations.delete(node);
      N.setObserved(node, false);
    }
  }

  // Observers with something to deliver. They are delivered to in the
  // order they were created, as browsers do.
  let pending = new Set();
  let microtaskQueued = false;
  let created = 0;
  // An observer's private state, given out by the class below.
  let stateOf;

  function schedule(observer) {
    pending.add(observer);
    if (microtaskQueued) return;
    microtaskQueued = true;
    ReflectApply(PromiseThen, resolvedPromise, [notify]);
  }

  // https://dom.spec.whatwg.org/#notify-mutation-observers
  function notify() {
    microtaskQueued = false;
    const observers = [...pending];
    pending = new Set();
    if (observers.length > 1) observers.sort((a, b) => stateOf(a).order - stateOf(b).order);
    for (const observer of observers) {
      const s = stateOf(observer);
      const records = s.records;
      s.records = [];
      dropTransients(observer, s);
      if (records.length === 0) continue;
      try {
        ReflectApply(s.callback, observer, [records, observer]);
      } catch (error) {
        reportError(error);
      }
    }
  }

  function dropTransients(observer, s) {
    for (const node of s.transient) unregister(node, (r) => r.observer === observer && r.source !== null);
    s.transient = [];
  }

  function bool(value) {
    return value === undefined ? undefined : !!value;
  }

  // Converts a MutationObserverInit dictionary, members in WebIDL order.
  function readInit(init) {
    if (init === undefined || init === null) init = {};
    else if (typeof init !== 'object' && typeof init !== 'function') {
      throw new TypeError("Failed to execute 'observe' on 'MutationObserver': The provided value is not of type 'MutationObserverInit'.");
    }
    let attributeFilter = init.attributeFilter;
    if (attributeFilter !== undefined) {
      if (attributeFilter === null || typeof attributeFilter !== 'object' || typeof attributeFilter[Symbol.iterator] !== 'function') {
        throw new TypeError("Failed to execute 'observe' on 'MutationObserver': Failed to read the 'attributeFilter' property from 'MutationObserverInit': The provided value cannot be converted to a sequence.");
      }
      attributeFilter = Array.from(attributeFilter, (name) => `${name}`);
    }
    return {
      attributeFilter,
      attributeOldValue: bool(init.attributeOldValue),
      attributes: bool(init.attributes),
      characterData: bool(init.characterData),
      characterDataOldValue: bool(init.characterDataOldValue),
      childList: !!init.childList,
      subtree: !!init.subtree,
    };
  }

  function invalid(message) {
    return new TypeError("Failed to execute 'observe' on 'MutationObserver': The options object " + message);
  }

  class MutationObserver {
    #s;

    static {
      stateOf = (observer) => observer.#s;
    }

    constructor(callback) {
      if (typeof callback !== 'function') {
        throw new TypeError("Failed to construct 'MutationObserver': parameter 1 is not of type 'Function'.");
      }
      // `nodes` holds weak references to the nodes observed, `transient`
      // the nodes with transient registrations.
      this.#s = { callback, order: created++, records: [], nodes: [], pruneAt: 16, transient: [] };
    }

    observe(target, options) {
      const s = this.#s;
      if (!isNode(target)) {
        throw new TypeError("Failed to execute 'observe' on 'MutationObserver': parameter 1 is not of type 'Node'.");
      }
      const o = readInit(options);
      if ((o.attributeOldValue !== undefined || o.attributeFilter !== undefined) && o.attributes === undefined) {
        o.attributes = true;
      }
      if (o.characterDataOldValue !== undefined && o.characterData === undefined) o.characterData = true;
      if (!o.childList && !o.attributes && !o.characterData) {
        throw invalid("must set at least one of 'attributes', 'characterData', or 'childList' to true.");
      }
      if (o.attributeOldValue && !o.attributes) {
        throw invalid("may only set 'attributeOldValue' to true when 'attributes' is true or not present.");
      }
      if (o.attributeFilter !== undefined && !o.attributes) {
        throw invalid("may only set 'attributeFilter' when 'attributes' is true or not present.");
      }
      if (o.characterDataOldValue && !o.characterData) {
        throw invalid("may only set 'characterDataOldValue' to true when 'characterData' is true or not present.");
      }
      const opts = {
        childList: o.childList,
        attributes: !!o.attributes,
        characterData: !!o.characterData,
        subtree: o.subtree,
        attributeOldValue: !!o.attributeOldValue,
        characterDataOldValue: !!o.characterDataOldValue,
        attributeFilter: o.attributeFilter === undefined ? null : o.attributeFilter,
      };
      const list = registrations.get(target);
      const existing = list === undefined ? undefined : list.find((r) => r.observer === this && r.source === null);
      if (existing !== undefined) {
        for (const node of s.transient) unregister(node, (r) => r.source === existing);
        existing.options = opts;
        return;
      }
      register(target, { observer: this, options: opts, source: null });
      // Forget nodes that have been collected, whenever the list has
      // doubled since the last time.
      if (s.nodes.length >= s.pruneAt) {
        s.nodes = s.nodes.filter((ref) => ReflectApply(WeakRefDeref, ref, []) !== undefined);
        s.pruneAt = Math.max(16, s.nodes.length * 2);
      }
      s.nodes.push(new WeakRefCtor(target));
    }

    disconnect() {
      const s = this.#s;
      for (const ref of s.nodes) {
        const node = ReflectApply(WeakRefDeref, ref, []);
        if (node !== undefined) unregister(node, (r) => r.observer === this);
      }
      for (const node of s.transient) unregister(node, (r) => r.observer === this);
      s.nodes = [];
      s.transient = [];
      s.records = [];
    }

    takeRecords() {
      const s = this.#s;
      const records = s.records;
      s.records = [];
      return records;
    }
  }
  tag(MutationObserver, 'MutationObserver');

  class MutationRecord {
    #type;
    #target;
    #added;
    #removed;
    #prev;
    #next;
    #name;
    #namespace;
    #oldValue;

    constructor(key, type, target, added, removed, prev, next, name, namespace, oldValue) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
      this.#type = type;
      this.#target = target;
      this.#added = added;
      this.#removed = removed;
      this.#prev = prev;
      this.#next = next;
      this.#name = name;
      this.#namespace = namespace;
      this.#oldValue = oldValue;
    }

    get type() { return TYPES[this.#type]; }
    get target() { return this.#target; }
    get addedNodes() { return this.#added; }
    get removedNodes() { return this.#removed; }
    get previousSibling() { return this.#prev; }
    get nextSibling() { return this.#next; }
    get attributeName() { return this.#name; }
    get attributeNamespace() { return this.#namespace; }
    get oldValue() { return this.#oldValue; }
  }
  tag(MutationRecord, 'MutationRecord');

  // https://dom.spec.whatwg.org/#queueing-a-mutation-record
  // `observed` are the inclusive ancestors of `target` that have
  // registrations, nearest first.
  function queueRecord(observed, type, target, added, removed, prev, next, name, namespace, oldValue) {
    let interested = null;
    for (const node of observed) {
      const list = registrations.get(node);
      if (list === undefined) continue;
      for (const r of list) {
        const o = r.options;
        if (node !== target && !o.subtree) continue;
        if (type === ATTRIBUTES) {
          if (!o.attributes) continue;
          if (o.attributeFilter !== null && (namespace !== null || !o.attributeFilter.includes(name))) continue;
        } else if (type === CHARACTER_DATA) {
          if (!o.characterData) continue;
        } else if (!o.childList) {
          continue;
        }
        if (interested === null) interested = new Map();
        const wantsOld = type === ATTRIBUTES ? o.attributeOldValue : type === CHARACTER_DATA && o.characterDataOldValue;
        if (wantsOld) interested.set(r.observer, oldValue);
        else if (!interested.has(r.observer)) interested.set(r.observer, null);
      }
    }

    // Removed nodes stay observed by the subtree observers of their old
    // ancestors until the next delivery, so changes made to them right
    // after removal are still seen.
    if (removed.length !== 0) {
      for (const node of observed) {
        const list = registrations.get(node);
        if (list === undefined) continue;
        for (const r of list.slice()) {
          if (!r.options.subtree) continue;
          const source = r.source === null ? r : r.source;
          const s = stateOf(r.observer);
          for (const child of removed) {
            register(child, { observer: r.observer, options: r.options, source });
            s.transient.push(child);
          }
          schedule(r.observer);
        }
      }
    }

    if (interested === null) return;
    const addedList = added.length === 0 ? EMPTY : staticNodeList(added);
    const removedList = removed.length === 0 ? EMPTY : staticNodeList(removed);
    for (const [observer, old] of interested) {
      const record = new MutationRecord(KEY, type, target, addedList, removedList, prev, next, name, namespace, old);
      stateOf(observer).records.push(record);
      schedule(observer);
    }
  }

  N.setMutationHook(queueRecord);

  internals.expose({
    MutationObserver,
    MutationRecord,
    // Chrome still has the prefixed alias, and old feature tests use it.
    WebKitMutationObserver: MutationObserver,
  });
})
