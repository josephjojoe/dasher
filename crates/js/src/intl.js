// A small Intl, since QuickJS has none. Every locale formats as en-US: the
// aim is that pages using Intl keep running and print plausible English,
// not locale correctness. supportedLocalesOf only claims English locales,
// and resolvedOptions().locale is the first requested English locale or
// en-US. Time zones: UTC, Etc/GMT±N and ±HH:MM offsets are honoured; any
// other zone name is accepted but formats in the host's zone. The host
// zone reports as UTC, or Etc/GMT±N when the host is elsewhere.
// It also implements Number, BigInt and Date toLocale*String and String
// localeCompare, so their locales and options take effect.
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
  const Intl = {};

  function hidden(target, members) {
    for (const k of Object.keys(members)) {
      defineProperty(target, k, { value: members[k], writable: true, configurable: true });
    }
  }
  const pad = (n, k) => String(n).padStart(k, '0');
  const join = (parts) => parts.map((p) => p.value).join('');
  const words = (s) => s.split(' ');

  // Locales.

  const TAG = /^[a-z]{2,3}(-[a-z]{4})?(-([a-z]{2}|\d{3}))?(-([a-z\d]{5,8}|\d[a-z\d]{3}))*(-[a-wyz\d](-[a-z\d]{2,8})+)*(-x(-[a-z\d]{1,8})+)?$/i;
  const isEnglish = (tag) => /^en(-|$)/.test(tag);

  function getCanonicalLocales(locales) {
    if (locales === undefined) return [];
    const out = [];
    for (let tag of typeof locales === 'string' ? [locales] : Array.from(Object(locales))) {
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

  function supportedLocalesOf(locales) {
    return getCanonicalLocales(locales).filter(isEnglish);
  }

  // The locale a formatter reports: the first English one asked for,
  // without extensions, or en-US.
  function resolve(locales) {
    const tag = getCanonicalLocales(locales).find(isEnglish);
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

  // Numbers, as decimal strings so rounding is exact.

  // A non-negative finite number or bigint as [digits, point]: the value
  // is 0.digits × 10^point, digits starting with a non-zero digit.
  function decimalOf(x) {
    if (typeof x === 'bigint') {
      const s = String(x);
      return s === '0' ? ['0', 1] : [s, s.length];
    }
    if (x === 0) return ['0', 1];
    const m = /^(\d)\.?(\d*)e([+-]\d+)$/.exec(x.toExponential());
    return [m[1] + m[2], +m[3] + 1];
  }

  // Rounds 0.d × 10^p half away from zero per `r`'s digit options and
  // returns [integer digits, fraction digits].
  function round(r, d, p, auto) {
    let sig = r.maximumSignificantDigits !== undefined;
    let mnsd = r.minimumSignificantDigits, mxsd = r.maximumSignificantDigits;
    let mnfd = r.minimumFractionDigits, mxfd = r.maximumFractionDigits;
    if (auto) {
      sig = p < 2;
      mnsd = 1;
      mxsd = 2;
      mnfd = mxfd = 0;
    }
    d = '0' + d;
    p++;
    const keep = sig ? mxsd + 1 : p + mxfd;
    if (d.length > keep) {
      const up = d[keep] >= '5';
      d = d.slice(0, Math.max(keep, 0));
      if (up) {
        let i = d.length - 1;
        while (d[i] === '9') i--;
        d = d.slice(0, i) + (+d[i] + 1) + '0'.repeat(d.length - i - 1);
      }
    }
    if (p <= 0) {
      d = '0'.repeat(1 - p) + d;
      p = 1;
    }
    d = d.padEnd(p, '0');
    const int = d.slice(0, p).replace(/^0+(?=.)/, '');
    let frac = d.slice(p).replace(/0+$/, '');
    const need = !sig ? mnfd
      : int !== '0' ? mnsd - int.length
        : frac ? mnsd + frac.search(/[1-9]/) : mnsd - 1;
    frac = frac.padEnd(need, '0');
    if (r.trailingZeroDisplay === 'stripIfInteger' && !/[1-9]/.test(frac)) frac = '';
    return [int.padStart(r.minimumIntegerDigits, '0'), frac];
  }

  const COMPACT = [words(' K M B T'), words(' thousand million billion trillion')];

  // Currency code, symbol and English name ('' when there is none).
  const CURRENCIES = {};
  for (const entry of ('USD:$:US dollar,EUR:€:euro,GBP:£:British pound,JPY:¥:Japanese yen,' +
    'CNY:CN¥:Chinese yuan,INR:₹:Indian rupee,KRW:₩:South Korean won,CAD:CA$:Canadian dollar,' +
    'AUD:A$:Australian dollar,NZD:NZ$:New Zealand dollar,HKD:HK$:Hong Kong dollar,' +
    'TWD:NT$:New Taiwan dollar,MXN:MX$:Mexican peso,BRL:R$:Brazilian real,ILS:₪:Israeli new shekel,' +
    'VND:₫:Vietnamese dong,PHP:₱:Philippine peso,XAF:FCFA:Central African CFA franc,' +
    'XOF:F CFA:West African CFA franc,CHF::Swiss franc,SEK::Swedish krona,NOK::Norwegian krone,' +
    'DKK::Danish krone,PLN::Polish zloty,CZK::Czech koruna,HUF::Hungarian forint,RUB::Russian ruble,' +
    'TRY::Turkish lira,ZAR::South African rand,SGD::Singapore dollar,THB::Thai baht,' +
    'IDR::Indonesian rupiah,MYR::Malaysian ringgit,AED::UAE dirham,SAR::Saudi riyal').split(',')) {
    const [code, symbol, name] = entry.split(':');
    CURRENCIES[code] = [symbol || code, name];
  }
  const NO_PLURAL = /(yen|yuan|won|dong|baht|rand|krona|krone|koruna|franc|real)$/;
  const ZERO_DIGITS = 'BIF CLP DJF GNF ISK JPY KMF KRW PYG RWF UGX UYI VND VUV XAF XOF XPF';
  const THREE_DIGITS = 'BHD IQD JOD KWD LYD OMR TND';

  // Unit identifiers ECMA-402 sanctions, with short English labels. A
  // trailing '+' marks labels that take an 's' in the plural.
  const UNITS = {};
  for (const entry of ('acre:ac,bit:bit,byte:byte,celsius:°C,centimeter:cm,day:day+,degree:deg,' +
    'fahrenheit:°F,fluid-ounce:fl oz,foot:ft,gallon:gal,gigabit:Gb,gigabyte:GB,gram:g,hectare:ha,' +
    'hour:hr,inch:in,kilobit:kb,kilobyte:kB,kilogram:kg,kilometer:km,liter:L,megabit:Mb,' +
    'megabyte:MB,meter:m,microsecond:μs,mile:mi,mile-scandinavian:smi,milliliter:mL,millimeter:mm,' +
    'millisecond:ms,minute:min,month:mth+,nanosecond:ns,ounce:oz,percent:%,petabyte:PB,pound:lb,' +
    'second:sec,stone:st,terabit:Tb,terabyte:TB,week:wk+,yard:yd,year:yr+').split(',')) {
    const [unit, label] = entry.split(':');
    UNITS[unit] = label;
  }

  function unitLabel(unit, display, one) {
    return unit.split('-per-').map((u, i) => {
      if (display === 'long') return i || one || /(s|ius|heit)$/.test(u) ? u : u + 's';
      const label = UNITS[u];
      return label.endsWith('+') ? label.slice(0, -1) + (i || one ? '' : 's') : label;
    }).join(display === 'long' ? ' per ' : '/');
  }

  // Resolves NumberFormat options. Returns the state numberParts reads.
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
      if (!unit.split('-per-').every((u) => Object.hasOwn(UNITS, u)) || unit.split('-per-').length > 2) {
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
    return { r, auto };
  }

  function numberParts({ r, auto }, x) {
    if (typeof x !== 'bigint') x = Number(x);
    const neg = x < 0 || Object.is(x, -0);
    if (neg) x = -x;
    const out = [];
    const body = [];
    const add = (list, type, value) => list.push({ type, value });
    let zero = false, one = false;
    if (x !== x) add(body, 'nan', 'NaN');
    else if (x === Infinity) add(body, 'infinity', '∞');
    else {
      const [d, point] = decimalOf(x);
      const p = r.style === 'percent' ? point + 2 : point;
      const n = r.notation;
      const exponent = (q) => n === 'standard' || d === '0' ? 0
        : n === 'scientific' ? q - 1
          : n === 'engineering' || q > 3 ? Math.min(floor((q - 1) / 3) * 3, n === 'compact' ? 12 : Infinity) : 0;
      let e = exponent(p);
      let [int, frac] = round(r, d, p - e, auto);
      // Rounding up may carry into the next exponent (999,950 is 1M).
      if (exponent(p + 1) !== e && int.replace(/^0+(?=.)/, '').length > p - e) {
        e = exponent(p + 1);
        [int, frac] = round(r, d, p - e, auto);
      }
      zero = !/[1-9]/.test(int + frac);
      one = int === '1' && !frac && !e;
      const g = r.useGrouping;
      if (g && int.length > (g === 'min2' ? 4 : 3)) {
        let i = int.length % 3 || 3;
        add(body, 'integer', int.slice(0, i));
        for (; i < int.length; i += 3) {
          add(body, 'group', ',');
          add(body, 'integer', int.slice(i, i + 3));
        }
      } else {
        add(body, 'integer', int);
      }
      if (frac) {
        add(body, 'decimal', '.');
        add(body, 'fraction', frac);
      }
      if (n === 'compact') {
        if (e) {
          const long = r.compactDisplay === 'long';
          if (long) add(body, 'literal', ' ');
          add(body, 'compact', COMPACT[+long][e / 3]);
        }
      } else if (n !== 'standard') {
        add(body, 'exponentSeparator', 'E');
        if (e < 0) add(body, 'exponentMinusSign', '-');
        add(body, 'exponentInteger', String(abs(e)));
      }
    }
    const sd = r.signDisplay;
    const sign = sd === 'never' || x !== x ? ''
      : sd === 'auto' ? (neg ? '-' : '')
        : sd === 'always' ? (neg ? '-' : '+')
          : zero ? '' : neg ? '-' : sd === 'exceptZero' ? '+' : '';
    const accounting = sign === '-' && r.currencySign === 'accounting';
    if (accounting) add(out, 'literal', '(');
    else if (sign) add(out, sign === '-' ? 'minusSign' : 'plusSign', sign);
    const currency = r.style === 'currency' ? r.currency : '';
    const known = CURRENCIES[currency] || [currency, ''];
    if (currency && r.currencyDisplay !== 'name') {
      let symbol = r.currencyDisplay === 'code' ? currency : known[0];
      if (r.currencyDisplay === 'narrowSymbol') symbol = symbol.replace(/^[A-Z]+(?=[^A-Z])/, '');
      add(out, 'currency', symbol);
      if (/[A-Z]$/.test(symbol)) add(out, 'literal', ' ');
    }
    out.push(...body);
    if (r.style === 'percent') {
      add(out, 'percentSign', '%');
    } else if (r.style === 'unit') {
      const label = unitLabel(r.unit, r.unitDisplay, one);
      if (r.unitDisplay !== 'narrow' && !/^[%°]/.test(label)) add(out, 'literal', ' ');
      add(out, 'unit', label);
    } else if (currency && r.currencyDisplay === 'name') {
      const name = known[1] || currency;
      add(out, 'literal', ' ');
      add(out, 'currency', one || !known[1] || NO_PLURAL.test(name) ? name : name + 's');
    }
    if (accounting) add(out, 'literal', ')');
    return out;
  }

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
      return this.#format ??= (x) => join(numberParts(this.#state, x));
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
    constructor(locales, o) {
      o = options(o);
      const r = { locale: resolve(locales), type: str(o, 'type', ['cardinal', 'ordinal'], 'cardinal') };
      digitOptions(o, r, 0, 3, false);
      r.pluralCategories = r.type === 'ordinal' ? ['few', 'one', 'other', 'two'] : ['one', 'other'];
      roundingOptions(r);
      this.#r = r;
    }
    select(n) {
      n = Number(n);
      if (!isFinite(n)) return 'other';
      const [int, frac] = round(this.#r, ...decimalOf(abs(n)), false);
      if (this.#r.type === 'cardinal') return +int === 1 && !frac ? 'one' : 'other';
      if (frac) return 'other';
      const a = int.slice(-1), b = int.slice(-2).padStart(2, '0');
      return a === '1' && b !== '11' ? 'one' : a === '2' && b !== '12' ? 'two' : a === '3' && b !== '13' ? 'few' : 'other';
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

  // Dates.

  const MONTHS = words('January February March April May June July August September October November December');
  const DAYS = words('Sunday Monday Tuesday Wednesday Thursday Friday Saturday');
  const TEXT = ['narrow', 'short', 'long'];
  const NUMERIC = ['2-digit', 'numeric'];
  const STYLES = ['full', 'long', 'medium', 'short'];
  const ERAS = { long: ['Before Christ', 'Anno Domini'], short: ['BC', 'AD'], narrow: ['B', 'A'] };
  const FIELDS = [['weekday', TEXT], ['era', TEXT], ['year', NUMERIC], ['month', NUMERIC.concat(TEXT)],
    ['day', NUMERIC], ['dayPeriod', TEXT], ['hour', NUMERIC], ['minute', NUMERIC], ['second', NUMERIC],
    ['fractionalSecondDigits'], ['timeZoneName', ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric']]];
  const DATE_FIELDS = ['weekday', 'year', 'month', 'day'];
  const TIME_FIELDS = ['dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits'];
  const OPTION_KEYS = words('localeMatcher calendar numberingSystem hour12 hourCycle timeZone formatMatcher dateStyle timeStyle')
    .concat(FIELDS.map((f) => f[0]));
  const dateStyle = (weekday, month, year) => ({ weekday, month, day: 'numeric', year });
  const timeStyle = (second, timeZoneName) => ({ hour: 'numeric', minute: '2-digit', second, timeZoneName });
  const DATE_STYLES = {
    full: dateStyle('long', 'long', 'numeric'),
    long: dateStyle(undefined, 'long', 'numeric'),
    medium: dateStyle(undefined, 'short', 'numeric'),
    short: dateStyle(undefined, 'numeric', '2-digit'),
  };
  const TIME_STYLES = {
    full: timeStyle('2-digit', 'long'),
    long: timeStyle('2-digit', 'short'),
    medium: timeStyle('2-digit'),
    short: timeStyle(),
  };

  // The host's zone: UTC, or the Etc/GMT zone (or offset) of its winter
  // offset. getTimezoneOffset is positive west of Greenwich, as is the
  // sign in Etc/GMT+N.
  function hostZone() {
    const winter = new Date(2000, 0, 1).getTimezoneOffset();
    if (!winter && !new Date(2000, 6, 1).getTimezoneOffset()) return 'UTC';
    if (winter % 60 === 0) return 'Etc/GMT' + (winter > 0 ? '+' : '-') + abs(winter / 60);
    return offsetName(-winter);
  }

  const offsetName = (off) => (off < 0 ? '-' : '+') + pad(floor(abs(off) / 60), 2) + ':' + pad(abs(off) % 60, 2);

  // A time zone option as [name, fixed offset in minutes or null for the
  // host's offset].
  function timeZone(tz) {
    if (tz === undefined) {
      const host = hostZone();
      return [host, host === 'UTC' ? 0 : null];
    }
    tz = String(tz);
    if (/^(etc\/)?(utc|uct|gmt|gmt0|zulu|universal|greenwich)$/i.test(tz)) return ['UTC', 0];
    let m = /^([+-])(\d\d):?(\d\d)$/.exec(tz);
    if (m && m[3] < 60) {
      const off = (m[1] === '-' ? -1 : 1) * (m[2] * 60 + +m[3]);
      return [offsetName(off), off];
    }
    m = /^etc\/gmt([+-])(\d{1,2})$/i.exec(tz);
    if (m && m[2] <= 14) return ['Etc/GMT' + m[1] + +m[2], (m[1] === '-' ? 60 : -60) * m[2]];
    if (!/^[a-z][\w+-]*(\/[\w+-]+)*$/i.test(tz)) throw new RangeError('Invalid time zone specified: ' + tz);
    return [tz, null];
  }

  function dateState(locales, o) {
    o = options(o);
    const r = { locale: resolve(locales), calendar: 'gregory', numberingSystem: 'latn' };
    const [zone, offset] = timeZone(o.timeZone);
    r.timeZone = zone;
    let hc = str(o, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    if (o.hour12 !== undefined) hc = o.hour12 ? 'h12' : 'h23';
    let s = {};
    let any = false;
    for (const [k, values] of FIELDS) {
      const v = values ? str(o, k, values) : num(o, k, 1, 3);
      if (v === undefined) continue;
      s[k] = v;
      if (k !== 'era' && k !== 'timeZoneName') any = true;
    }
    const ds = str(o, 'dateStyle', STYLES), ts = str(o, 'timeStyle', STYLES);
    if (ds || ts) {
      const set = Object.keys(s)[0];
      if (set) throw new TypeError("Can't set option " + set + ' when ' + (ds ? 'dateStyle' : 'timeStyle') + ' is used');
      s = { ...DATE_STYLES[ds], ...TIME_STYLES[ts] };
    } else if (!any) {
      s.year = s.month = s.day = 'numeric';
    }
    if (s.hour) {
      r.hourCycle = hc ||= 'h12';
      r.hour12 = hc === 'h11' || hc === 'h12';
    }
    if (ds) r.dateStyle = ds;
    if (ts) r.timeStyle = ts;
    if (!ds && !ts) for (const [k] of FIELDS) if (s[k] !== undefined) r[k] = s[k];
    return { r, s, offset, hc, at: ds === 'full' || ds === 'long' };
  }

  function toTime(x) {
    const t = x === undefined ? Date.now() : Number(x);
    if (!isFinite(t) || abs(t) > 8.64e15) throw new RangeError('Invalid time value');
    return Math.trunc(t);
  }

  const text = (list, i, style) => style === 'long' ? list[i] : style === 'short' ? list[i].slice(0, 3) : list[i][0];
  const numeric = (n, style) => style === '2-digit' ? pad(n % 100, 2) : String(n);

  function dateParts({ r, s, offset, hc, at }, x) {
    const t = toTime(x);
    const off = offset === null ? -new Date(t).getTimezoneOffset() : offset;
    const date = new Date(t + off * 60000);
    const out = [];
    const add = (type, value) => out.push({ type, value });
    const lit = (value) => add('literal', value);
    const y = date.getUTCFullYear(), mo = date.getUTCMonth(), d = date.getUTCDate();
    const h = date.getUTCHours(), mi = date.getUTCMinutes(), sec = date.getUTCSeconds();
    if (s.weekday) {
      add('weekday', text(DAYS, date.getUTCDay(), s.weekday));
      if (s.month || s.day || s.year) lit(', ');
    }
    if (s.month && TEXT.includes(s.month)) {
      add('month', text(MONTHS, mo, s.month));
      if (s.day) {
        lit(' ');
        add('day', numeric(d, s.day));
      }
      if (s.year) {
        lit(s.day ? ', ' : ' ');
        add('year', numeric(y, s.year));
      }
    } else {
      let first = true;
      for (const [type, value] of [['month', mo + 1], ['day', d], ['year', y]]) {
        if (!s[type]) continue;
        if (!first) lit('/');
        first = false;
        add(type, numeric(value, s[type]));
      }
    }
    if (s.era) {
      if (out.length) lit(' ');
      add('era', ERAS[s.era][+(y > 0)]);
    }
    const time = s.hour || s.minute || s.second || s.dayPeriod;
    if (time) {
      if (out.length) lit(at ? ' at ' : ', ');
      if (s.hour) {
        const hour = hc === 'h23' ? h : hc === 'h24' ? h || 24 : hc === 'h11' ? h % 12 : h % 12 || 12;
        add('hour', s.hour === '2-digit' || hc === 'h23' || hc === 'h24' ? pad(hour, 2) : String(hour));
      }
      if (s.minute) {
        if (s.hour) lit(':');
        add('minute', s.hour || s.minute === '2-digit' ? pad(mi, 2) : String(mi));
      }
      if (s.second) {
        if (s.hour || s.minute) lit(':');
        add('second', s.hour || s.minute || s.second === '2-digit' ? pad(sec, 2) : String(sec));
        if (s.fractionalSecondDigits) {
          lit('.');
          add('fractionalSecond', pad(date.getUTCMilliseconds(), 3).slice(0, s.fractionalSecondDigits));
        }
      }
      if (s.dayPeriod || (s.hour && (hc === 'h12' || hc === 'h11'))) {
        if (s.hour) lit(' ');
        add('dayPeriod', !s.dayPeriod ? (h < 12 ? 'AM' : 'PM')
          : h >= 6 && h < 12 ? 'in the morning' : h >= 12 && h < 18 ? 'in the afternoon' : h >= 18 && h < 21 ? 'in the evening' : 'at night');
      }
    }
    if (s.timeZoneName) {
      if (out.length) lit(time ? ' ' : ', ');
      const style = s.timeZoneName;
      const long = style.startsWith('long');
      let name;
      if (r.timeZone === 'UTC' && (style === 'short' || style === 'long')) {
        name = long ? 'Coordinated Universal Time' : 'UTC';
      } else if (!off) {
        name = 'GMT';
      } else {
        const hh = floor(abs(off) / 60), mm = abs(off) % 60;
        name = 'GMT' + (off < 0 ? '-' : '+') + (long ? pad(hh, 2) + ':' + pad(mm, 2) : hh + (mm ? ':' + pad(mm, 2) : ''));
      }
      add('timeZoneName', name);
    }
    return out;
  }

  class DateTimeFormat {
    #state;
    #format;
    constructor(locales, options) {
      this.#state = dateState(locales, options);
    }
    get format() {
      return this.#format ??= (date) => join(dateParts(this.#state, date));
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
      return { ...this.#state.r };
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
  install(Collator, 'Collator', true);
  install(PluralRules, 'PluralRules');
  install(RelativeTimeFormat, 'RelativeTimeFormat');
  install(ListFormat, 'ListFormat');

  const SUPPORTED = {
    calendar: () => ['gregory'],
    collation: () => [],
    currency: () => Object.keys(CURRENCIES).sort(),
    numberingSystem: () => ['latn'],
    timeZone: () => ['UTC'],
    unit: () => Object.keys(UNITS),
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
  // locale support. Formatters for the default locale and options are
  // reused across calls.

  let defaultNumber, defaultCollator;
  const defaultDates = {};

  function formatNumber(x, locales, o) {
    const state = locales === undefined && o === undefined ? defaultNumber ??= numberState() : numberState(locales, o);
    return join(numberParts(state, x));
  }

  function formatDate(date, locales, o, required, defaults) {
    const t = dateValue.call(date);
    if (t !== t) return 'Invalid Date';
    const plain = locales === undefined && o === undefined;
    let state = plain ? defaultDates[defaults] : undefined;
    if (!state) {
      o = options(o);
      const c = {};
      for (const k of OPTION_KEYS) if (o[k] !== undefined) c[k] = o[k];
      const needed = required === 'date' ? DATE_FIELDS : required === 'time' ? TIME_FIELDS : DATE_FIELDS.concat(TIME_FIELDS);
      if (c.dateStyle === undefined && c.timeStyle === undefined && needed.every((k) => c[k] === undefined)) {
        if (defaults !== 'time') c.year = c.month = c.day = 'numeric';
        if (defaults !== 'date') c.hour = c.minute = c.second = 'numeric';
      }
      state = dateState(locales, c);
      if (plain) defaultDates[defaults] = state;
    }
    return join(dateParts(state, t));
  }

  // lazy.js owns the global and the methods, and installs them.
  return {
    Intl,
    Number: {
      toLocaleString(locales, options) {
        return formatNumber(numberValue.call(this), locales, options);
      },
    },
    BigInt: {
      toLocaleString(locales, options) {
        return formatNumber(bigintValue.call(this), locales, options);
      },
    },
    Date: {
      toLocaleString(locales, options) {
        return formatDate(this, locales, options, 'any', 'all');
      },
      toLocaleDateString(locales, options) {
        return formatDate(this, locales, options, 'date', 'date');
      },
      toLocaleTimeString(locales, options) {
        return formatDate(this, locales, options, 'time', 'time');
      },
    },
    String: {
      localeCompare(that, locales, options) {
        if (this === undefined || this === null) throw new TypeError('String.prototype.localeCompare called on null or undefined');
        const r = locales === undefined && options === undefined ? defaultCollator ??= collatorOptions() : collatorOptions(locales, options);
        return collate(r, this, that);
      },
    },
  };
})
