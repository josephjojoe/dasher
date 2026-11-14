// A small Intl, since QuickJS has none. Numbers, plurals, lists, relative
// times and collation format as en-US whatever the locale: the aim there
// is that pages using Intl keep running and print plausible English, not
// locale correctness. supportedLocalesOf only claims English locales, and
// resolvedOptions().locale is the first requested English locale or
// en-US. Dates are closer to a browser: DateTimeFormat follows ICU's
// pattern selection for en-US, en-CA, en-GB and de, and knows every IANA
// time zone's rules from 1970 on. It also implements Number, BigInt and
// Date toLocale*String and String localeCompare, so their locales and
// options take effect. Intl.Locale takes tags apart and puts them back
// together, and knows the likely script and region of about fifty
// languages.
//
// This file is the JS surface: locales, reading and checking options,
// resolvedOptions and the errors. Formatting runs in Rust (src/intl/),
// which `native` here is the functions of: the options a formatter
// resolves to are handed over once, and each format call passes the
// formatter's number and the value.
//
// This file is loaded lazily (see lazy.js): it evaluates to a function
// that returns the Intl object and those methods without installing them.
// https://tc39.es/ecma402/
(function (native, internals) {
  'use strict';

  const defineProperty = Object.defineProperty;
  const isFinite = Number.isFinite;
  const floor = Math.floor;
  const abs = Math.abs;
  const dateValue = Date.prototype.getTime;
  const numberValue = Number.prototype.valueOf;
  const bigintValue = BigInt.prototype.valueOf;
  const {
    numberFormatter, numberFormat, numberToParts, pluralSelect, timeZoneName, hostTimeZone,
    dateFormatter, dateHasHour, dateFormat, dateToParts,
  } = native;
  const Intl = {};

  function hidden(target, members) {
    for (const k of Object.keys(members)) {
      defineProperty(target, k, { value: members[k], writable: true, configurable: true });
    }
  }
  const join = (parts) => parts.map((p) => p.value).join('');
  const words = (s) => s.split(' ');

  // Caches of what a string of locales, with no options, resolves to: the
  // same string always resolves the same way. Pages call toLocaleString
  // with a locale and no options in loops.
  const CACHE_SIZE = 64;
  function cached(cache, key, make) {
    let value = cache.get(key);
    if (value === undefined) {
      value = make();
      if (cache.size >= CACHE_SIZE) cache.clear();
      cache.set(key, value);
    }
    return value;
  }

  // Locales.

  const TAG = /^[a-z]{2,3}(-[a-z]{4})?(-([a-z]{2}|\d{3}))?(-([a-z\d]{5,8}|\d[a-z\d]{3}))*(-[a-wyz\d](-[a-z\d]{2,8})+)*(-x(-[a-z\d]{1,8})+)?$/i;
  const isEnglish = (tag) => /^en(-|$)/.test(tag);

  function canonicalLocales(locales) {
    if (locales === undefined) return [];
    const out = [];
    for (let tag of typeof locales === 'string' || locales instanceof Locale ? [locales] : Array.from(Object(locales))) {
      if (typeof tag !== 'string' && (typeof tag !== 'object' || tag === null)) {
        throw new TypeError('Language ID should be string or object.');
      }
      tag = String(tag);
      if (!TAG.test(tag)) throw new RangeError('Incorrect locale information provided');
      let ext = false;
      tag = tag.toLowerCase().split('-').map((s, i) => {
        if (s.length === 1) ext = true;
        if (ext || !i) return s;
        if (s.length === 2) return s.toUpperCase();
        return i === 1 && /^[a-z]{4}$/.test(s) ? s[0].toUpperCase() + s.slice(1) : s;
      }).join('-');
      if (!out.includes(tag)) out.push(tag);
    }
    return out;
  }

  // The canonical locales, shared for a string: callers must not change
  // the list.
  const canonicalCache = new Map();
  const canonical = (locales) => typeof locales === 'string'
    ? cached(canonicalCache, locales, () => canonicalLocales(locales)) : canonicalLocales(locales);

  function getCanonicalLocales(locales) {
    return canonical(locales).slice();
  }

  function supportedLocalesOf(locales) {
    return canonical(locales).filter(isEnglish);
  }

  // The locale a formatter reports: the first English one asked for,
  // without extensions, or en-US.
  function resolve(locales) {
    const tag = canonical(locales).find(isEnglish);
    return tag ? tag.replace(/-[a-z\d]-.*$/, '') : 'en-US';
  }

  // Options.

  function options(o) {
    if (o === null) throw new TypeError('Cannot convert undefined or null to object');
    return o === undefined ? {} : Object(o);
  }

  function str(o, key, values, fallback) {
    const v = o[key];
    if (v === undefined) return fallback;
    const s = String(v);
    if (!values.includes(s)) throw new RangeError('Value ' + s + ' out of range for Intl options property ' + key);
    return s;
  }

  function num(o, key, min, max, fallback) {
    const v = o[key];
    if (v === undefined) return fallback;
    const n = Number(v);
    if (!(n >= min && n <= max)) throw new RangeError(key + ' value is out of range.');
    return floor(n);
  }

  // Reads the digit options into `r`. Returns true when compact notation
  // gets its own default rounding (two significant digits below 100).
  function digitOptions(o, r, minDefault, maxDefault, compact) {
    r.minimumIntegerDigits = num(o, 'minimumIntegerDigits', 1, 21, 1);
    const mnfd = num(o, 'minimumFractionDigits', 0, 100);
    const mxfd = num(o, 'maximumFractionDigits', 0, 100);
    if (o.minimumSignificantDigits !== undefined || o.maximumSignificantDigits !== undefined) {
      r.minimumSignificantDigits = num(o, 'minimumSignificantDigits', 1, 21, 1);
      r.maximumSignificantDigits = num(o, 'maximumSignificantDigits', r.minimumSignificantDigits, 21, 21);
      return false;
    }
    if (mnfd === undefined && mxfd === undefined && compact) {
      r.minimumFractionDigits = r.maximumFractionDigits = 0;
      return true;
    }
    const mn = mnfd === undefined ? Math.min(minDefault, mxfd === undefined ? minDefault : mxfd) : mnfd;
    const mx = mxfd === undefined ? Math.max(maxDefault, mn) : mxfd;
    if (mn > mx) throw new RangeError('maximumFractionDigits value is out of range.');
    r.minimumFractionDigits = mn;
    r.maximumFractionDigits = mx;
    return false;
  }

  function roundingOptions(r) {
    r.roundingIncrement = 1;
    r.roundingMode = 'halfExpand';
    r.roundingPriority = 'auto';
    if (!r.trailingZeroDisplay) r.trailingZeroDisplay = 'auto';
  }

  // Numbers. src/intl/number.rs formats them, from the resolvedOptions
  // object built here.

  const ZERO_DIGITS = 'BIF CLP DJF GNF ISK JPY KMF KRW PYG RWF UGX UYI VND VUV XAF XOF XPF';
  const THREE_DIGITS = 'BHD IQD JOD KWD LYD OMR TND';
  // Unit identifiers ECMA-402 sanctions.
  const UNIT_LIST = native.units();
  const UNITS = new Set(UNIT_LIST);

  // Resolves NumberFormat options: the resolvedOptions object and the
  // number of the formatter they make.
  function numberState(locales, o) {
    o = options(o);
    const r = { locale: resolve(locales), numberingSystem: 'latn' };
    const style = r.style = str(o, 'style', ['decimal', 'percent', 'currency', 'unit'], 'decimal');
    let currency = o.currency;
    if (currency !== undefined) {
      currency = String(currency);
      if (!/^[a-z]{3}$/i.test(currency)) throw new RangeError('Invalid currency code : ' + currency);
      currency = currency.toUpperCase();
    }
    const currencyDisplay = str(o, 'currencyDisplay', ['code', 'symbol', 'narrowSymbol', 'name'], 'symbol');
    const currencySign = str(o, 'currencySign', ['standard', 'accounting'], 'standard');
    let unit = o.unit;
    if (unit !== undefined) {
      unit = String(unit);
      if (!unit.split('-per-').every((u) => UNITS.has(u)) || unit.split('-per-').length > 2) {
        throw new RangeError('Invalid unit argument for Intl.NumberFormat() \'' + unit + '\'');
      }
    }
    const unitDisplay = str(o, 'unitDisplay', ['short', 'narrow', 'long'], 'short');
    let digits = 0;
    if (style === 'currency') {
      if (currency === undefined) throw new TypeError('Currency code is required with currency style.');
      Object.assign(r, { currency, currencyDisplay, currencySign });
      digits = ZERO_DIGITS.includes(currency) ? 0 : THREE_DIGITS.includes(currency) ? 3 : 2;
    } else if (style === 'unit') {
      if (unit === undefined) throw new TypeError('Unit is required with unit style.');
      Object.assign(r, { unit, unitDisplay });
    }
    const notation = str(o, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
    const compact = notation === 'compact';
    const auto = digitOptions(o, r, compact ? 0 : digits, compact ? 0 : style === 'currency' ? digits : style === 'percent' ? 0 : 3, compact);
    const g = o.useGrouping;
    r.useGrouping = g === undefined || g === 'true' || g === 'false' ? (compact ? 'min2' : 'auto')
      : g === true ? 'always' : !g ? false : str(o, 'useGrouping', ['min2', 'auto', 'always']);
    r.notation = notation;
    if (compact) r.compactDisplay = str(o, 'compactDisplay', ['short', 'long'], 'short');
    r.signDisplay = str(o, 'signDisplay', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto');
    r.trailingZeroDisplay = str(o, 'trailingZeroDisplay', ['auto', 'stripIfInteger'], 'auto');
    roundingOptions(r);
    return { r, id: numberFormatter(r, auto) };
  }

  // A value to format as Rust takes it: a number, or a bigint's digits.
  const numeric = (x) => typeof x === 'number' ? x : typeof x === 'bigint' ? String(x) : Number(x);
  const numberParts = (state, x) => numberToParts(state.id, numeric(x));

  function rangeParts(format, a, b) {
    if (a === undefined || b === undefined) throw new TypeError('start or end is undefined');
    const x = format(a), y = format(b);
    if (join(x) === join(y)) return [{ type: 'approximatelySign', value: '~', source: 'shared' }, ...x.map((p) => ({ ...p, source: 'shared' }))];
    return [...x.map((p) => ({ ...p, source: 'startRange' })), { type: 'literal', value: ' – ', source: 'shared' },
      ...y.map((p) => ({ ...p, source: 'endRange' }))];
  }

  class NumberFormat {
    #state;
    #format;
    constructor(locales, options) {
      this.#state = numberState(locales, options);
    }
    get format() {
      const id = this.#state.id;
      return this.#format ??= (x) => numberFormat(id, numeric(x));
    }
    formatToParts(x) {
      return numberParts(this.#state, x);
    }
    formatRange(a, b) {
      return join(this.formatRangeToParts(a, b));
    }
    formatRangeToParts(a, b) {
      return rangeParts((x) => numberParts(this.#state, x), a, b);
    }
    resolvedOptions() {
      return { ...this.#state.r };
    }
  }

  // Plural rules: English cardinal and ordinal.

  class PluralRules {
    #r;
    #id;
    constructor(locales, o) {
      o = options(o);
      const r = { locale: resolve(locales), type: str(o, 'type', ['cardinal', 'ordinal'], 'cardinal') };
      digitOptions(o, r, 0, 3, false);
      r.pluralCategories = r.type === 'ordinal' ? ['few', 'one', 'other', 'two'] : ['one', 'other'];
      roundingOptions(r);
      this.#r = r;
      this.#id = numberFormatter(r, false);
    }
    select(n) {
      n = Number(n);
      if (!isFinite(n)) return 'other';
      return pluralSelect(this.#id, n, this.#r.type === 'ordinal');
    }
    selectRange(a, b) {
      if (a === undefined || b === undefined) throw new TypeError('start or end is undefined');
      return this.select(b);
    }
    resolvedOptions() {
      const r = { ...this.#r };
      r.pluralCategories = r.pluralCategories.slice();
      return r;
    }
  }

  // Dates. src/intl/date.rs picks the pattern for the options, as ICU
  // does, and formats with it.

  const TEXT = ['narrow', 'short', 'long'];
  const NUMERIC = ['2-digit', 'numeric'];
  const STYLES = ['full', 'long', 'medium', 'short'];
  const FIELDS = [['weekday', TEXT], ['era', TEXT], ['year', NUMERIC], ['month', NUMERIC.concat(TEXT)],
    ['day', NUMERIC], ['dayPeriod', TEXT], ['hour', NUMERIC], ['minute', NUMERIC], ['second', NUMERIC],
    ['fractionalSecondDigits'], ['timeZoneName', ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric']]];
  const DATE_FIELDS = ['weekday', 'year', 'month', 'day'];
  const TIME_FIELDS = ['dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits'];
  const OPTION_KEYS = words('localeMatcher calendar numberingSystem hour12 hourCycle timeZone formatMatcher dateStyle timeStyle')
    .concat(FIELDS.map((f) => f[0]));

  // The locales with date tables, numbered as date.rs numbers them, and
  // the hour letter of each one's own hour cycle.
  const DATE_LOCALES = new Map([['en-US', 0], ['en-CA', 1], ['en-GB', 2], ['de', 3]]);
  const LOCALE_HOURS = 'hhHH';

  // The locale a date formatter reports and the number of its table: the
  // first requested locale that is English, or German for Germany; else
  // en-US. English for a region without a table formats as en-US.
  function dateLocale(locales) {
    for (let tag of canonical(locales)) {
      tag = tag.replace(/-[a-z\d]-.*$/, '');
      if (isEnglish(tag)) return [tag, DATE_LOCALES.get(tag) ?? 0];
      if (tag === 'de' || tag === 'de-DE') return [tag, 3];
    }
    return ['en-US', 0];
  }

  // The skeleton for a set of field options, as V8 builds it, in the
  // order CLDR writes skeletons.
  const HOUR_CHARS = { h11: 'K', h12: 'h', h23: 'H', h24: 'k' };
  const WIDTHS = { numeric: 1, '2-digit': 2, short: 3, long: 4, narrow: 5 };
  const ZONE_STYLES = { short: 'z', long: 'zzzz', shortOffset: 'O', longOffset: 'OOOO', shortGeneric: 'v', longGeneric: 'vvvv' };
  function skeletonOf(s, hc) {
    const field = (c, style) => style ? c.repeat(style === 'short' && c !== 'M' ? 1 : WIDTHS[style]) : '';
    return field('G', s.era) + field('y', s.year) + field('M', s.month) + field('E', s.weekday) + field('d', s.day) +
      field('B', s.dayPeriod) + field(HOUR_CHARS[hc], s.hour) + field('m', s.minute) + field('s', s.second) +
      'S'.repeat(s.fractionalSecondDigits || 0) + (ZONE_STYLES[s.timeZoneName] || '');
  }

  // A time zone option as its canonical name, or undefined for the host's
  // zone. As in V8, names match in any case and report their canonical
  // form, and a name that is no zone is a RangeError.
  function timeZone(tz) {
    if (tz === undefined) return undefined;
    tz = String(tz);
    const name = timeZoneName(tz);
    if (name === undefined) throw new RangeError('Invalid time zone specified: ' + tz);
    return name;
  }

  // Resolves DateTimeFormat options: the resolvedOptions object, less the
  // host's zone, and the number of the formatter they make.
  function dateState(locales, o) {
    o = options(o);
    const [locale, table] = dateLocale(locales);
    const r = { locale, calendar: 'gregory', numberingSystem: 'latn' };
    const zone = timeZone(o.timeZone);
    // The host's zone is named when it is asked for.
    r.timeZone = zone;
    let hc = str(o, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    if (o.hour12 !== undefined) hc = o.hour12 ? 'h12' : 'h23';
    hc ||= LOCALE_HOURS[table] === 'h' ? 'h12' : 'h23';
    const s = {};
    let any = false;
    for (const [k, values] of FIELDS) {
      const v = values ? str(o, k, values) : num(o, k, 1, 3);
      if (v === undefined) continue;
      s[k] = v;
      if (k !== 'era' && k !== 'timeZoneName') any = true;
    }
    const ds = str(o, 'dateStyle', STYLES), ts = str(o, 'timeStyle', STYLES);
    let skeleton;
    if (ds || ts) {
      const set = Object.keys(s)[0];
      if (set) throw new TypeError("Can't set option " + set + ' when ' + (ds ? 'dateStyle' : 'timeStyle') + ' is used');
    } else {
      if (!any) s.year = s.month = s.day = 'numeric';
      skeleton = skeletonOf(s, hc);
    }
    const id = dateFormatter(table, zone, hc, STYLES.indexOf(ds), STYLES.indexOf(ts), skeleton);
    if (dateHasHour(id)) {
      r.hourCycle = hc;
      r.hour12 = hc === 'h11' || hc === 'h12';
    }
    if (ds) r.dateStyle = ds;
    if (ts) r.timeStyle = ts;
    if (!ds && !ts) for (const [k] of FIELDS) if (s[k] !== undefined) r[k] = s[k];
    return { r, id };
  }

  // A time to format as Rust takes it, which checks its range.
  const timeOf = (x) => typeof x === 'number' ? x : x === undefined ? Date.now() : Number(x);
  const dateParts = (state, x) => dateToParts(state.id, timeOf(x));

  class DateTimeFormat {
    #state;
    #format;
    constructor(locales, options) {
      this.#state = dateState(locales, options);
    }
    get format() {
      const id = this.#state.id;
      return this.#format ??= (date) => dateFormat(id, timeOf(date));
    }
    formatToParts(date) {
      return dateParts(this.#state, date);
    }
    formatRange(a, b) {
      return join(this.formatRangeToParts(a, b));
    }
    formatRangeToParts(a, b) {
      return rangeParts((x) => dateParts(this.#state, x), a, b).filter((p) => p.type !== 'approximatelySign');
    }
    resolvedOptions() {
      const r = { ...this.#state.r };
      r.timeZone ??= hostTimeZone();
      return r;
    }
  }

  // Collation: case- and accent-insensitive first, then accents, then case
  // with lowercase first, comparing code points at each level.

  const COLLATION_LEVELS = { base: [0], accent: [0, 1], case: [0, 2], variant: [0, 1, 3] };

  function collationKey(s, level, caseFirst) {
    s = s.normalize('NFD');
    if (!(level & 1)) s = s.replace(/\p{M}/gu, '');
    if (level < 2) return s.toLowerCase();
    return caseFirst === 'upper' ? s : s.replace(/\p{L}/gu, (c) => {
      const lower = c.toLowerCase();
      return c === lower ? c.toUpperCase() : lower;
    });
  }

  function compareKeys(x, y, numericOrder) {
    if (numericOrder) {
      const xs = x.match(/\d+|\D+/g) || [], ys = y.match(/\d+|\D+/g) || [];
      for (let i = 0; i < xs.length && i < ys.length; i++) {
        let a = xs[i], b = ys[i];
        if (/^\d/.test(a) && /^\d/.test(b)) {
          a = a.replace(/^0+(?=.)/, '');
          b = b.replace(/^0+(?=.)/, '');
          if (a.length !== b.length) return a.length < b.length ? -1 : 1;
        }
        if (a !== b) return a < b ? -1 : 1;
      }
      return Math.sign(xs.length - ys.length);
    }
    return x < y ? -1 : x > y ? 1 : 0;
  }

  function collate(r, a, b) {
    a = String(a);
    b = String(b);
    if (r.ignorePunctuation) {
      a = a.replace(/[\p{P}\s]/gu, '');
      b = b.replace(/[\p{P}\s]/gu, '');
    }
    for (const level of COLLATION_LEVELS[r.sensitivity]) {
      const c = compareKeys(collationKey(a, level, r.caseFirst), collationKey(b, level, r.caseFirst), r.numeric);
      if (c) return c;
    }
    return 0;
  }

  function collatorOptions(locales, o) {
    o = options(o);
    const r = { locale: resolve(locales), usage: str(o, 'usage', ['sort', 'search'], 'sort') };
    const numericOrder = o.numeric === undefined ? false : Boolean(o.numeric);
    const caseFirst = str(o, 'caseFirst', ['upper', 'lower', 'false'], 'false');
    r.sensitivity = str(o, 'sensitivity', ['base', 'accent', 'case', 'variant'], 'variant');
    r.ignorePunctuation = Boolean(o.ignorePunctuation);
    r.collation = 'default';
    r.numeric = numericOrder;
    r.caseFirst = caseFirst;
    return r;
  }

  class Collator {
    #r;
    #compare;
    constructor(locales, options) {
      this.#r = collatorOptions(locales, options);
    }
    get compare() {
      return this.#compare ??= (a, b) => collate(this.#r, a, b);
    }
    resolvedOptions() {
      return { ...this.#r };
    }
  }


  // Relative times.

  const RELATIVE_UNITS = words('second minute hour day week month quarter year');
  const SHORT_UNITS = { second: 'sec.', minute: 'min.', hour: 'hr.', day: 'day', week: 'wk.', month: 'mo.', quarter: 'qtr.', year: 'yr.' };
  let decimalState;

  class RelativeTimeFormat {
    #r;
    constructor(locales, o) {
      o = options(o);
      this.#r = {
        locale: resolve(locales),
        style: str(o, 'style', ['long', 'short', 'narrow'], 'long'),
        numeric: str(o, 'numeric', ['always', 'auto'], 'always'),
        numberingSystem: 'latn',
      };
    }
    format(value, unit) {
      return join(this.formatToParts(value, unit));
    }
    formatToParts(value, unit) {
      const v = Number(value);
      const u = String(unit).replace(/s$/, '');
      if (!isFinite(v)) throw new RangeError('Invalid value for format()');
      if (!RELATIVE_UNITS.includes(u)) throw new RangeError("Invalid unit argument for format() '" + unit + "'");
      const { style, numeric: mode } = this.#r;
      const label = (plural) => style === 'long' ? u + (plural ? 's' : '')
        : plural && u === 'day' ? 'days' : plural && u === 'quarter' ? 'qtrs.' : SHORT_UNITS[u];
      if (mode === 'auto' && (v === 0 || (abs(v) === 1 && RELATIVE_UNITS.indexOf(u) > 2))) {
        const word = u === 'day' ? ['yesterday', 'today', 'tomorrow'][v + 1]
          : u === 'second' ? 'now'
            : ['last ', 'this ', 'next '][v + 1] + label(false);
        return [{ type: 'literal', value: word }];
      }
      decimalState ??= numberState();
      const number = numberParts(decimalState, abs(v)).map((p) => ({ ...p, unit: u }));
      const plural = join(number) !== '1';
      return v < 0 || Object.is(v, -0)
        ? [...number, { type: 'literal', value: ' ' + label(plural) + ' ago' }]
        : [{ type: 'literal', value: 'in ' }, ...number, { type: 'literal', value: ' ' + label(plural) }];
    }
    resolvedOptions() {
      return { ...this.#r };
    }
  }

  // Lists.

  class ListFormat {
    #r;
    constructor(locales, o) {
      o = options(o);
      this.#r = {
        locale: resolve(locales),
        type: str(o, 'type', ['conjunction', 'disjunction', 'unit'], 'conjunction'),
        style: str(o, 'style', ['long', 'short', 'narrow'], 'long'),
      };
    }
    format(list) {
      return join(this.formatToParts(list));
    }
    formatToParts(list) {
      const items = [];
      for (const item of list === undefined ? [] : list) {
        if (typeof item !== 'string') throw new TypeError('Iterable yielded ' + String(item) + ' which is not a string');
        items.push(item);
      }
      const { type, style } = this.#r;
      const word = type === 'disjunction' ? 'or' : type === 'unit' ? '' : style === 'long' ? 'and' : style === 'short' ? '&' : '';
      const comma = type === 'unit' && style === 'narrow' ? ' ' : ', ';
      const out = [];
      items.forEach((value, i) => {
        if (i) {
          const last = i === items.length - 1;
          out.push({ type: 'literal', value: !last || !word ? comma : (items.length === 2 ? ' ' : ', ') + word + ' ' });
        }
        out.push({ type: 'element', value });
      });
      return out;
    }
    resolvedOptions() {
      return { ...this.#r };
    }
  }

  // Locale: a language tag taken apart, with the -u- keywords the options
  // name. Tags are put in canonical case with their keywords in order,
  // but aliases (iw for he, gregorian for gregory) stay as written.
  // maximize and minimize know the likely script and region of the
  // languages in LIKELY only, and leave other tags as they are.
  // https://tc39.es/ecma402/#locale-objects

  const KEYWORDS = [
    ['calendar', 'ca'], ['collation', 'co'], ['hourCycle', 'hc'],
    ['caseFirst', 'kf'], ['numeric', 'kn'], ['numberingSystem', 'nu'],
  ];
  const KEYWORD_TYPE = /^[a-z\d]{3,8}(-[a-z\d]{3,8})*$/i;
  // language script region, the likeliest first for its language.
  const LIKELY = 'en Latn US|de Latn DE|fr Latn FR|es Latn ES|it Latn IT|pt Latn BR|nl Latn NL|sv Latn SE|'
    + 'da Latn DK|nb Latn NO|no Latn NO|fi Latn FI|pl Latn PL|cs Latn CZ|sk Latn SK|hu Latn HU|ro Latn RO|'
    + 'tr Latn TR|id Latn ID|ms Latn MY|vi Latn VN|ca Latn ES|hr Latn HR|sl Latn SI|et Latn EE|lv Latn LV|'
    + 'lt Latn LT|is Latn IS|ga Latn IE|fil Latn PH|sw Latn TZ|af Latn ZA|ru Cyrl RU|uk Cyrl UA|bg Cyrl BG|'
    + 'sr Cyrl RS|el Grek GR|he Hebr IL|ar Arab EG|fa Arab IR|ur Arab PK|hi Deva IN|bn Beng BD|ta Taml IN|'
    + 'te Telu IN|th Thai TH|ko Kore KR|ja Jpan JP|zh Hans CN';
  let likely;

  // The parts of a canonical tag: language, script, region, the variants,
  // the -u- attributes and keywords, and the other extensions as written.
  function parseTag(tag) {
    const parts = tag.split('-');
    const t = { language: parts[0], script: undefined, region: undefined, variants: [], attributes: [], keywords: new Map(), others: [] };
    let i = 1;
    if (i < parts.length && /^[a-z]{4}$/i.test(parts[i])) t.script = parts[i++];
    if (i < parts.length && /^([a-z]{2}|\d{3})$/i.test(parts[i])) t.region = parts[i++];
    while (i < parts.length && parts[i].length > 1) t.variants.push(parts[i++]);
    t.variants.sort();
    while (i < parts.length) {
      const singleton = parts[i++];
      const start = i;
      while (i < parts.length && (singleton === 'x' || parts[i].length > 1)) i++;
      const subtags = parts.slice(start, i);
      if (singleton !== 'u' || t.keywords.size || t.attributes.length) {
        t.others.push([singleton, ...subtags]);
        continue;
      }
      let key;
      for (const subtag of subtags) {
        if (subtag.length === 2) {
          key = subtag;
          if (!t.keywords.has(key)) t.keywords.set(key, []);
        } else if (key === undefined) {
          t.attributes.push(subtag);
        } else {
          t.keywords.get(key).push(subtag);
        }
      }
    }
    return t;
  }

  const baseName = (t) => [t.language, t.script, t.region, ...t.variants].filter((s) => s !== undefined).join('-');

  function tagOf(t) {
    const extensions = t.others.filter((e) => e[0] !== 'x').map((e) => e.join('-'));
    if (t.keywords.size || t.attributes.length) {
      const u = ['u', ...t.attributes];
      for (const key of [...t.keywords.keys()].sort()) {
        const type = t.keywords.get(key).join('-');
        u.push(key);
        // A type of true is left out, as in kn for kn-true.
        if (type !== '' && type !== 'true') u.push(type);
      }
      extensions.push(u.join('-'));
    }
    extensions.sort();
    const priv = t.others.find((e) => e[0] === 'x');
    return [baseName(t), ...extensions, ...(priv ? [priv.join('-')] : [])].join('-');
  }

  // The script and region a language most likely has, given what is known.
  function likelySubtags(t) {
    likely ??= new Map(LIKELY.split('|').map((row) => {
      const [language, script, region] = row.split(' ');
      return [language, [script, region]];
    }));
    const known = likely.get(t.language === 'und' ? 'en' : t.language);
    if (known === undefined) return undefined;
    let [script, region] = known;
    if (t.language === 'zh') {
      // Traditional where it is written, and there when that is all that is known.
      if (t.script === undefined && ['TW', 'HK', 'MO'].includes(t.region)) script = 'Hant';
      if (t.script === 'Hant') region = 'TW';
    }
    return { language: t.language === 'und' ? 'en' : t.language, script: t.script ?? script, region: t.region ?? region };
  }

  class Locale {
    #t;
    constructor(tag, o) {
      if (typeof tag !== 'string' && (typeof tag !== 'object' || tag === null)) {
        throw new TypeError('First argument to Intl.Locale constructor can\'t be empty or missing');
      }
      const t = parseTag(canonicalLocales(String(tag))[0]);
      o = options(o);
      const subtag = (key, pattern) => {
        const v = o[key];
        if (v === undefined) return undefined;
        const s = String(v);
        if (!pattern.test(s)) throw new RangeError('Incorrect locale information provided');
        return s.toLowerCase();
      };
      const language = subtag('language', /^([a-z]{2,3}|[a-z]{5,8})$/i);
      const script = subtag('script', /^[a-z]{4}$/i);
      const region = subtag('region', /^([a-z]{2}|\d{3})$/i);
      if (language !== undefined) t.language = language;
      if (script !== undefined) t.script = script[0].toUpperCase() + script.slice(1);
      if (region !== undefined) t.region = region.toUpperCase();
      for (const [name, key] of KEYWORDS) {
        let type;
        if (name === 'hourCycle') {
          type = str(o, name, ['h11', 'h12', 'h23', 'h24']);
        } else if (name === 'caseFirst') {
          type = str(o, name, ['upper', 'lower', 'false']);
        } else if (name === 'numeric') {
          type = o[name] === undefined ? undefined : String(Boolean(o[name]));
        } else {
          type = subtag(name, KEYWORD_TYPE);
        }
        if (type !== undefined) t.keywords.set(key, type.split('-'));
      }
      this.#t = t;
    }
    #keyword(key) {
      const type = this.#t.keywords.get(key);
      return type === undefined ? undefined : type.join('-');
    }
    get baseName() { return baseName(this.#t); }
    get language() { return this.#t.language; }
    get script() { return this.#t.script; }
    get region() { return this.#t.region; }
    get calendar() { return this.#keyword('ca'); }
    get collation() { return this.#keyword('co'); }
    get hourCycle() { return this.#keyword('hc'); }
    get caseFirst() { return this.#keyword('kf'); }
    get numeric() {
      const type = this.#keyword('kn');
      return type === '' || type === 'true';
    }
    get numberingSystem() { return this.#keyword('nu'); }
    maximize() {
      return new Locale(tagOf({ ...this.#t, ...likelySubtags(this.#t) }));
    }
    minimize() {
      const t = this.#t;
      const full = likelySubtags(t);
      if (full === undefined) return new Locale(tagOf(t));
      // The least that still maximizes to the same: the language alone,
      // with its region, or with its script.
      const same = (trial) => {
        const again = likelySubtags(trial);
        return again.script === full.script && again.region === full.region;
      };
      const { language } = full;
      const least = [{ language }, { language, region: full.region }, { language, script: full.script }].find(same) ?? full;
      return new Locale(tagOf({ ...t, script: undefined, region: undefined, ...least }));
    }
    toString() { return tagOf(this.#t); }
  }

  // The Intl namespace.

  function install(C, name, callable) {
    let F = C;
    if (callable) {
      // These three may be called without `new`.
      F = function (locales, options) {
        return new C(locales, options);
      };
      defineProperty(F, 'name', { value: name });
      F.prototype = C.prototype;
      hidden(C.prototype, { constructor: F });
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: 'Intl.' + name, configurable: true });
    hidden(F, { supportedLocalesOf });
    hidden(Intl, { [name]: F });
  }
  install(NumberFormat, 'NumberFormat', true);
  install(DateTimeFormat, 'DateTimeFormat', true);
  hidden(Intl.DateTimeFormat, {
    supportedLocalesOf(locales) {
      return getCanonicalLocales(locales).filter((tag) => isEnglish(tag) || /^de(-DE)?$/.test(tag.replace(/-[a-z\d]-.*$/, '')));
    },
  });
  install(Collator, 'Collator', true);
  install(PluralRules, 'PluralRules');
  install(RelativeTimeFormat, 'RelativeTimeFormat');
  install(ListFormat, 'ListFormat');
  defineProperty(Locale.prototype, Symbol.toStringTag, { value: 'Intl.Locale', configurable: true });
  hidden(Intl, { Locale });

  const SUPPORTED = {
    calendar: () => ['gregory'],
    collation: () => [],
    currency: () => native.currencies(),
    numberingSystem: () => ['latn'],
    timeZone: () => native.timeZones(),
    unit: () => UNIT_LIST.slice(),
  };
  hidden(Intl, {
    getCanonicalLocales,
    supportedValuesOf(key) {
      key = String(key);
      if (!Object.hasOwn(SUPPORTED, key)) throw new RangeError('Invalid key : ' + key);
      return SUPPORTED[key]();
    },
  });
  defineProperty(Intl, Symbol.toStringTag, { value: 'Intl', configurable: true });

  // The locale-sensitive methods of the built-ins, which QuickJS gives no
  // locale support. Formatters for the default locale and options, or for
  // a string of locales and no options, are reused across calls.

  let defaultNumber, defaultCollator;
  const numberStates = new Map();

  function formatNumber(x, locales, o) {
    let state;
    if (o !== undefined) {
      state = numberState(locales, o);
    } else if (locales === undefined) {
      state = defaultNumber ??= numberState();
    } else if (typeof locales === 'string') {
      state = cached(numberStates, locales, () => numberState(locales));
    } else {
      state = numberState(locales);
    }
    return numberFormat(state.id, numeric(x));
  }

  // The state of a toLocale*String call: `required` the fields it needs
  // one of ('date', 'time' or 'any'), `defaults` those it shows if none
  // are asked for ('date', 'time' or 'all').
  function dateStateFor(locales, o, required, defaults) {
    o = options(o);
    const c = {};
    for (const k of OPTION_KEYS) if (o[k] !== undefined) c[k] = o[k];
    const needed = required === 'date' ? DATE_FIELDS : required === 'time' ? TIME_FIELDS : DATE_FIELDS.concat(TIME_FIELDS);
    if (c.dateStyle === undefined && c.timeStyle === undefined && needed.every((k) => c[k] === undefined)) {
      if (defaults !== 'time') c.year = c.month = c.day = 'numeric';
      if (defaults !== 'date') c.hour = c.minute = c.second = 'numeric';
    }
    return dateState(locales, c);
  }

  // A toLocale*String method. `cache` holds its states for strings of
  // locales, and the default state under undefined.
  function dateMethod(required, defaults) {
    const cache = new Map();
    return (date, locales, o) => {
      const t = dateValue.call(date);
      if (t !== t) return 'Invalid Date';
      let state;
      if (o === undefined && (locales === undefined || typeof locales === 'string')) {
        state = cache.get(locales);
        if (state === undefined) {
          state = dateStateFor(locales, undefined, required, defaults);
          if (cache.size >= CACHE_SIZE) cache.clear();
          cache.set(locales, state);
        }
      } else {
        state = dateStateFor(locales, o, required, defaults);
      }
      return dateFormat(state.id, t);
    };
  }

  // lazy.js owns the global and the methods, and installs them. These
  // take the receiver first.
  return {
    Intl,
    Number: {
      toLocaleString: (x, locales, options) => formatNumber(numberValue.call(x), locales, options),
    },
    BigInt: {
      toLocaleString: (x, locales, options) => formatNumber(bigintValue.call(x), locales, options),
    },
    Date: {
      toLocaleString: dateMethod('any', 'all'),
      toLocaleDateString: dateMethod('date', 'date'),
      toLocaleTimeString: dateMethod('time', 'time'),
    },
    String: {
      localeCompare(s, that, locales, options) {
        if (s === undefined || s === null) throw new TypeError('String.prototype.localeCompare called on null or undefined');
        const r = locales === undefined && options === undefined ? defaultCollator ??= collatorOptions() : collatorOptions(locales, options);
        return collate(r, s, that);
      },
    },
  };
})
