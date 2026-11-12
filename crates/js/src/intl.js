// A small Intl, since QuickJS has none. Numbers, plurals, lists, relative
// times and collation format as en-US whatever the locale: the aim there
// is that pages using Intl keep running and print plausible English, not
// locale correctness. supportedLocalesOf only claims English locales, and
// resolvedOptions().locale is the first requested English locale or
// en-US. Dates are closer to a browser: DateTimeFormat follows ICU's
// pattern selection for en-US, en-CA, en-GB and de, and knows every IANA
// time zone's rules from 1970 on (see "Dates" and "Time zones" below).
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
  //
  // Formatting follows ICU, which is what V8 formats with: the options
  // become a skeleton ('yMMdd'), the locale's closest pattern is found and
  // its field widths adjusted ('y-MM-dd'), and the pattern is filled in.
  // bestPattern is ICU's DateTimePatternGenerator::getBestPattern; the
  // locale tables hold CLDR's patterns for en-US, en-CA, en-GB and de.
  // They were checked against Chrome 157 over 27,854 option combinations
  // per locale.

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

  // Locale data. Names are [wide, abbreviated, narrow]; `alone` holds the
  // names used when a month or weekday stands by itself, where they
  // differ. `date`, `time` and `glue` run from full to short; `formats`
  // maps skeletons to patterns (CLDR's availableFormats), the locale's own
  // first and then its parent's.
  const short3 = (list) => list.map((s) => s.slice(0, 3));
  const initials = (list) => list.map((s) => s[0]);
  const EN_MONTHS = words('January February March April May June July August September October November December');
  const EN_DAYS = words('Sunday Monday Tuesday Wednesday Thursday Friday Saturday');
  const DE_MONTHS = words('Januar Februar März April Mai Juni Juli August September Oktober November Dezember');
  const DE_DAYS = words('Sonntag Montag Dienstag Mittwoch Donnerstag Freitag Samstag');
  const EN = {
    hour: 'h',
    months: [EN_MONTHS, short3(EN_MONTHS), initials(EN_MONTHS)],
    days: [EN_DAYS, short3(EN_DAYS), initials(EN_DAYS)],
    ampm: ['AM', 'PM'],
    eras: [['Before Christ', 'Anno Domini'], ['BC', 'AD'], ['B', 'A']],
    // Day periods as [first hour, name or names], each running to the
    // next, and the names of noon, which is the hour of 12 until the
    // minutes or seconds shown say otherwise.
    periods: [[0, 'in the morning'], [12, 'in the afternoon'], [18, 'in the evening'], [21, 'at night']],
    noon: ['noon', 'noon', 'n'],
    date: ['EEEE, MMMM d, y', 'MMMM d, y', 'MMM d, y', 'M/d/yy'],
    time: ['h:mm:ss a zzzz', 'h:mm:ss a z', 'h:mm:ss a', 'h:mm a'],
    glue: ["{1} 'at' {0}", "{1} 'at' {0}", '{1}, {0}', '{1}, {0}'],
    formats: ['Bh=h B|Bhm=h:mm B|Bhms=h:mm:ss B|d=d|E=ccc|EBhm=E h:mm B|EBhms=E h:mm:ss B|Ed=d E|Eh=E h a|Ehm=E h:mm a|' +
      'EHm=E HH:mm|Ehms=E h:mm:ss a|EHms=E HH:mm:ss|Gy=y G|GyMd=M/d/y G|GyMMM=MMM y G|GyMMMd=MMM d, y G|GyMMMEd=E, MMM d, y G|' +
      'h=h a|H=HH|hm=h:mm a|Hm=HH:mm|hms=h:mm:ss a|Hms=HH:mm:ss|hmsv=h:mm:ss a v|Hmsv=HH:mm:ss v|hmv=h:mm a v|Hmv=HH:mm v|' +
      'M=L|Md=M/d|MEd=E, M/d|MMM=LLL|MMMd=MMM d|MMMEd=E, MMM d|MMMMd=MMMM d|ms=mm:ss|y=y|yM=M/y|yMd=M/d/y|yMEd=E, M/d/y|' +
      'yMMM=MMM y|yMMMd=MMM d, y|yMMMEd=E, MMM d, y|yMMMM=MMMM y'],
    // How a field with no pattern of its own is added, by field number,
    // and what the field is called.
    append: { 0: '{0} {1}', 1: '{0} {1}', 6: '{0} {1}', 15: '{0} {1}' },
    names: { 3: 'month', 9: 'day', 10: 'AM/PM', 11: 'hour', 12: 'minute', 13: 'second' },
    utc: 'Coordinated Universal Time',
    // Zone names: the language of the long ones (0 English, 1 German),
    // the letters TZ_METAZONES marks the locale's short ones with, the
    // region whose zones stand for a metazone, and words spelt its way.
    zones: { language: 0, shorts: 'e', region: 'US' },
  };
  const LOCALES = {
    'en-US': EN,
    'en-CA': {
      ...EN,
      ampm: ['a.m.', 'p.m.'],
      periods: [[0, ['in the morning', 'in the morning', 'mor']], [12, ['in the afternoon', 'in the afternoon', 'aft']],
        [18, ['in the evening', 'in the evening', 'eve']], [21, ['at night', 'at night', 'night']]],
      date: ['EEEE, MMMM d, y', 'MMMM d, y', 'MMM d, y', 'y-MM-dd'],
      zones: { language: 0, shorts: 'ec', region: 'CA', spelling: [['St. Pierre & Miquelon', 'Saint-Pierre-et-Miquelon'], [' & ', ' and ']] },
      formats: ['Ed=E d|Md=MM-dd|MEd=E, MM-dd|MMdd=MM-dd|yM=y-MM|yMd=y-MM-dd|yMEd=E, y-MM-dd', EN.formats[0]],
    },
    'en-GB': {
      ...EN,
      hour: 'H',
      months: [EN_MONTHS, short3(EN_MONTHS).map((s) => s === 'Sep' ? 'Sept' : s), initials(EN_MONTHS)],
      ampm: ['am', 'pm'],
      zones: { language: 0, shorts: 'g', region: 'GB', spelling: [['St. ', 'St ']] },
      date: ['EEEE, d MMMM y', 'd MMMM y', 'd MMM y', 'dd/MM/y'],
      time: ['HH:mm:ss zzzz', 'HH:mm:ss z', 'HH:mm:ss', 'HH:mm'],
      formats: ['Ed=E d|GyMd=dd/MM/y G|GyMMMd=d MMM y G|GyMMMEd=E, d MMM y G|Md=dd/MM|MEd=E dd/MM|MMdd=dd/MM|' +
        'MMMd=d MMM|MMMEd=E d MMM|MMMMd=d MMMM|yM=MM/y|yMd=dd/MM/y|yMEd=E, dd/MM/y|yMMMd=d MMM y|yMMMEd=E, d MMM y', EN.formats[0]],
    },
    de: {
      hour: 'H',
      months: [DE_MONTHS, words('Jan. Feb. März Apr. Mai Juni Juli Aug. Sept. Okt. Nov. Dez.'), initials(DE_MONTHS)],
      monthsAlone: [DE_MONTHS, words('Jan Feb Mär Apr Mai Jun Jul Aug Sep Okt Nov Dez'), initials(DE_MONTHS)],
      days: [DE_DAYS, DE_DAYS.map((s) => s.slice(0, 2) + '.'), initials(DE_DAYS)],
      daysAlone: [DE_DAYS, DE_DAYS.map((s) => s.slice(0, 2)), initials(DE_DAYS)],
      ampm: ['AM', 'PM'],
      eras: [['v. Chr.', 'n. Chr.'], ['v. Chr.', 'n. Chr.'], ['v. Chr.', 'n. Chr.']],
      periods: [[0, 'nachts'], [5, 'morgens'], [10, ['vormittags', 'vorm.', 'vorm.']], [12, 'mittags'],
        [13, ['nachmittags', 'nachm.', 'nachm.']], [18, 'abends']],
      date: ['EEEE, d. MMMM y', 'd. MMMM y', 'dd.MM.y', 'dd.MM.yy'],
      time: ['HH:mm:ss zzzz', 'HH:mm:ss z', 'HH:mm:ss', 'HH:mm'],
      glue: ["{1} 'um' {0}", "{1} 'um' {0}", '{1}, {0}', '{1}, {0}'],
      formats: ["Bh=h B|Bhm=h:mm B|Bhms=h:mm:ss B|d=d|E=ccc|EBhm=E h:mm B|EBhms=E h:mm:ss B|Ed=E, d.|Eh=E, h a|Ehm=E h:mm a|EHm=E, HH:mm|" +
        "Ehms=E, h:mm:ss a|EHms=E, HH:mm:ss|Gy=y G|GyMd=dd.MM.y G|GyMMM=MMM y G|GyMMMd=d. MMM y G|GyMMMEd=E, d. MMM y G|" +
        "h=h a|H=HH 'Uhr'|hm=h:mm a|Hm=HH:mm|hms=h:mm:ss a|Hms=HH:mm:ss|hmsv=h:mm:ss a v|Hmsv=HH:mm:ss v|hmv=h:mm a v|Hmv=HH:mm v|" +
        'M=L|Md=d.M.|MEd=E, d.M.|MMd=dd.MM.|MMdd=dd.MM.|MMM=LLL|MMMd=d. MMM|MMMEd=E, d. MMM|MMMMd=d. MMMM|MMMMEd=E, d. MMMM|' +
        'ms=mm:ss|y=y|yM=M/y|yMd=d.M.y|yMEd=E, d.M.y|yMM=MM/y|yMMdd=dd.MM.y|yMMM=MMM y|yMMMd=d. MMM y|yMMMEd=E, d. MMM y|yMMMM=MMMM y'],
      append: { 0: '{1} {0}', 1: '{1} {0}', 6: '{0} {1}', 15: '{0} {1}' },
      names: { 3: 'Monat', 9: 'Tag', 10: 'Tageshälfte', 11: 'Stunde', 12: 'Minute', 13: 'Sekunde' },
      utc: 'Koordinierte Weltzeit',
      decimal: ',',
      zones: { language: 1, shorts: 'd', region: 'DE' },
    },
  };

  // The locale a date formatter reports and its data: the first requested
  // locale that is English, or German for Germany; else en-US. English
  // for a region without a table formats as en-US.
  function dateLocale(locales) {
    for (let tag of getCanonicalLocales(locales)) {
      tag = tag.replace(/-[a-z\d]-.*$/, '');
      if (isEnglish(tag)) return [tag, LOCALES[tag] || EN];
      if (tag === 'de' || tag === 'de-DE') return [tag, LOCALES.de];
    }
    return ['en-US', EN];
  }

  // Pattern fields, numbered as ICU numbers them: era 0, year 1, month 3,
  // weekday 6, day 9, day period 10, hour 11, minute 12, second 13,
  // fractional second 14, zone 15. Fields 0-9 are the date.
  const FIELD = { G: 0, y: 1, M: 3, L: 3, E: 6, c: 6, d: 9, a: 10, B: 10, h: 11, K: 11, H: 11, k: 11, m: 12, s: 13, S: 14, z: 15, O: 15, v: 15 };
  const PART = { G: 'era', y: 'year', M: 'month', L: 'month', E: 'weekday', c: 'weekday', d: 'day', a: 'dayPeriod', B: 'dayPeriod',
    h: 'hour', K: 'hour', H: 'hour', k: 'hour', m: 'minute', s: 'second', S: 'fractionalSecond', z: 'timeZoneName', O: 'timeZoneName', v: 'timeZoneName' };
  const HOUR_CHARS = { h11: 'K', h12: 'h', h23: 'H', h24: 'k' };
  // How far a letter's type sits from its field's first letter, as text
  // and as a number: ICU's field types, in steps of 16.
  const TEXT_SHIFT = { L: 1, c: 2, B: 3, O: 1, v: 2 };
  const NUMBER_SHIFT = { L: 1, c: 2, K: 1, H: 10, k: 11 };

  // A field's type: positive and growing with its width when numeric,
  // negative when text. Patterns are compared by these.
  function fieldType(c, n) {
    if ('GEaBzOv'.includes(c) || (n > 2 && 'MLc'.includes(c))) return (n > 4 ? -0x101 : n === 4 ? -0x104 : -0x103) - 16 * (TEXT_SHIFT[c] || 0);
    return 0x100 + 16 * (NUMBER_SHIFT[c] || 0) + n;
  }

  // A pattern as literal strings and [letter, width] fields.
  function tokens(pattern) {
    return pattern.match(/'[^']*'|([A-Za-z])\1*|[^A-Za-z']+/g).map((s) => s[0] === "'" ? s.slice(1, -1)
      : /[A-Za-z]/.test(s[0]) ? [s[0], s.length] : s);
  }

  // What a skeleton or pattern asks for: per field its type, letter and
  // width, and a mask of the fields present. As in ICU, a 12-hour hour
  // brings AM/PM with it and a 24-hour one drops any day period.
  function matcher(pattern) {
    const m = { type: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], chr: [], len: [], mask: 0 };
    const set = (c, n) => {
      const f = FIELD[c];
      m.type[f] = fieldType(c, n);
      m.chr[f] = c;
      m.len[f] = n;
      m.mask |= 1 << f;
    };
    let quoted = false;
    for (let i = 0, j; i < pattern.length; i = j) {
      const c = pattern[i];
      j = i + 1;
      if (c === "'") quoted = !quoted;
      if (quoted || FIELD[c] === undefined) continue;
      while (pattern[j] === c) j++;
      set(c, j - i);
    }
    if (m.type[11]) {
      if (m.chr[11] === 'h' || m.chr[11] === 'K') {
        if (!m.type[10]) set('a', 1);
      } else {
        m.type[10] = 0;
        m.mask &= ~0x400;
      }
    }
    if (m.type[14] && !m.type[13]) set('s', 1);
    return m;
  }

  // A locale's patterns, in the order ICU collects them: each field by
  // itself, the date and time styles, then `formats`, the locale's own
  // before its parent's and each sorted by skeleton. Only the last have a
  // skeleton that was written down rather than read off the pattern.
  function patternsOf(data) {
    if (data.patterns) return data.patterns;
    const map = new Map();
    const add = (skeleton, pattern, given) => {
      const m = matcher(skeleton);
      let key = '';
      for (let i = 0; i < 16; i++) if (m.type[i]) key += m.chr[i].repeat(m.len[i]);
      if (given || !map.has(key)) map.set(key, { m, pattern, given });
    };
    for (const c of 'GyMEdaHmsSv') add(c, c);
    for (let i = 0; i < 4; i++) {
      add(data.date[i], data.date[i]);
      add(data.time[i], data.time[i]);
    }
    const seen = new Set();
    for (const level of data.formats) {
      for (const entry of level.split('|').sort()) {
        const [skeleton, pattern] = entry.split('=');
        if (!seen.has(skeleton)) add(skeleton, pattern, true);
        seen.add(skeleton);
      }
    }
    return data.patterns = [...map.values()];
  }

  // The pattern closest to the fields of `want` that are in `mask`, with
  // the fields it lacks. A missing field costs more than any difference
  // in width, and an unwanted one more than that. Of two equally close,
  // the one lacking the later field wins.
  function closest(data, want, mask) {
    let best, bestDistance = Infinity, bestMissing = 0, bestExtra = 0;
    for (const entry of patternsOf(data)) {
      let distance = 0, missing = 0, extra = 0;
      for (let i = 0; i < 16; i++) {
        const mine = mask >> i & 1 ? want.type[i] : 0, other = entry.m.type[i];
        if (mine === other) continue;
        if (!mine) {
          distance += 0x10000;
          extra |= 1 << i;
        } else if (!other) {
          distance += 0x1000;
          missing |= 1 << i;
        } else {
          distance += abs(mine - other);
        }
      }
      if (distance < bestDistance || (distance === bestDistance && missing > bestMissing)) {
        best = entry;
        bestDistance = distance;
        bestMissing = missing;
        bestExtra = extra;
        if (!distance) break;
      }
    }
    return [best, bestMissing, bestExtra];
  }

  // `pattern` with its fields widened or narrowed to what `want` asks.
  // Minutes and seconds keep the pattern's width, and so does a field
  // whose width in the pattern's own skeleton is the one asked for: that
  // is how 'y-MM-dd' survives a request for a numeric month.
  function adjust(entry, pattern, want, decimal) {
    let out = '';
    for (const t of tokens(pattern)) {
      if (typeof t === 'string') {
        out += /[A-Za-z]/.test(t) ? "'" + t + "'" : t;
        continue;
      }
      let [c, n] = t;
      const f = FIELD[c];
      if (decimal && f === 13) {
        out += c.repeat(n) + decimal + 'S'.repeat(want.len[14]);
        continue;
      }
      if (want.type[f]) {
        let asked = want.len[f];
        if (want.chr[f] === 'E' && asked < 3) asked = 3;
        const own = entry.given && (entry.m.len[f] === asked || fieldType(c, n) > 0 !== entry.m.type[f] > 0);
        if (f !== 12 && f !== 13 && !own) n = asked;
        if (f !== 11 && f !== 3 && f !== 6 && f !== 1) c = want.chr[f];
      }
      out += c.repeat(n);
    }
    return out;
  }

  // The pattern for the fields in `mask`: the closest one, with each
  // field it lacks appended the way the locale appends that field.
  function appending(data, want, mask) {
    let [entry, missing] = closest(data, want, mask);
    let out = adjust(entry, entry.pattern, want);
    let last = 0;
    while (missing && missing !== last) {
      if ((missing & 0x6000) === 0x4000 && (mask & 0x6000) === 0x6000) {
        out = adjust(entry, out, want, data.decimal || '.');
        missing &= ~0x4000;
        continue;
      }
      const asked = missing;
      [entry, missing] = closest(data, want, asked);
      const top = 31 - Math.clz32(asked & ~missing);
      out = (data.append[top] || '{0} ({2}: {1})').replace('{0}', out).replace('{1}', adjust(entry, entry.pattern, want))
        .replace('{2}', "'" + data.names[top] + "'");
      last = missing;
    }
    return out;
  }

  const glue = (data, month, weekday, time, date) => data.glue[month === 4 ? (weekday ? 0 : 1) : month === 3 ? 2 : 3]
    .replace('{0}', time).replace('{1}', date);

  // ICU's getBestPattern: the locale's pattern for a skeleton. A date and
  // a time with no pattern in common are found apart and glued, the glue
  // chosen by the width of the month. A skeleton the locale has a pattern
  // for, or a date and a time it has patterns for, need no search.
  function bestPattern(data, skeleton) {
    const cache = data.cache ??= new Map();
    let pattern = cache.get(skeleton);
    if (pattern !== undefined) return pattern;
    if (!data.keys) {
      data.keys = new Map();
      for (const level of data.formats) {
        for (const entry of level.split('|')) {
          const [key, value] = entry.split('=');
          if (!data.keys.has(key)) data.keys.set(key, value);
        }
      }
    }
    const split = skeleton.search(/[BhHKkmsSzOv]/);
    const date = data.keys.get(skeleton.slice(0, split)), time = data.keys.get(skeleton.slice(split));
    pattern = data.keys.get(skeleton);
    if (pattern === undefined && split > 0 && date !== undefined && time !== undefined) {
      pattern = glue(data, (/M+/.exec(skeleton) || [''])[0].length, skeleton.includes('E'), time, date);
    }
    if (pattern === undefined) {
      const want = matcher(skeleton);
      const [entry, missing, extra] = closest(data, want, -1);
      if (!missing && !extra) {
        pattern = adjust(entry, entry.pattern, want);
      } else {
        const date = want.mask & 0x3ff ? appending(data, want, want.mask & 0x3ff) : '';
        const time = want.mask & 0xfc00 ? appending(data, want, want.mask & 0xfc00) : '';
        pattern = !date || !time ? date || time : glue(data, want.type[3] ? want.len[3] : 0, want.type[6], time, date);
      }
    }
    cache.set(skeleton, pattern);
    return pattern;
  }

  // The skeleton for a set of field options, as V8 builds it, in the
  // order CLDR writes skeletons.
  const WIDTHS = { numeric: 1, '2-digit': 2, short: 3, long: 4, narrow: 5 };
  const ZONE_STYLES = { short: 'z', long: 'zzzz', shortOffset: 'O', longOffset: 'OOOO', shortGeneric: 'v', longGeneric: 'vvvv' };
  function skeletonOf(s, hc) {
    const field = (c, style) => style ? c.repeat(style === 'short' && c !== 'M' ? 1 : WIDTHS[style]) : '';
    return field('G', s.era) + field('y', s.year) + field('M', s.month) + field('E', s.weekday) + field('d', s.day) +
      field('B', s.dayPeriod) + field(HOUR_CHARS[hc], s.hour) + field('m', s.minute) + field('s', s.second) +
      'S'.repeat(s.fractionalSecondDigits || 0) + (ZONE_STYLES[s.timeZoneName] || '');
  }

  // Time zones.

  const offsetName = (off) => (off < 0 ? '-' : '+') + pad(floor(abs(off) / 60), 2) + ':' + pad(abs(off) % 60, 2);
  // Time zone rules: what Chrome 157 reports for every zone it accepts,
  // from 1970 to 2100, packed. Numbers are base 36, and times of day and
  // offsets count half minutes.
  // TZ_SLOTS: one transition a year, as month, day (two digits: a day of
  // the month, 40 + weekday for the last such weekday, or 50 + 31 * weekday
  // + day - 1 for the first such weekday on or after a day), time of day
  // on the clock before it, and the change in offset.
  // TZ_ZONES: 'names:offset at the start of 1970:eras' per set of zones
  // with the same rules, a leading digit in a name standing for a region
  // (REGIONS). An era is the year it starts, less 1970, in two digits,
  // then the two-digit numbers of its slots.
  // TZ_LINKS: other names for zones, as name=number of the zone, counting
  // the names in TZ_ZONES.
  const TZ_SLOTS = '30p24o+3c 80r0-3c 4060+3c 20o3c+3c 80ma0-3c 90q0-3c 30p0+3c 90v6o-3c 4010+3c 0010+3c 4013c+3c 901a0-3c 60p3c+3c 60c3c+3c 4063c+3c 35g280+3c 818280-3c 30r280+3c 80l280-3c 30q280+3c 806280-3c 30o280+3c 70s280-3c 30n280+3c 70k280-3c 30t280+3c 70a280-3c 809280+3c 80u280-3c 40f280+3c 50q280-3c 60v280+3c 80p280-3c 918280-3c 50n280+3c 70v280-3c 30u280+3c 60v280-3c 80r280-3c 40v280+3c 703280-3c 20f280+3c b0v280-3c 401280+3c 707280-3c 302280+3c 60u280-3c 30t6o+3c 60ka0-3c 70k6o+3c 80ua0-3c 30s6o+3c 607a0-3c 70a6o+3c 90ra0-3c 20u6o+3c 50sa0-3c 7026o+3c 90qa0-3c 20t6o+3c 50ea0-3c 60j6o+3c 90pa0-3c 20r6o+3c 505a0-3c 60a6o+3c 90ua0-3c 20q6o+3c 40la0-3c 6026o+3c 90ta0-3c 20p6o+3c 40da0-3c 50h6o+3c 405a0-3c 5096o+3c 30ja0-3c 40v6o+3c 30ba0-3c 40g6o+3c 20ra0-3c 4086o+3c 20ja0-3c 30n6o+3c 20aa0-3c 30e6o+3c 10na0-3c 3066o+3c 10fa0-3c 20m6o+3c 80k6o-3c 50o0+3c 8010-3c 7010-3c 80s0-3c 5010+3c 7040-3c 20g0+3c 2146o+3c 814a0-3c 914a0-3c 30e0+3c 70l0-3c 4020+3c 7080-3c 3030+3c 60v0-3c 318280+3c 90e280-3c 376280+3c 00f140+3c 00v280-3c 90v280-3c 0070+2h 90e0+3c 2080-3c 0013c+3c 0016o-3c 0010-3c 3010+3c 9010-3c 3060+3c 3040+3c 9030-3c 4040+3c 80u0-3c a0a6o-3c 20t3c+3c 30u0+3c 80o3c-3c 9013c-3c 80p3c-3c 20q0+3c 80u3c-3c 80u6o-3c 20l0-3c 8046o+3c 31e6o-3c 81e6o+3c 3146o+3c 9146o-3c 0066o+3c 90r6o-3c 10n6o+3c 90q6o-3c 30o6o+3c 31e6o+3c 21l6o+3c a1e6o-3c a020+3c 20f0-3c 90p0+3c 10e0-3c 1070-3c 90g0+3c 00t0-3c 90f0+3c 10b0-3c 11n0-3c 9060+3c 2010-3c 90b0+3c 11y0-3c 91f0+3c 10i0-3c 10h0-3c a030+3c 10g0-3c 90l0+3c 00n0+3c 4010-3c b010+3c 21e0-3c 91s0+3c 2010-6o 4070+3c 90k0+3c 90i0+3c 2070-3c 5010-3c 50k0+3c b0u0+3c 20g0-3c 2030-6o 90k0+6o 40v0-3c 60p0+3c 2050-3c 20e0-6o 00l0-3c 2090-3c 90c0+3c 50d0+3c 90j0+3c 40u0-3c 9010+3c 3010-3c 90m0+3c 9050+3c 20v0-3c 1140-3c 10n0-3c 91e0+3c 31e0-3c 81e0+3c 3040-3c 90h0+3c 21k0-3c 31i0-3c 21z0-3c 11o0-3c 1090-3c 00v0-3c 11r0-3c 91n0+3c 10q0-3c 4066o+3c 3046o+6o 50c6o+3c 9026o-3c 30g6o+3c 9016o-3c 30f6o+3c 30k6o+3c 80p6o-3c b050+3c b0i0+3c 10c0-3c 9030+3c 10r0-3c 9080+3c 90f0-3c 1036o+3c 3046o+3c 3026o+3c a050-3c 90s6o-3c 10f0-3c 10k0-3c 10j0-3c a050+3c 10p0-3c 11s0-3c 91q0+3c 11t0-3c 91r0+3c a040+3c b0q6o+3c 0040-3c 3076o+3c 3056o+3c 7026o-3c 90p6o-3c 1016o+3c b09a0-1o 4018c+1o 3036o+3c 20d6o+3c a0u0-3c 1140+3c 51e0-3c 00j0+3c 6010-3c 00i0+3c 20t0-3c 21l0-3c 91m0+3c 20b0-3c 80u0+3c 21m0-3c 30c0-3c 80g0+3c 20u0-3c 80r0+3c 90a0+3c 4080-3c 70l0+3c 3140-3c 81f0+3c 41m0-3c 71l0+3c 81i0+3c 80r24o-3c 27721c+3c 87624o-3c 0010+a0 90s0+3c 2086o+3c 30u6o+3c a0a0-3c 41e0+3c 8140-3c 90m0-3c 97724o-3c 20p21c+3c 27724o+3c 9140-3c 3052+3c 90p2-3c 3032+6o 90u2-6o 31e2+3c 9142-3c 21l2+3c a1e2-3c 20d2+3c a066o-3c a046o-3c a0p0+3c 10o0-3c 40l0+3c 80m0-3c 20n0+3c 8070-3c a0s0+3c 1050-3c 7010+2i 20t3c-3c 3140+3c 9080-3c 21p0+3c 91l0-3c 41h0+3c 21r0+3c 91j0-3c 90e0-3c 31e0+3c 91k3c-3c 91j3c-3c 2140+3c 9143c-3c 20s0+3c 90t3c-3c 21n0+3c 21l0+3c 20k0+3c a0d3c-3c a043c-3c a1e3c-3c 90t6o-3c 30p6o+3c 20b6o+3c 30q6o+3c 20b6o+6o 30t6o+6o 90v6o-6o 2040-6o 20h0-3c 9060+6o 90u6o-6o 10f280-3c 26u280+3c 51v280-3c 001dc+3c 80n280-3c b0v280+3c 309280+3c 901280-3c 90u6o-3c 90i0+6o 40n0-3c 80q0+3c a026o-3c a016o-3c 21k6o+3c 00k6o-3c 20a6o+3c a036o-3c 50e0-3c 30n0+3c 60g0-3c 00d0+50 20a0-3c 8010-1o b0m0+3c b0j0+3c 2060-3c b040+3c b0h0+3c 2040-3c 30t0+3c b0e0+3c 10s0-3c b0b0+3c 90t0+3c 2030-3c 90r0+3c 80j0+3c 20r6o-3c 9096o+3c 21l6o-3c 91e6o+3c 2086o-3c 90u6o+3c 21i6o+3c 20e6o+3c 9010+1o 4080+3c 90u0-3c 31e3c+3c 40f0-3c 70e0+3c 31f0-3c 10l0-1o 90p0+1o 00k0-1o 90v0+1o 00l0-1o 9140+1o b033c+3c 80sa0-3c 20t0+6o 80r3c-3c 8143c-3c 90r0-3c 7070-3c 8146o-3c 90i6o+a0 2056o-a0 90s6o+a0 10mdc-a0 90m0+a0 20bdc-a0 907dc+a0 20ha0-a0 904a0+a0 208a0-a0 9042+a0 21l0-a0 91e2+a0 209a0-a0 90i6o-6o 20b3c+6o 90s6o-6o 10m3c+6o 21la0-3c 9146o+3c 10ra0-3c 90t6o+3c 21ea0-3c 214a0-3c 202a0-3c 90j6o+3c 21ra0-3c 20qa0-3c 70r6o+3c 302a0-3c 9016o+3c 20pa0-3c 9076o+3c 31ea0-3c 90i6o-3c a036o+3c 90q6o+3c 205a0-3c 9086o+3c 21sa0-3c 20ia0-3c 80u6o+3c 8146o+3c 91l0+3c b010-a0 2143c+6o 914a0-6o 1010-nc a010+nc b0i6o-6o 80ta0-3c 00j6o+3c 80ra0-3c 505280+3c 31a280+3c 819280-3c 20v280+3c 34u280+3c 94u280-3c 906280-3c 407280+3c 905280-3c 904280-3c 30g280+3c 80q280-3c 301280+3c 80f280-3c 8623c-3c 50u280+3c 217280+3c 8193c-3c 20s280+3c 20r280+3c 90o3c-3c 20p280+3c 90f3c-3c 218280+3c 9193c-3c 20t280+3c b0j280-3c 10o280+3c 90va0-3c 20s6o+3c 80qa0-3c 80pa0-6o 80oa0-3c 20v0+3c 80t6o-3c 2143c+3c 301a0+3c 901dc-3c 20vgo+3c 90rk0-3c 214dc+3c 914go-3c 40s0-3c 50m0+3c 90g0-3c 40a0+3c 9040-3c 70v6o-3c 31j0+3c 2148c+3c 9148c-3c 20r8c+3c 90q6o-6o 40p0+3c 90q1o-1o 30f1o-1o 4016o+3c 8016o-3c 3096o+3c 10g6o+3c 9096o-3c 2016o+3c 20f6o+3c 20v6o+3c 3016o+3c 3080+3c 80p0-3c 2150+3c 20u0+3c a020-3c a010-3c 20r0+3c 35p0+3c 9190-3c 2190+3c 20p0+3c 50j24o+3c 4030-3c 80h0+3c 809a0-3c 30d0+3c 90c0-3c 40f0+3c 90b0-3c 3020+3c 9020-3c 20t0+3c 90pdc-3c 214a0+3c 914dc-3c 20ra0+3c 90tdc-3c 6070+3c 90d0-3c 30k0+3c 70v0-3c 7030+3c 80e3c-3c 70q3c-3c 40i0+3c 30f0+3c 80d0-3c 30a0+3c 8040-3c 8030-3c 70q0-3c 2200+3c 81e0-3c 8050-3c 70s0-3c 3630+3c 9620-3c 30g0+3c 9046o-3c 80d6o-3c 70t0-3c 8043c-3c 20r2+3c 70b0-3c 3012+3c 80l3c-3c 90o0-3c 90n3c-3c 2733c+3c 91a3c-3c 30c6o+3c 2776o+3c 9776o-3c 90a6o-3c 80h6o-3c 20o6o+3c 90k6o+3c 70o6o-3c 9056o+3c 70g6o-3c 80r6o+3c 7016o-3c 80j6o+3c 90o6o-3c 60n6o-3c 8036o+3c 60f6o-3c 70q6o+3c 50u6o-3c 70i6o+3c 50m6o-3c 7036o+3c 5066o-3c 60p6o+3c 40t6o-3c 40l6o-3c 4066o-3c 50o6o+3c 30r6o-3c 5086o+3c 30c6o-3c 3046o-3c 40n6o+3c 2766o+3c 90k6o-3c 90c6o-3c 80j6o-3c 80a6o-3c 90m6o+3c 8026o-3c 90e6o+3c 70i6o-3c 9066o+3c 70a6o-3c 80l6o+3c 60p6o-3c 80c6o+3c 60h6o-3c 6096o-3c 50o6o-3c 70c6o+3c 50f6o-3c 60r6o+3c 5076o-3c 40n6o-3c 60b6o+3c 40f6o-3c 50q6o+3c 30t6o-3c 30l6o-3c 30d6o-3c 40p6o+3c 70u0+3c 31qbo+3c 91tbo-3c 30mbo+3c 90lbo-3c b0ubo+3c 90kbo-3c 31rbo+3c 91qbo-3c 40dbo+3c 21a6o+3c 81a6o-3c 81a0-3c 20f0+3c 80g0-3c 20l0+3c 80e0-3c 8060-3c 8036o-3c 9063c-3c 3093c+3c 9073c-3c 20s3c+3c 9033c-3c 3073c+3c 80m3c-3c 80g6o-3c 9056o-3c 80r6o-3c 80c6o-3c 80n6o-3c 26b6o+3c 3070+3c 9060-3c 0010+u b0v26c+1o 60t0-3c 60o6o+3c 70f0-1o 40426c+1o b0l0-3c 50d0-3c 41h6o+3c 91ia0-3c 4046o+3c 80e6o-3c 31o6o+3c 81o6o-3c 6010+3c 80o0-3c 50r0-3c 20l24o+3c 90l0-1o 7053c-3c a0b0-1o 40q0+3c 80j0-3c 80n0-3c 4030+3c 20m0+3c 80l0-3c 401a0-3c 40t6o+3c 3010+a0 b0r3c+3c 50h6o-3c 80q3c-3c 80s6o-3c a0p6o+3c 80p0+3c 30t0-3c 30s0-3c 31t0-3c 81m0+3c 31s6o-3c 90v6o+3c 21na0-3c 91w6o+3c 203a0-3c 90r6o+3c 20ma0-3c 90p6o+3c 207a0-3c 21xa0-3c 90s6o+3c 201a0-3c 304a0-3c 204a0-3c a0h6o+3c b036o+3c 20ta0-3c 206a0-3c 2010+1o 21e6o-3c 2036o-3c 90r6o+1o 21n6o-1o 91w6o+1o 21r6o-1o 9146o+1o 21e6o-1o 2146o-1o 20q6o-1o 70r6o+1o 3026o-1o 90t6o+1o 20p6o-1o 90s6o+1o 31e6o-1o 91e6o+1o 91q6o+3c 80pa0-3c 30c0+3c a0q3c-3c 30b6o+3c 90aa0-3c 80odc-3c 301u0+3c 814dc-3c 40r0+3c 30524o+3c 80s3c-3c 21s6o+3c 920a0-3c 9206o-3c 91y6o-3c 5033c+3c 20l6o+3c 90g6o-3c 60v6o+3c 30k3c+3c 21x3c+3c 20sa0+3c 90udc-3c 20va0+3c 90qdc-3c 20ta0+3c a08dc-3c 6016o-3c 90rdc-3c 3023c+3c 36q24o+3c 91e3c-3c 20r24o+3c 30224o+3c 40v0+3c 40n0+3c 40s0+3c 80t3c-3c 30l0+3c 80g3c-3c 31r6o+3c 81s6o-3c 80l6o-3c 40u280+3c 40m280+3c 40r280+3c 502280+3c 41a280+3c 40l280+3c 40q280+3c 90ka0+3c b046o+3c 20u6o+6o 20v24o+3c 30424o+3c 30324o+3c 90vdc-3c 403280+3c 46k280+3c 96k280-3c 45p280+3c 81a280-3c 402280+3c 903280-3c 30p280+3c 902280-3c 30h280+3c b0r6o-3c 3033c+3c 20t6o-3c 302dc-3c 80oa0+3c b0u0+280 31edc-3c 814a0+3c 304dc-3c b0s6o+3c a0396+3c 10nci-3c 90q96+3c 21eci-3c 91496+3c 205ci-3c 90896+3c 21sci-3c 91e96+3c 20ici-3c 80u96+3c 31eci-3c 81496+3c 20s1y0-3c 90a1y0+3c 26q1y0-3c 96r1y0+3c 20a1y0-3c 80t1y0+3c 26r1y0-3c 90921c+3c 26p21c-3c 96q21c+3c 30b21c-3c 90a21c+3c 96r21c+3c 20a21c-3c 80f21c+3c 26r21c-3c 20t21c-3c 90b21c+3c 20e21c-3c 80q21c+3c 30321c-3c 26q21c-3c 40721c-3c 70k21c+3c 31a21c-3c 86k21c+3c 46r21c-3c 76q21c+3c 36k21c-3c 86n21c+3c b0m24o+3c 2200-3c 8200+3c 00q0-3c 00o0-3c b0v0+280 a016o+3c 10sa0-3c a076o+3c a0t6o+3c 20sa0-3c 90o6o+3c 90n6o+3c 01ta0-3c 91y6o+3c 00j6o-3c a026o+3c 01pa0-3c a1e6o+3c 00da0-3c a0a6o+3c 00ca0-3c b0k6o+3c 00ha0-3c 81e6o-3c b0g6o+3c 10o6o-3c 40q6o+3c 70m6q-3c 70s6o-3c 9010+28 70l0+280 10a6o+1o 9046o-1o b030+3c b016o+3c 30r0+1o a0c0+3c 21e0-1o 2030-1o a056o+3c 00s6o-3c a046o+3c 00r6o-3c a066o+3c 00fa0-3c';
  const TZ_ZONES = '3Abidjan,Factory,3Accra,3Bamako,3Banjul,3Conakry,3Dakar,3Freetown,3Lome,3Nouakchott,3Ouagadougou,7Reykjavik,7St_Helena:0|3Nairobi,1Riyadh,9GMT-3,3Addis_Ababa,3Asmera,3Dar_es_Salaam,3Djibouti,3Kampala,3Mogadishu,6Syowa,1Aden,1Kuwait,8Antananarivo,8Comoro,8Mayotte:a0|3Algiers:0:010001,02,0702,080304,0905,0a0607,0b08,0c|3Lagos,9GMT-1,3Bangui,3Brazzaville,3Douala,3Kinshasa,3Libreville,3Luanda,3Malabo,3Niamey,3Porto-Novo:3c|3Bissau:-3c:0509,06|3Johannesburg,3Maputo,9GMT-2,3Blantyre,3Bujumbura,3Gaborone,3Harare,3Kigali,3Lubumbashi,3Lusaka,3Maseru,3Mbabane:6o|3Cairo:6o:000a0b,0c0c0b,0d0d0b,0e0a0b,0j0e0b,0k0a0b,0p0f0g,100h0i,110j0k,120l0m,130n0o,140p0q0r0s,15,180t0u0v0w,19,1h0f0x|3Casablanca:0:040y0z,05,061011,071012,081314,09,0e15,0f16,0g,12130z,13130o,141718,15191a,161b1c1d1e,171f1g1h1i,181j1k1l1m,191n1o1p1q,1a1r1s1t1u,1b1v1w1x1y,1c1z2021,1d2223,1e2425,1f2627,1g2829,1h2a2b,1i2c2d,1j2e2f,1k2g2h2i,1l|3Ceuta:0:042j2k,05,06082l,07082m,082n2o,09,0e2p,0f,0g2q2r,0q2q2s|3El_Aaiun:-3c:062t082l,07082m,082n2o,09,122n2k,132n2u,142v2w,152x2y,161b1c1d1e,171f1g1h1i,181j1k1l1m,191n1o1p1q,1a1r1s1t1u,1b1v1w1x1y,1c1z2021,1d2223,1e2425,1f2627,1g2829,1h2a2b,1i2c2d,1j2e2f,1k2g2h2i,1l|3Juba:6o:002z30,023130,0g,0u32,0v,1f33,1g|3Khartoum:6o:002z30,023130,0g,0u32,0v,1b34,1c|3Monrovia:-2h:0235,03|3Ndjamena:3c:0936,0a37,0b|3Sao_Tome:0:1c38,1d39,1e|3Tripoli:6o:0c3a3b3c,0d3b3c,0f3d3c,0g3e3f,0h3b3c,0k3g,0l,0q3h,0r3e,0s,163i,173j,18|3Tunis:3c:073k3l,08083m,09,0i2n3n,0j3o3l,0k083p,0l,0z083q,102q2s,13|3Windhoek:6o:0o3r3s,0p3t3u,1c|0Adak:-10o:003v3w,043x3y,053z40,063v3w,0d41,0e3v3w,0h423w,114344|0Anchorage:-xc:003v3w,043x3y,053z40,063v3w,0d41,0e3v3w,0h423w,114344|0La_Paz,0Puerto_Rico,9GMT+4,0Anguilla,0Antigua,0Aruba,0Blanc-Sablon,0Curacao,0Dominica,0Grenada,0Guadeloupe,0Kralendijk,0Lower_Princes,0Marigot,0Montserrat,0Port_of_Spain,0St_Barthelemy,0St_Kitts,0St_Lucia,0St_Thomas,0St_Vincent,0Tortola:-dc|0Araguaina:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k4d,0l,0p4c,0q4e4f,0s4g4h,0t4i4j,0v4k36,0w4l4m,0x4n,0y,164o,174l,18|0Argentina/La_Rioja:-a0:044p4q,05,0i4r,0j4s4t,0l4u4v4w,0m4g4x,0n4y,0o,0y4z50,0z,1151,1252,13|0Argentina/Rio_Gallegos:-a0:044p4q,05,0i4r,0j4s4t,0n4y,0o,0y4z50,0z,1151,1252,13|0Argentina/Salta:-a0:044p4q,05,0i4r,0j4s4t,0l5354,0m4g4x,0n4y,0o,1151,1252,13|0Argentina/San_Juan:-a0:044p4q,05,0i4r,0j4s4t,0l4u4v4w,0m4g4x,0n4y,0o,0y5556,0z,1151,1252,13|0Argentina/San_Luis:-a0:044p4q,05,0i4r,0j574c,0k584c,0l4g2n,0m,0y5556,0z,1151,12595a5b,13374h,14|0Argentina/Tucuman:-a0:044p4q,05,0i4r,0j4s4t,0l5354,0m4g4x,0n4y,0o,0y4z5c,0z,1151,12525d,1346,14|0Argentina/Ushuaia:-a0:044p4q,05,0i4r,0j4s4t,0n4y,0o,0y5e50,0z,1151,1252,13|0Asuncion:-dc:025f,03,045g,055f,064g5f,095g5f,0j5g5h,0k5g5f,0l5g4f,0m4g5i,0n5j5f,0o5k5f,0q4g4f,0r5l5i,0s4s5m,0w5n5o,0y5p5q,0z5r4t,145s5m,175t5m,1j|0Bahia:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k5u4t,0m5v47,0n5w5q,0o5x5y,0q4e4f,0s4g4h,0t4i4j,0v4k36,0w4l4m,0x4n,0y,154a,165z,17|0Bahia_Banderas:-nc:0q423w,0v603q,0w423w,146107,15423w,1h|0Barbados:-dc:076263,086465,09663q,0a6768,0b|0Belem:-a0:0f45,0g4647,0h4847,0i49,0j|0Belize:-k0:0369,045v,05,0c6a,0d6b,0e|0Boa_Vista:-dc:0f45,0g4647,0h4847,0i49,0j,0t6c,0u6d6e6f,0v|0Bogota:-go:0m2v,0n49,0o|0Boise:-nc:003v3w,046g3y,053z40,063v3w,0h423w,114344|0Buenos_Aires:-a0:044p4q,05,0i4r,0j4s4t,0n4y,0o,1151,12525d,1346,14|0Cambridge_Bay:-nc:023v3w,0h423w,0t6h,0u6i6j,0v6k,0w423w,114344|0Campo_Grande:-dc:0f45,0g4647,0h4847,0i494a,0j4b4c,0k5u4t,0m5v47,0n5w5q,0o5x5y,0q4e4f,0s4g4h,0t4i4j,0v4k36,0w4l4m,0x4n5d,0y6l45,0z6m4a,106n6o,116p36,126q6r,165z4o,176s6t,1c4k6u,1d4l,1e|0Cancun:-k0:0b6v,0c,0d6w,0e,0q6x3y,0r2f,0s6y6z70,0t423w,0v603q,0w423w,1971,1a|0Caracas:-dc:1172,12,1a73,1b|0Catamarca:-a0:044p4q,05,0i4r,0j4s4t,0l5354,0m4g4x,0n4y,0o,0y4z50,0z,1151,1252,13|0Cayenne,9GMT+3:-a0|0Panama,9GMT+5,0Cayman,0Coral_Harbour:-go|0Chicago,CST6CDT:-k0:003v3w,043x3y,053z40,063v3w,0h423w,114344|0Chihuahua:-k0:0q423w,0s70,0t423w,0v603q,0w423w,1g74,1h|0Ciudad_Juarez:-k0:0q423w,0s70,0t423w,0v603q,0w423w,144344,1g7576,1h4344|0Cordoba:-a0:044p4q,05,0i4r,0j4s4t,0l5354,0m4g4x,0n4y,0o,1151,12525d,1346,14|0Costa_Rica:-k0:097778,0b,0l797a,0m7b46,0n|0Coyhaique:-a0:007c4h,017d7e,037f7g,047h7e,0h7i4h,0i5r7e,0k7f7j,0l7h7e,0r7k5b,0s467l,0t5p7m,0u7d7e,127k5b,13464h,145p7m,157n7o,167p7q,19,1a7r7s,1d5n7t,1h5n7q,1j|0Phoenix,9GMT+7,0Creston:-nc|0Cuiaba:-dc:0f45,0g4647,0h4847,0i494a,0j4b4c,0k5u4t,0m5v47,0n5w5q,0o5x5y,0q4e4f,0s4g4h,0t4i4j,0v4k36,0w4l4m,0x4n,0y45,0z6m4a,106n6o,116p36,126q6r,165z4o,176s6t,1c4k6u,1d4l,1e|0Danmarkshavn:-a0:0a2f7u,0b7v7w,0q7x,0r|0Dawson:-u0:037y,04,0a3v3w,0h423w,114344,1e7z,1f|0Dawson_Creek:-qo:003v3w,0280,03|0Denver,MST7MDT:-nc:003v3w,043x3y,053z40,063v3w,0h423w,114344|0Detroit:-go:031b6k,043x3y,053v3w,0h423w,114344|0Edmonton:-nc:023v3w,0h423w,114344,1k7z,1l|0Eirunepe:-go:0f45,0g4647,0h4847,0i49,0j,0n5q,0o6m,0p,122j,13,1781,18|0El_Salvador:-k0:0h8283,0j|0Fort_Nelson:-qo:003v3w,0h423w,114344,197z,1a|0Fortaleza:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k4d,0l,0t6c,0u6d6e84,0v36,0w4l,0x|0Glace_Bay:-dc:023v3w,0h423w,114344|0Godthab:-a0:0a2f7u,0b7v7w,0q7v85,1h86,1i8788|0Goose_Bay:-dc:003v3w,0h898a,0i8b8c,0j8d8e,118f8g,158h8i,164344|0Grand_Turk:-go:093v3w,0h423w,114344,197z,1a,1c8j,1d4344|0Guatemala:-k0:038k,048l,05,0d8m8n,0e,0l8o8p,0m,103k3c,11|0Guayaquil:-go:0m8q,0n8r,0o|0Guyana:-ci:058s,06,0m8t,0n|0Halifax:-dc:003v3w,0h423w,114344|0Havana:-go:008u88,028u8v,058u88,084v8v,098w8x,0b8y8x,0g8z90,0k3b91,0l9293,0q9294,0s9596,0u9296,0y97,0z,1098,119996,139a96,159b9c,163b9d,179a9e|0Hermosillo:-nc:0q423w,0t|0Indiana/Knox:-k0:003v3w,043x3y,053z40,063v3w,0h423w,0l6x,0m,109f,114344|0Indiana/Marengo:-go:003v3w,04,053z40,06,106i9f,114344|0Indiana/Petersburg:-k0:003v3w,043x3y,053z40,069g07,0741,08,109f,119h,124344|0Indiana/Tell_City:-go:009i70,01,109f,114344|0Indiana/Vevay:-go:003v3w,03,106i9f,114344|0Indiana/Vincennes:-go:009i70,01,109f,119h,124344|0Indiana/Winamac:-go:009i70,01,109f,119j8j,124344|0Indianapolis:-go:009i70,01,106i9f,114344|0Inuvik:-qo:023v3w,099k6k,0a3v3w,0h423w,114344,1k7z,1l|0Iqaluit:-go:023v3w,0h423w,0t6h9l,0u6i,0v423w,114344|0Jamaica:-go:043x3y,053z40,063v3w,0e|0Jujuy:-a0:044p4q,05,0i4r,0j574c,0k9m7y,0l9n9o,0m4g4x,0n4y,0o,1151,1252,13|0Juneau:-qo:003v3w,043x3y,053z40,063v3w,0a,0b3v3w,0d419p,0e3v3w,0h423w,114344|0Kentucky/Monticello:-k0:003v3w,043x3y,053z40,063v3w,0h423w,0u6i,0v423w,114344|0Lima:-go:0g095g,0i,0k095g,0l,0o095g,0p|0Los_Angeles,PST8PDT:-qo:003v3w,043x3y,053z40,063v3w,0h423w,114344|0Louisville:-go:003v3w,04,053z40,063v3w,0h423w,114344|0Maceio:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k4d,0l,0p4c,0q4d,0r,0t6c,0u6d6e84,0v36,0w4l,0x|0Managua:-k0:0310,04,059q,06,099r9s,0b,0m9t9u9v,0n,0q16,0r,0z9w9x,10803m,11|0Manaus:-dc:0f45,0g4647,0h4847,0i49,0j,0n5q,0o6m,0p|0Martinique:-dc:0a3d2m,0b|0Matamoros:-k0:0i749y,0j,0q423w,0v603q,0w423w,144344|0Mazatlan:-nc:0q423w,0v603q,0w423w,1h|0Mendoza:-a0:044p4q,05,0i4r,0j574c,0k9m4c,0l4g4c,0m4g9z,0n4y,0o,0ya0a1,0z,1151,1252,13|0Menominee:-go:036k,043x3y,053z40,063v3w,0h423w,114344|0Merida:-k0:0b6v,0ca2,0d,0q423w,0v603q,0w423w,1h|0Metlakatla:-qo:003v3w,043x3y,053z40,063v3w,0e,19a3,1aa444,1c9h,1da5a6a7,1e4344|0Mexico_City:-k0:0q423w,0v603q,0w423w,1h|0Miquelon:-dc:0a08,0b,0h423w,114344|0Moncton:-dc:003v3w,03,043v3w,0h423w,0n8d8e,114344|0Monterrey:-k0:0i749y,0j,0q423w,0v603q,0w423w,1h|0Montevideo:-a0:0006a8,01,02a9aa,03,04abacadae,057k,06af,07agah,0857ai,09ajak,0a52,0b,0hal,0iaman,0j57ao,0k6p4o,0lapaq,0m4g4x,0nam,0o,0yar,0zasat,10auav,19aw,1a|0Toronto,0Nassau:-go:003v3w,0h423w,114344|0New_York,EST5EDT:-go:003v3w,043x3y,053z40,063v3w,0h423w,114344|0Nome:-10o:003v3w,043x3y,053z40,063v3w,0d41ax,0e3v3w,0h423w,114344|0Noronha:-6o:0f45,0g4647,0h4847,0i494a,0j4b4c,0k4d,0l,0t6c,0u6d6e6f,0v36,0w4l,0x|0North_Dakota/Beulah:-nc:003v3w,043x3y,053z40,063v3w,0h423w,11ay44,14az,154344|0North_Dakota/Center:-nc:003v3w,043x3y,053z40,063v3w,0h423w,0m6y,0n423w,114344|0North_Dakota/New_Salem:-nc:003v3w,043x3y,053z40,063v3w,0h423w,0x2f,0y423w,114344|0Ojinaga:-k0:0q423w,0s70,0t423w,0v603q,0w423w,144344,1g75,1h4344|0Paramaribo:-bo:0eb0,0f|0Port-au-Prince:-go:0db1b2,0e8u88,0ib33w,0s,0z9288,11,16ay44,1a,1b4344|0Porto_Velho:-dc:0f45,0g4647,0h4847,0i49,0j|0Punta_Arenas:-a0:007c4h,017d7e,037f7g,047h7e,0h7i4h,0i5r7e,0k7f7j,0l7h7e,0r7k5b,0s467l,0t5p7m,0u7d7e,127k5b,13464h,145p7m,157n7o,167p7q,19,1ab4b5,1b|0Rankin_Inlet:-k0:023v3w,0h423w,0u6i,0v6k,0w423w,114344|0Recife:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k4d,0l,0t6c,0u6d6e6f,0v36,0w4l,0x|0Regina,9GMT+6:-k0|0Resolute:-k0:023v3w,0h423w,0u6i,0v6k,0w423w,106i,118j,124344|0Rio_Branco:-go:0f45,0g4647,0h4847,0i49,0j,122j,13,1781,18|0Santarem:-dc:0f45,0g4647,0h4847,0i49,0j,122j,13|0Santiago:-a0:007c4h,017d7e,037f7g,047h7e,0h7i4h,0i5r7e,0k7f7j,0l7h7e,0r7k5b,0s467l,0t5p7m,0u7d7e,127k5b,13464h,145p7m,157n7o,167p7q,19,1a7r7s,1d5n7t,1hb67q|0Santo_Domingo:-f0:00b7b8,01b9ba,02bbbc,04bbaq,05,0u9fbd,0v|0Sao_Paulo:-a0:0f45,0g4647,0h4847,0i494a,0j4b4c,0k5u4t,0m5v47,0n5w5q,0o5x5y,0q4e4f,0s4g4h,0t4i4j,0v4k36,0w4l4m,0x4n5d,0y6l45,0z6m4a,106n6o,116p36,126q6r,165z4o,176s6t,1c4k6u,1d4l,1e|0Scoresbysund:-6o:0a2fbe,0bbfbg,0c95bh,0q9596,1ibi,1j8788|0Sitka:-qo:003v3w,043x3y,053z40,063v3w,0d419p,0e3v3w,0h423w,114344|0St_Johns:-bo:003v3w,0h898a,0i8b8c,0j8d8e,118f8g,158h8i,164344|0Swift_Current:-nc:0280,03|0Tegucigalpa:-k0:0h8283,0j,104vbj,11|0Thule:-dc:0l2qbk,0n423w,114344|0Tijuana:-qo:003v3w,043x3y,053z40,063v3w,0h423w,144344|0Vancouver:-qo:003v3w,0h423w,114344,1k7z,1l|0Whitehorse:-qo:0a3v3w,0h423w,114344,1e7z,1f|0Winnipeg:-k0:003v2s,0h422s,106i9f,114344,1k7z,1l|0Yakutat:-u0:003v3w,043x3y,053z40,063v3w,0h423w,114344|6Casey:qo:13bl,14bm,15bn,16bo,17,1abp,1b,1cbqbr,1dbsbt,1ebubv,1fbwbx,1hby,1i|6Davis:nc:13bz,14c0,15c1,16c2,17|4Port_Moresby,9GMT-10,6DumontDUrville,4Truk:xc|6Macquarie:10o:00c3c4,02c5c6,03c7c4,0cc8c4,0ec7c4,0gc9ca,0hcbc4,0lc8av,0ucccd,0vc8av,10cecf,11cgch,12ciav,14,15ciav|6Mawson:k0:13cj,14|4Auckland,6McMurdo:140:04ck,052ecl,06c7c4,0jcmcn,0kcoav,11cpcq,12cicr|6Palmer:-a0:044p4q,05,0c4q7m,0d5rcs,0h7i4h,0i5r7e,0k7f7j,0l7h7e,0r7k5b,0s467l,0t5p7m,0u7d7e,127k5b,13464h,145p7m,157n7o,167p7q,19,1ab4b5,1b|6Rothera:0:06ct,07|6Troll:0:0zcucv|6Vostok:nc:0ocwcx,0p,1hcy,1i|2Berlin,Arctic/Longyearbyen,2Copenhagen,2Oslo,2Stockholm:3c:0a2fbe,0b2q2r,0q2q2s|1Almaty:k0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0z,1i4g,1j|1Amman:6o:03d20s,04100s,061034,07d3d4,09,0fd50s,0gd6d7,0id5d8,0jd9da,0k0jdb,0ldcdd,0m9w9x,0nde0s,0od5df,0pd6dg,0tdh3l,0udidj,0wdkbg,0xdldm,0ydndo,0zd53p,10dpdq,16dr,17ds,18dpdq,1gdt,1h|1Anadyr:17c:0b3b3c,0c3c,0d3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,14du,151r,16|1Aqtau:go:0b5f,0c3c,0d3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0ndvdw,0o1rdx,0p1vdy,0q2q2s,0ydv,0z|1Aqtobe:go:0b3b,0c3c,0d3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0z|1Ashgabat:go:0b3b3c,0e3b1e,0f2q2r,0lcz,0md0,0n|1Atyrau:go:0b5f,0c3c,0d3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0tdu,0u2q2s,0ydv,0z|1Baghdad:a0:0c083c,0ddz3c,0e3b3c,0f3be0,0ge1bk,0le2e3,12|1Qatar,1Bahrain:dc:024z,03|1Baku:dc:0b3b3c,0e3b1e,0f2q2r,0lcz,0m1n,0n,0qe4e5,0re6e7,1a|1Bangkok,1Jakarta,9GMT-7,1Phnom_Penh,1Vientiane,8Christmas:nc|1Barnaul:nc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0p1ve8dy,0q2q2s,151r,16,1840,19,1a1r,1b|1Beirut:6o:02e93c,03083c,083k3h,09,0e08ea,0i2nea,0jebea,0k08ea,0m08ec,0n9583,0t9588|1Bishkek:k0:0b3b3c,0e3b1e,0f2q2r,0led,0mee83,0refeg,0zeh,10|1Kuching,1Makassar,9GMT-8,1Brunei:qo|1Calcutta:ic|1Chita:u0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,18ei,19,1a1r,1b|1Colombo:ic:0qejek,0r,10el,11|1Damascus:6o:00em65,07emen,09,0deo65,0f,0gepeq,0her07,0ies07,0jet65,0keu3q,0l3b3c,0mev3c,0n3oew,0o3b3c,0rex3c,0t3b3c,103b8n,11eyez,123ef0,13f1b2,14f2f3,16f4f3,1gf5,1h|1Dhaka:k0:13f616,14|1Dili:u0:06f7,07,0uf8,0v|1Dubai,9GMT-4,1Muscat,8Mahe,8Reunion:dc|1Dushanbe:k0:0b3b3c,0e3b1e,0f2q2r,0lf9,0m|1Famagusta:6o:05fafb,06fcfd,072xew,08feff,099283,0b9583,0sfgfh,0tfifj,1afk,1bfl,1cfifj|1Gaza:6o:04fmfn,05fofp,06,0afqfr,0b,0e02fs,0f2t2k,0gft8p,0hfufv,0ifwfx,0j3kfy,0kf5fz,0lg0g1,0nfeg2,0o3bg3,0pdzfy,0qf2dg,0tg4g5,0yg63m,0zfug7,103b8n,113bg8,1297g9,13f1ga,14gbgc,15gd2l,16eyge,17fg01,1897gf,1997gg,1aghgi,1dfg05,1e97dm,1ff198,1gf19f,1h1b6k,1i6740,1jgj70,1kgkgl,1u1ncj,1vdvgm,1w1r68,1x1vgn,1ygoengp3y,1z1jgqgr40,201ngsgt70,21dvgugvgw,221vgxgy9f,231zgzh06k,24goh1h23y,251jh3h440,26dvh5h6gw,271rh71t9y,281vh81x9f,291zh9ha6k,2a1jhbhc40,2b1nhd2570,2cdvhehfgw,2d299y,2e1b6k,2f2d3y,2g2f40,2hhggl,2qgohh,2r1jhi,2s1ng7,2tdvhj,2u1vhkhl9f,2v1zhmhn6k,2wgohohp3y,2x1jhqhr40,2ydvhshtgw,2z1rhu3s9y,301vhv1d9f,311zhwhx6k,321jhyhz40,331ni01p70,34dvi1i2gw,351ri3i49y,361zi5216k,37goi6233y,381ji7i840,39gkgl|1Hebron:6o:04fmfn,05fofp,06,0afqfr,0b,0e02fs,0f2t2k,0gft8p,0hfufv,0ifwfx,0j3kfy,0kf5fz,0lg0g1,0nfeg2,0o3bg3,0pdzfy,0qf2dg,0tg4g5,0yg63m,0zfug7,103b8n,113bg8,12972k,13f1ga,143ogc,15gd2li93h,16eyge,17fg01,1897gf,1997gg,1aghgi,1dfg05,1e97dm,1ff198,1gf19f,1h1b6k,1i6740,1jgj70,1kgkgl,1u1ncj,1vdvgm,1w1r68,1x1vgn,1ygoengp3y,1z1jgqgr40,201ngsgt70,21dvgugvgw,221vgxgy9f,231zgzh06k,24goh1h23y,251jh3h440,26dvh5h6gw,271rh71t9y,281vh81x9f,291zh9ha6k,2a1jhbhc40,2b1nhd2570,2cdvhehfgw,2d299y,2e1b6k,2f2d3y,2g2f40,2hhggl,2qgohh,2r1jhi,2s1ng7,2tdvhj,2u1vhkhl9f,2v1zhmhn6k,2wgohohp3y,2x1jhqhr40,2ydvhshtgw,2z1rhu3s9y,301vhv1d9f,311zhwhx6k,321jhyhz40,331ni01p70,34dvi1i2gw,351ri3i49y,361zi5216k,37goi6233y,381ji7i840,39gkgl|1Hong_Kong,1Macau:qo:00iaib,03icidie,04if,05igih,07,09iiid,0a|1Hovd:k0:0809,09,0d3b3c,0e3b3h,0f9583,0t,0v1fe0,0wijik,11,19ijil,1b|1Irkutsk:qo:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Tokyo,1Jayapura,4Palau,9GMT-9:u0|1Jerusalem:6o:04fmfn,05fofp,06,0afqfr,0b,0e02fs,0f2t2k,0gft8p,0hfufv,0ifwfx,0j3kfy,0kf5fz,0lg0g1,0nfeg2,0o3bg3,0pdzfy,0qimin,0rioip,0s9biq,0t6iir,0u2dis,0vit3l,0w3jiu,0xiviw,0yixiy,0zeueq,10et65,111jiz,12dvj0,131rj1,141vj2,15eu63,161jj3,17j43w|1Kabul:f0|1Kamchatka:140:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,14du,151r,16|1Karachi:go:0wj5j6,0x,122nf0,13fuf0,14|1Katmandu:ic:0gj7,0h|1Khandyga:u0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0y09dvdu,0z2q2s,151rfv,16,1840,19|1Krasnoyarsk:nc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Singapore,1Kuala_Lumpur:p0:0bj8,0c|1Magadan:10o:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,18ei,19,1a41,1b|1Manila:qo:07978n,08,0k8mj9,0l|1Nicosia:6o:05fafb,06fcfd,072xew,08feff,099283,0b9583,0sfgfh,0tfifj|1Novokuznetsk:nc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,14du,151r,16|1Novosibirsk:nc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0ndva0dw,0o2q2r,0q2q2s,151r,16,1840,19,1aja,1b|1Omsk:k0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Oral:go:0b3b,0c3c,0d3b3c,0e3b1e,0f2q2r,0jdy,0k2q2r,0md0d1,0n2q2r,0q2q2s,0ydv,0z|1Pontianak:qo:0i3a,0j|1Pyongyang:u0:19jb,1a,1cjc,1d|1Qostanay:go:0b3b,0c3c,0d3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0ydv,0z,1i4g,1j|1Qyzylorda:go:0b3b,0c3c,0d3b3c,0e3b1e,0f2q2r,0l,0md0d1,0n2q2r,0q2q2s,0ydv,0z,1cjd,1d|1Rangoon,8Cocos:lo|1Saigon:qo:05je,06|1Sakhalin:10o:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0qet1i,0r1m,0s2q2s,151r,16,1840,19,1a1r,1b|1Samarkand:go:0b3b,0c3c,0d3b3c,0e3b1e,0f2q2r,0m|1Seoul:u0:0hjfjg,0j|1Shanghai:qo:0gjhji,0hjjjk,0m|1Srednekolymsk:10o:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Taipei:qo:043b3c,06,09jl3c,0a|1Tashkent:k0:0b3b3c,0e3b1e,0f2q2r,0lcz,0m|1Tbilisi:dc:0b3b3c,0e3b1e,0f2q2r,0lcz,0m9583,0of1,0p3ojm,0qdz,0r05,0s9588,0y97jndu,0z1r,10|1Tehran:bo:07jojp,08f5jqjr,09jsjt,0aioju,0b,0ljv8n,0mjw8n,0qiojx,0rjw8n,0uiojx,0vjw8n,0yiojx,0zjw8n,10,12iojx,13jw8n,16iojx,17jw8n,1aiojx,1bjw8n,1eiojx,1fjw8n,1h|1Thimphu:ic:0hb0,0i|1Tomsk:nc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,0wetjy1i,0x2q2s,151r,16,1840,19,1ajz,1b|1Ulaanbaatar:nc:0809,09,0d3b3c,0e3b3h,0f9583,0t,0v1fe0,0wijik,11,19ijil,1b|1Urumqi,9GMT-6:k0|1Ust-Nera:u0:0bk03c,0c3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151rfv,16,1840,19|1Vladivostok:xc:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Yakutsk:u0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Yekaterinburg:go:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|1Yerevan:dc:0b3b3c,0e3b1e,0f2q2r,0lcz,0m2q2r,0p1v,0q,0r2q2s,16|7Azores:-3c:0c95bh,0mfgbgk1,0nivk2k3,0o95bh,0q9596|7Bermuda:-dc:043v3w,0h423w,114344|7Canary:0:0a3dk4,0be1bk,0qe13w|7Cape_Verde:-6o:05k5,06|7Faeroe:0:0be1bk,0qe13w|7Madeira:0:0c3ek3,0d95bh,0geyk4,0he1bk,0qe13w|7South_Georgia,9GMT+2:-6o|7Stanley:-dc:0d08k6,0ek77j,0fk8,0gk9ka,0vkb3u,15|5Adelaide:vo:01kc,02c5c6,03c7c4,0gkdke,0icbc4,0lkfkg,0mkhki,0nkjkc,0okkc4,0qc8c4,10cec6,11cgkl,12ciav|5Brisbane:xc:01kc,02c5,03,0jc6,0kc7c4,0mkm,0n|5Broken_Hill:vo:01kc,02c5c6,03c7c4,0cknkc,0dc7c4,0gkdke,0icbc4,0kc7c4,0qc8c4,10cec6,11cgkl,12ciav|5Darwin:vo|5Eucla:t6:04kg,05c9,06,0dax,0eko,0f,0lkp,0mkm,0n,10kq,11c8c4,13kr,14|5Hobart:10o:00c3c4,02c5c6,03c7c4,0cc8c4,0ec7c4,0gc9ca,0hcbc4,0lc8av,0ucccd,0vc8av,10cecf,11cgch,12ciav|5Lindeman:xc:01kc,02c5,03,0jc6,0kc7c4,0oks,0p|5Lord_Howe:xc:0bktki,0ckuc4,0fkvkw,0gkxky,0ikzl0,0kl1l0,0ql2l0,0ul3l4,0vl2l0,10l5l6,11l7l8,12l9la|5Melbourne:xc:01kc,02c5c6,03c7c4,0gkdlb,0icbc4,0lc7c4,0pc8c4,0ucccd,0vc8c4,10cec6,11cgkl,12ciav|5Perth:qo:04kg,05c9,06,0dax,0eko,0f,0lkp,0mkm,0n,10kq,11c8c4,13kr,14|5Sydney:xc:01kc,02c5c6,03c7c4,0cknkc,0dc7c4,0gkdke,0icbc4,0kc7c4,0qc8c4,0ucccd,0vc8c4,10cec6,11cgkl,12ciav|2Brussels,2Amsterdam,2Luxembourg:3c:0774lc,086i0b,09422r,0b2q2r,0q2q2s|2Andorra:3c:0f2q2r,0q2q2s|2Astrakhan:dc:0b3b3c,0e3b1e,0f2q2r,0jdy,0k1z1e,0let,0md1,0n2q2r,0q2q2s,151r,16,1840,19,1a1r,1b|2Athens:6o:05ldle,06lflg,0774dw,086ilh,09lie0,0a3b2m,0bfilj,0qfifj|2Belgrade,2Ljubljana,2Podgorica,2Sarajevo,2Skopje,2Zagreb:3c:0d2q2r,0q2q2s|2Prague,2Bratislava:3c:09422r,0b2q2r,0q2q2s|2Bucharest:6o:09lk3h,0alllm,0b2q2r,0l95bh,0o9583,0qdzbi,0rfifj|2Budapest:3c:0a3dlm,0b95bh,0e2q2r,0q2q2s|2Zurich,2Busingen,2Vaduz:3c:0b2q2r,0q2q2s|2Chisinau:a0:0b3b3c,0e3b1e,0f2q2r,0k1zh91e,0letcz,0m9583,0qdzbi,0r2q2s,1gfifj|2London,2Dublin,2Guernsey,2Isle_of_Man,2Jersey:3c:01du,02lnlo,0be1lp,0pe1lq,0te13w|2Gibraltar:3c:0c2q2r,0q2q2s|2Helsinki,2Mariehamn:6o:0b2q2r,0dfilj,0qfifj|2Istanbul:6o:03lr8j,04eta7,052ha2,06ls07,0774lt,086i,09,0dlu63,0ea3,0flvk4,0ge1bk,0olwbk,0qe13w,11fifj,15lxly,16fifj,18lzm0,19m1m2,1afk,1b|2Kaliningrad:a0:0b3b3c,0e3b1e,0f2q2r,0jdy,0k2q2r,0q2q2s,151r,16,1840,19|2Kiev:a0:0b3b3c,0e3b1e,0f2q2r,0k1zm3,0lcz,0m2q2r,0qetm4,0rfifj|2Kirov:dc:0b3b3c,0e3b1e,0f2q2r,0jdy,0k1z1e,0let,0md1,0n2q2r,0q2q2s,151r,16,1840,19|2Lisbon:3c:06k3,07f13n,08m565,09b3bk,0b95bh,0ge1bk,0m3j,0n2q2r,0q3y,0re13w|2Madrid:3c:04m6m7,06m8k3,07m93n,086i0b,09422r,0b2q2r,0q2q2s|2Malta:3c:00mabg,01mbk3,02mc3m,03dzmd,04memf,05mgmh,0aetmi,0b2q2r,0q2q2s|2Minsk:a0:0b3b3c,0e3b1e,0f2q2r,0k,0lcz,0m2q2r,0q2q2s,151r,16|2Paris,2Monaco:3c:06ivk3,0774lc,086i0b,09422r,0b2q2r,0q2q2s|2Moscow:a0:0b3b3c,0e3b1e,0f2q2r,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19|2Riga:a0:0b3b3c,0e3b1e,0f2q2r,0jdy,0k2q2r,0rfifj,0u,0vfifj|2Rome,2San_Marino,2Vatican:3c:00mjbg,01mkk3,02ml3m,03mm3p,04mnbh,07mo3n,08ml3m,09mp3p,0a2fbe,0b2q2r,0q2q2s|2Samara:dc:0b3b3c,0e3b1e,0f2q2r,0jdy,0k1z1e,0lmq,0m2q2r,0q2q2s,14du,151r,16|2Saratov:dc:0b3b3c,0e3b1e,0f2q2r,0ilc,0j2q2r,0let,0md1,0n2q2r,0q2q2s,151r,16,1840,19,1amr,1b|2Simferopol:a0:0b3b3c,0e3b1e,0f2q2r,0km3,0l,0m2q2r,0o1r08lc,0p1vdy,0qdzm4,0rm0,0sfifj,18ms40,19|2Sofia:6o:09mt3m,0allmd,0bmuj1,0cmvdw,0d2q2r,0l9583,0qdzbi,0rfifj|2Tallinn:a0:0b3b3c,0e3b1e,0f2q2r,0jdy,0k2q2r,0q2q2s,0s1nfh,0tlxmw,0u,0wfifj|2Tirane:3c:04mx9x,05109x,06mymz,08n0n1,0an2n3,0bn4dd,0c17n5,0dn60s,0ed51e,0f2q2r,0q2q2s|2Ulyanovsk:dc:0b3b3c,0e3b1e,0f2q2r,0jdy,0k1z1e,0lcz,0md01nd1,0n2q2r,0q2q2s,151r,16,1840,19,1a1r,1b|2Vienna:3c:0a3d2m,0b2q2r,0q2q2s|2Vilnius:a0:0b3b3c,0e3b1e,0f2q2r,0jdy,0k2q2r,0q2q2s,0s1q,0tdv,0u,0xfifj|2Volgograd:dc:0b3b3c,0e3b1e,0f2q2r,0ilc,0j2q2r,0let,0md1,0n2q2r,0q2q2s,151r,16,1840,19,1ckl,1d,1en7,1f|2Warsaw:3c:07n868,08m565,09b3bk,0be1bk,0i2q2r,0q2q2s|8Chagos:go:0q09,0r|8Maldives,9GMT-5,8Kerguelen:go|8Mauritius:dc:0c7m,0d3r,0e,12cl,13n9,14|4Apia:-10o:14a1,15nanbnc,16ndne,1fnf,1g|4Bougainville:xc:18ng,19|4Chatham:16i:04nh,05ninj,06nknl,0jnmnn,0knonp,11nqnr,12nsnt|4Easter:-k0:00nunv,01nwnx,03nynz,04o0nx,0co1,0do2o3,0ho4o5,0io2o6,0ko7o8,0lo9o6,0roaob,0socod,0toeo1,0uofo6,12oaob,13oco5,14oeo1,15ogoh,16oioj,19,1aokol,1domon,1homoj|4Efate:10o:03oo,045j,05,0dk6,0eopoq,0mor47,0nos,0o|4Enderbury:-140:095f,0a,0oot,0p|4Fakaofo:-10o:15nc,16|4Fiji:140:0sou,0tovow,0uc5,0v,13ox,14oyoz,15ksp0,16p1p2,18p3p4,19p5p6,1dp7p8,1ep9pa,1fpb,1g|4Tarawa,9GMT-12,4Funafuti,4Majuro,4Wake,4Wallis:140|4Galapagos:-go:0g3a,0h,0m8q,0n8r,0o|4Gambier,9GMT+9:-u0|4Guadalcanal,9GMT-11,4Ponape:10o|4Guam,4Saipan:xc:003vpc,02,03pd,04pe,05,06pfpg,0741ph,08|4Honolulu,4Tahiti,9GMT+10:-xc|4Kiritimati:-zk:09pi,0a,0oot,0p|4Kosrae:140:0t3a,0u|4Kwajalein:-140:0npj,0o|4Marquesas:-vo|4Niue,4Pago_Pago,9GMT+11,4Midway:-10o|4Nauru:12c:09pk,0a|4Norfolk:12c:04kg,05c9,06,19pl,1a,1dhp,1eciav|4Noumea:10o:07ah,086dpm,096d,0a,0qpn,0rc9,0s|4Pitcairn:-sc:0spo,0t|4Rarotonga:-z0:08pp,09pqbc,0lpr,0m|4Tongatapu:17c:0tch,0u2aps,0vptpu,0wpv,0x,1apw,1bpx,1c|9GMT-14:1ao|9GMT-13:17c|9GMT+1:-3c|9GMT+8:-qo|9GMT+12:-140';
  const TZ_LINKS = '3Asmara=h 3Timbuktu=3 0Argentina/Buenos_Aires=2y 0Argentina/Catamarca=33 0Argentina/ComodRivadavia=33 0Argentina/Cordoba=3e 0Argentina/Jujuy=4h 0Argentina/Mendoza=4u 0Atikokan=39 0Atka=1t 0Ensenada=5x 0Fort_Wayne=4d 0Indiana/Indianapolis=4d 0Kentucky/Louisville=4n 0Knox_IN=46 0Montreal=53 0Nipigon=53 0Nuuk=3x 0Pangnirtung=4f 0Porto_Acre=5m 0Rainy_River=60 0Rosario=3e 0Santa_Isabel=5x 0Shiprock=3o 0Thunder_Bay=53 0Virgin=2e 0Yellowknife=3r 6South_Pole=6b 1Ashkhabad=6q 1Choibalsan=8u 1Chongqing=8m 1Chungking=8m 1Dacca=7d 1Harbin=8m 1Ho_Chi_Minh=8i 1Istanbul=ae 1Kashgar=8v 1Kathmandu=80 1Kolkata=79 1Macao=7p 1Tel_Aviv=7w 1Thimbu=8s 1Ujung_Pandang=76 1Ulan_Bator=8u 1Yangon=8g 7Faroe=96 7Jan_Mayen=6h 5ACT=9l 5Canberra=9l 5Currie=9g 5LHI=9i 5NSW=9l 5North=9e 5Queensland=9c 5South=9b 5Tasmania=9g 5Victoria=9j 5West=9k 5Yancowinna=9d Brazil/Acre=5m Brazil/DeNoronha=58 Brazil/East=5q Brazil/West=4q CET=9m Canada/Atlantic=43 Canada/Central=60 Canada/Eastern=53 Canada/Mountain=3r Canada/Newfoundland=5t Canada/Pacific=5y Canada/Saskatchewan=5j Canada/Yukon=5z Chile/Continental=5o Chile/EasterIsland=bc Cuba=44 EET=9r EST=36 Egypt=1h Eire=a7 2Belfast=a6 2Kyiv=ag 2Nicosia=87 2Tiraspol=a5 2Uzhgorod=ag 2Zaporozhye=ag GB=a6 GB-Eire=a6 HST=bv Hongkong=7o Iceland=b Iran=8r Israel=7w Jamaica=4g Japan=7s Kwajalein=c0 Libya=1q MET=9m MST=3h Mexico/BajaNorte=5x Mexico/BajaSur=4t Mexico/General=4y NZ=6a NZ-CHAT=bb Navajo=3o PRC=8m 4Chuuk=67 4Johnston=bv 4Kanton=be 4Pohnpei=bs 4Samoa=c3 4Yap=67 Poland=b3 Portugal=ai ROC=8o ROK=8l Singapore=83 Turkey=ae US/Alaska=1u US/Aleutian=1t US/Arizona=3h US/Central=3a US/East-Indiana=4d US/Eastern=55 US/Hawaii=bv US/Indiana-Starke=46 US/Michigan=3q US/Mountain=3o US/Pacific=4l US/Samoa=c3 W-SU=ao WET=ai ACT=9e AET=9l AGT=2y ART=1h AST=1u BET=5q BST=7d CAT=16 CNT=5t CST=3a CTT=8m EAT=d ECT=am IET=4d IST=79 JST=7s MIT=b9 NET=91 NST=6a PLT=7z PNT=3h PRT=1w PST=4l SST=bq VST=8i';
  // Zone names, as CLDR has them and Chrome 157 prints them.
  // TZ_METAZONES: 'English~German~zones~short names~base' per metazone.
  // Long names are 'generic;standard;daylight', '=name' for a standard
  // name alone, or a stem and * for the usual three (ZONE_WORDS), *S
  // being English summer rather than daylight time. The zones are the
  // one whose clock the metazone's generic name means, then others for
  // regions ('CA0Toronto'). Short names are 'standard,daylight,generic'
  // after a letter and a colon: e for English, c for Canada's alone, g
  // for British English, d for German. A base is the number of the
  // metazone a zone's own names (London's British Summer Time) stand in
  // for.
  // TZ_ERAS: the metazones a zone has been in, each as the metazone's
  // number in two digits, the sign of its standard offset (! if unknown)
  // and the offset, then @ and the second it starts unless it is the
  // first. Nothing before the @ is a time in no metazone, and -- for the
  // number one in no metazone whose standard offset matters all the same.
  // TZ_ZONE_ERAS: two digits per name in TZ_ZONES, the number of its
  // entry in TZ_ERAS.
  const TZ_METAZONES = 'Acre*S~Acre-*~0Rio_Branco|=Afghanistan Time~=Afghanistan-Zeit|=Central Africa Time~=Zentralafrikanische Zeit|=East Africa Time~=Ostafrikanische Zeit|=South Africa Standard Time~=Südafrikanische Zeit|=West Africa Time~=Westafrikanische Zeit|Alaska*D~Alaska-*~0Juneau~e:AKST,AKDT,AKT|Amazon*S~Amazonas-*~0Manaus|Central*D~Nordamerikanische Zentralzeit;Nordamerikanische Zentral-Normalzeit;Nordamerikanische Zentral-Sommerzeit~0Chicago,CA0Winnipeg~e:CST,CDT,CT|Eastern*D~Nordamerikanische Ostküstenzeit;Nordamerikanische Ostküsten-Normalzeit;Nordamerikanische Ostküsten-Sommerzeit~0New_York,CA0Toronto~e:EST,EDT,ET|Mountain*D~Rocky-Mountains-*~0Denver,CA0Edmonton~e:MST,MDT,MT|Pacific*D~Nordamerikanische Westküstenzeit;Nordamerikanische Westküsten-Normalzeit;Nordamerikanische Westküsten-Sommerzeit~0Los_Angeles,CA0Vancouver~e:PST,PDT,PT|Anadyr*S~Anadyr *~1Anadyr|Samoa*D~Apia-*~4Apia|Arabian*D~Arabische *~1Riyadh|Argentina*S~Argentinische *~0Buenos_Aires|Western Argentina*S~Westargentinische *~0Argentina/San_Luis|Armenia*S~Armenische *~1Yerevan|Atlantic*D~Atlantik-*~0Halifax~e:AST,ADT,AT|Australian Central*D~Zentralaustralische *~5Adelaide|Australian Central Western*D~Zentral-/Westaustralische *~5Eucla|Australian Eastern*D~Ostaustralische *~5Sydney|Australian Western*D~Westaustralische *~5Perth|Azerbaijan*S~Aserbaidschanische Zeit;Aserbeidschanische Normalzeit;Aserbaidschanische Sommerzeit~1Baku|Azores*S~Azoren-*~7Azores|Bangladesh*S~Bangladesch-*~1Dhaka|=Bhutan Time~=Bhutan-Zeit|=Bolivia Time~=Bolivianische Zeit|Brasilia*S~Brasília-*~0Sao_Paulo|=Brunei Time~=Brunei-Darussalam-Zeit|Cape Verde*S~Cabo-Verde-*~7Cape_Verde|=Chamorro Standard Time~=Chamorro-Zeit|Chatham*D~Chatham-*~4Chatham|Chile*S~Chilenische *~0Santiago|China*D~Chinesische *~1Shanghai|=Christmas Island Time~=Weihnachtsinsel-Zeit|=Cocos Islands Time~=Kokosinseln-Zeit|Colombia*S~Kolumbianische *~0Bogota|Cook Islands Time;Cook Islands Standard Time;Cook Islands Summer Time~Cookinseln-*~4Rarotonga|Cuba*D~Kubanische *~0Havana|=Davis Time~=Davis-Zeit|=Dumont d’Urville Time~=Dumont-d’Urville-Zeit|=Timor-Leste Time~=Osttimor-Zeit|Easter Island*S~Osterinsel-*~4Easter|=Ecuador Time~=Ecuadorianische Zeit|Central European*S~Mitteleuropäische *~2Paris,DE2Berlin~g:CET,CEST,CET;d:MEZ,MESZ,MEZ|Eastern European*S~Osteuropäische *~2Bucharest~g:EET,EEST,EET;d:OEZ,OESZ,OEZ|=Further-eastern European Time~=Kaliningrader Zeit|Western European*S~Westeuropäische *~7Canary~g:WET,WEST,WET;d:WEZ,WESZ,WEZ|Falkland Islands*S~Falklandinseln-*~7Stanley|Fiji*S~Fidschi-*~4Fiji|=French Guiana Time~=Französisch-Guayana-Zeit|=French Southern & Antarctic Time~=Französische-Süd-und-Antarktisgebiete-Zeit|=Galapagos Time~=Galapagos-Zeit|=Gambier Time~=Gambier-Zeit|Georgia*S~Georgische *~1Tbilisi|=Gilbert Islands Time~=Gilbert-Inseln-Zeit|=Greenwich Mean Time~=Mittlere Greenwich-Zeit~~e:GMT;g:GMT|Greenland*S~;;~0Godthab|East Greenland*S~Ostgrönland-*~0Scoresbysund|West Greenland*S~Westgrönland-*~0Godthab|=Guam Standard Time~=Guam-Zeit|=Gulf Standard Time~=Golf-Zeit~~g:GST|=Guyana Time~=Guyana-Zeit|Hawaii-Aleutian*D~Hawaii-Aleuten-*~4Honolulu~e:HAST,HADT,HAT|Hong Kong*S~Hongkong-*~1Hong_Kong|Khovd*S~Chowd-*~1Hovd|=India Standard Time~=Indische Normalzeit|=Indian Ocean Time~=Indischer-Ozean-Zeit|=Indochina Time~=Indochina-Zeit|=Central Indonesia Time~=Zentralindonesische Zeit|=Eastern Indonesia Time~=Ostindonesische Zeit|=Western Indonesia Time~=Westindonesische Zeit|Iran*D~Iranische *~1Tehran|Irkutsk*S~Irkutsker *~1Irkutsk|Israel*D~Israelische *~1Jerusalem|Japan*D~Japanische *~1Tokyo|Kamchatka*S~Kamtschatka-*~1Kamchatka|=Kazakhstan Time~=Kasachische Zeit|=East Kazakhstan Time~=Ostkasachische Zeit|=West Kazakhstan Time~=Westkasachische Zeit|Korean*D~Koreanische *~1Seoul|=Kosrae Time~=Kosrae-Zeit|Krasnoyarsk*S~Krasnojarsker *~1Krasnoyarsk|=Kyrgyzstan Time~=Kirgisische Zeit|=Line Islands Time~=Linieninseln-Zeit|Lord Howe*D~Lord-Howe-*~5Lord_Howe|Macao*S~Macau-*~1Macau|Magadan*S~Magadan-*~1Magadan|=Malaysia Time~=Malaysische Zeit|=Maldives Time~=Malediven-Zeit|=Marquesas Time~=Marquesas-Zeit|=Marshall Islands Time~=Marshallinseln-Zeit|Mauritius*S~Mauritius-*~8Mauritius|=Mawson Time~=Mawson-Zeit|Mexican Pacific*D~Mexikanische Pazifikzeit;Mexikanische Pazifik-Normalzeit;Mexikanische Pazifik-Sommerzeit~0Mazatlan|Ulaanbaatar*S~Ulaanbaatar-*~1Ulaanbaatar|Moscow*S~Moskauer *~2Moscow|=Myanmar Time~=Myanmar-Zeit|=Nauru Time~=Nauru-Zeit|=Nepal Time~=Nepalesische Zeit|New Caledonia*S~Neukaledonische *~4Noumea|New Zealand*D~Neuseeland-*~4Auckland|Newfoundland*D~Neufundland-*~0St_Johns~c:NST,NDT,NT|=Niue Time~=Niue-Zeit|Norfolk Island*D~Norfolkinsel-*~4Norfolk|Fernando de Noronha*S~Fernando-de-Noronha-*~0Noronha|Novosibirsk*S~Nowosibirsker *~1Novosibirsk|Omsk*S~Omsker *~1Omsk|Pakistan*S~Pakistanische *~1Karachi|=Palau Time~=Palau-Zeit|=Papua New Guinea Time~=Papua-Neuguinea-Zeit|Paraguay*S~Paraguayische *~0Asuncion|Peru*S~Peruanische *~0Lima|Philippine*S~Philippinische *~1Manila|=Phoenix Islands Time~=Phoenixinseln-Zeit|St. Pierre & Miquelon*D~St.-Pierre-und-Miquelon-*~0Miquelon~c:PMST,PMDT,PMT|=Pitcairn Time~=Pitcairninseln-Zeit|=Pohnpei Time~=Pohnpei-Zeit|=North Korea Time~=Pjöngjang-Zeit|=Réunion Time~=Réunion-Zeit|=Rothera Time~=Rothera-Zeit|Sakhalin*S~Sachalin-*~1Sakhalin|Samara*S~Samara-*~2Samara|American Samoa*D~Samoa-*~4Pago_Pago|=Seychelles Time~=Seychellen-Zeit|=Singapore Standard Time~=Singapurische Normalzeit|=Solomon Islands Time~=Salomonen-Zeit|=South Georgia Time~=Südgeorgische Zeit|=Suriname Time~=Suriname-Zeit|=Syowa Time~=Syowa-Zeit|=Tahiti Time~=Tahiti-Zeit|Taiwan*D~Taipeh-*~1Taipei|=Tajikistan Time~=Tadschikische Zeit|=Tokelau Time~=Tokelau-Zeit|Tonga*S~Tongaische *~4Tongatapu|=Chuuk Time~=Chuuk-Zeit|Turkmenistan*S~Turkmenische *~1Ashgabat|=Tuvalu Time~=Tuvalu-Zeit|Uruguay*S~Uruguayische *~0Montevideo|Uzbekistan*S~Usbekische *~1Tashkent|Vanuatu*S~Vanuatu-*~4Efate|=Venezuela Time~=Venezuela-Zeit|Vladivostok*S~Wladiwostoker *~1Vladivostok|Volgograd*S~Wolgograder *~2Volgograd|=Vostok Time~=Wostok-Zeit|=Wake Island Time~=Wake-Insel-Zeit|=Wallis & Futuna Time~=Wallis-und-Futuna-Zeit|Yakutsk*S~Jakutsker *~1Yakutsk|Yekaterinburg*S~Jekaterinburger *~1Yekaterinburg|=Yukon Time~=Yukon-Zeit|Almaty*S~Almaty-*~1Almaty|Aqtau*S~Aqtau-*~1Aqtau|Aqtobe*S~Aqtöbe-*~1Aqtobe|=Casey Time~=Casey-Zeit|=Lanka Time~=Sri-Lanka-Zeit|=Northern Mariana Islands Time~=Nördliche-Marianen-Zeit|Kyzylorda*S~Quysylorda-Zeit;Quysylorda-Normalzeit;Qysylorda-Sommerzeit~1Qyzylorda|Türkiye*S~Türkische *~2Istanbul|;Greenwich Mean Time;British Summer Time~;Mittlere Greenwich-Zeit;Britische Sommerzeit~~g:GMT,BST~1l|;Greenwich Mean Time;Irish Standard Time~;Mittlere Greenwich-Zeit;Irische Sommerzeit~~~1l|Hawaii-Aleutian*D~Hawaii-Aleuten-*~~e:HST,HDT,HST~1s';
  const TZ_ERAS = '|1l+0|03+a0|0e+a0|3m+a0|1c+0,19+3c@42lp80,1c+0@54et80,19+3c@5wuyo0|05+3c|04+6o|02+6o|1a+6o|1c+0,19+3c@7eveo0,1c+0@8cm580,--+0@pha800,1c+0@tln1g0|1c+0,19+3c@7eveo0|,1c+0@3a22s0,---3c@pha800,1c+0@tln1g0|02+6o,03+a0@fodfs0,02+6o@qnth00|02+6o,03+a0@fodfs0,02+6o@oyph00|,1l+0@11v0r0|1l+0,05+3c@p1uqs0,1l+0@pkmo40|1a+6o,19+3c@69gig0,1a+6o@am3h80,19+3c@dyil40,1a+6o@ehhx40,19+3c@md8w00,1a+6o@mv76o0|19+3c|04+6o,02+6o@ajtx40,05+3c@cmzh40,02+6o@oyaqg0|,1s-xc@79e140|,06-u0@79dyc0|0r-dc|0i-dc|0s-a0|0f-a0,0g-dc@b1l480,0f-a0@b51cg0,0f-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hym0c0,0f-a0@hzl9s0|0f-a0,0f-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hym0c0,0f-a0@hzl9s0|0f-a0,@b1otk0,0f-a0@bdkr40,0f-6o@fj0b00,0f-a0@fqtsc0|0f-a0,0g-dc@b1l480,0f-a0@b51cg0,0f-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hyk5o0,0f-a0@i1e340|0f-a0,0g-dc@ajh9k0,0f-a0@b6bn40,0g-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hyk5o0,0f-a0@i1e340,0g-dc@juz1k0,0f-a0@krc0g0|0f-a0,@b1otk0,0f-a0@bdkr40,0f-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hym0c0,0f-a0@hz8b40|0f-a0,0f-6o@fj0b00,0f-a0@fqtsc0,@hyib00,0f-a0@hzl9s0|34-dc,34-a0@1fnkg0,34-dc@27sgc0,34-a0@sldn00|0a-nc,08-k0@l0ch00|08-k0|07-dc|11-go|0a-nc|0f-a0,0f-6o@fj0b00,0f-a0@fqtsc0|0a-nc,08-k0@fkggs0,09-go@g36go0,08-k0@g3jck0,0a-nc@gb3vo0|08-k0,09-go@696680,08-k0@6sej80,09-go@eina00,08-k0@ex1so0,09-go@nj3280|3y-dc,3y-f0@jsrss0,3y-dc@o6hks0|0f-a0,@b1otk0,0f-a0@bdkr40,0f-6o@fj0b00,0f-a0@fqtsc0,0g-dc@hym0c0,0f-a0@hzl9s0|1f-a0|09-go|08-k0,2n-nc@eqxno0,08-k0@rkk0s0|08-k0,2n-nc@eqxno0,08-k0@rkk0s0,0a-nc@rm5co0|0x-dc,@stej00|1o-a0,1l+0@dkhf00|,0b-qo@1ztvo0,46-nc@qj3vg0|0b-qo,0a-nc@1e0p00|00-go,07-dc@k2yb80,00-go@mw14g0|0b-qo,0a-nc@nkw140|1o-a0,1o-6o@rs3s40,1m-6o@saxk40|0i-dc,@9iylto,0i-dc@9trc9o|09-go,0i-dc@nkvss0,09-go@p5ezg0|18-go|1r-ci,1r-a0@2wud30,1r-dc@bluwg0|13-go|2n-nc|08-k0,09-go@bdxvc0,08-k0@ix3240|09-go,08-dc@23fcs0,09-go@2ijq00|08-k0,09-go@432wo0,08-k0@ix3240,09-go@jqywo0|09-go,08-k0@ix3240|09-go,08-k0@ix3240,09-go@jqywo0|09-go,08-k0@ix3240,09-go@jeqbk0|0b-qo,0a-nc@4v6bs0|09-go,08-k0@fkge00,09-go@g36go0|0f-a0,@aiyqw0,0f-a0@bcuts0,0f-6o@fj0b00,0f-a0@fqtsc0|0b-qo,@5dweg0,0b-qo@5n9fs0,@77sjo0,06-u0@79dyc0|08-k0,09-go@g36go0|35-go|0b-qo|08-k0,09-go@1qkbc0,08-k0@2ob1w0,09-go@bhceg0,08-k0@bv2gk0,09-go@c05vc0,08-k0@e3bck0|0f-a0,@aiyqw0,0f-a0@bwats0,0f-6o@fj0b00,0f-a0@fqtsc0,@hy5cc0,0f-a0@i4mr40|09-go,08-k0@1qgos0|08-k0,09-go@696680,08-k0@6p60s0|0b-qo,06-u0@nx4rs0,0b-qo@phnvo0,06-u0@plmjs0|0i-dc,38-a0@5e3cg0|3v-a0|2y-6o|0a-nc,08-k0@lbi640|0a-nc,08-k0@bwo0s0|0a-nc,08-k0@hncrg0|08-k0,0a-nc@eqxno0,08-k0@rkk3k0|,3l-bo@32jwe0,3l-a0@7p4720|0x-dc,@ohmt80|08-k0,09-go@g36go0,08-k0@gb3sw0|08-k0,09-go@g36go0,08-k0@gb3sw0,09-go@j7vvc0,08-k0@jeqbk0|07-dc,0s-a0@k2y8g0|0x-dc|,0i-dc@2ijn80,09-go@g36go0,0i-dc@g4za00|,1n-3c@5v6080,1m-6o@sb6tg0|0b-qo,@77sjo0,06-u0@79dyc0|2v-bo|0a-nc,08-k0@17qro0|0b-qo,46-nc@qj3vg0|0m+qo,4a+10o@kro7c0,0m+qo@kyrj00,4a+10o@ltqko0,0m+qo@lzr5w0,4a+10o@ofen40,0m+qo@p5dwk0,4a+10o@pg70w0,0m+qo@pogv40,4a+10o@pytbg0,0m+qo@q6tz40,4a+10o@qhmv5o,0m+qo@qpws40,4a+10o@r0cxto,0m+qo@r8mus0,4a+10o@rj30ho,0m+qo@rr7ls0|14+nc,14+go@kroa40,14+nc@kz30w0,14+go@ltqng0,14+nc@lzre80|33+xc|15+xc|3s+xc|0l+xc|2m+k0,2m+go@krocw0|2u+140|0f-a0,0x-dc@6fn4c0,@ohn4c0|3d+0,3d-a0@3lxs00|41+nc,41+0@cki780,41+nc@cykao0,41+go@s5trg0|47+k0,47+go@b347k0,47+k0@bi8qc0,27+k0@i6ey80,26+go@s9mq00|1a+6o,@rkfmg0|0c+17c,0c+140@6e2bw0,0c+10o@b33qw0,0c+140@bi89o0,2g+10o@kzy1k0,25+140@lio700|--+go,48+dc@bghm40,48+go@bi8t40,48+dc@cwnjo0,28+go@i6f3s0,26+go@s9mq00|--+go,49+dc@bghm40,49+go@bi8t40,28+go@i6f100,26+go@s9mq00|,3t+go@bi8t40|--+k0,28+go@i6f3s0,26+go@s9mq00|1q+dc,0e+a0@19d0w0|,0n+a0@b34d40,0n+dc@bv7jw0|1x+nc|20+nc|0z+nc|,2b+nc@o4nww0|2c+go,2c+k0@il2ko0|,2h+qo@69g1s0|1y+qo|0t+qo|1v+ic|44+u0,44+qo@b33z80,44+u0@bi8i00,44+xc@liock0,@ne0cg0,22+qo@ne0f80,44+u0@o4nrc0|1v+ic,4b+lo@drxa20,4b+k0@dzufc0,1v+ic@ixq620|1a+6o,@rkfjo0|,0p+k0@n30o0|16+u0,1y+qo@3b0ho0,16+u0@g0zls0|1q+dc|3h+dc|3c+dc|3p+go|1a+6o,@od5jo0,1a+6o@oyk840|23+6o,1a+6o@dkh140|1t+qo|2f+qo,0y+qo@fmzwg0|1u+k0,1u+nc@46anc0|22+qo,22+nc@b34200,22+qo@bi8ks0,22+u0@liofc0,22+qo@ne0f80|24+u0|1z+u0|32+u0|23+6o|01+f0|25+140,25+10o@b33qw0,25+140@bi89o0,25+10o@kzy1k0,25+140@lio700|,31+go@n33g0|2s+ic,2s+j6@8clsq0|44+u0,44+qo@b33z80,44+u0@bi8i00,3z+xc@hqrlo0,3z+10o@lio9s0,44+xc@lreus0,44+u0@ne0cg0|2b+nc,2b+k0@b344s0,2b+nc@bi8nk0,2b+qo@lioi40,2b+nc@ne0i00|3i+p0,3i+qo@69g1s0|2h+qo|2g+10o,2g+xc@b33to0,2g+10o@bi8cg0,2g+140@lio700,2g+xc@ne06w0,2g+10o@o63gg0|36+qo|2b+nc,2b+k0@b344s0,2b+nc@bi8nk0,2z+k0@kzyfg0,2z+nc@liokw0,2b+nc@ne0ks0|2z+nc,2z+k0@b344s0,2z+nc@bi8nk0,2z+k0@c7fr40,2z+nc@liokw0,2z+k0@ne0ks0,2b+nc@oasa80|30+k0,30+go@b347k0,30+k0@bi8qc0,30+nc@liokw0,30+k0@ne0ks0|1y+qo,20+nc@9e5gg0|29+u0,3b+sc@nt2v00,29+u0@p87lo0|--+go,27+k0@i6f100,26+go@s9mq00|--+k0,4d+go@bghm40,4d+k0@bi8qc0,4d+go@blua80,27+k0@i6f100,28+go@pk1rc0,26+go@s9mq00|2q+lo|10+lo|3e+10o,3e+xc@b33to0,3e+10o@bi8cg0,3e+xc@e7t8c0,3e+10o@lio9s0,2g+xc@ne09o0,@o4nls0,2g+10o@o690g0|,3w+go@bb18o0|29+u0|0y+qo|2g+10o,2g+xc@b33to0,2g+10o@bi8cg0,2g+140@lio700,@ne06w0,2g+10o@o63gg0|3o+qo|,3w+go@b347k0|,1j+a0@b34d40,1j+dc@cwngw0,1j+a0@hzxjg0,1j+dc@idzek0|21+bo,21+dc@42lfi0,21+bo@4mga80|1v+ic,0q+k0@99fa20|,2b+nc@o7wkw0|2o+nc,2o+qo@46akk0|44+u0,2g+10o@5vapo0,2g+xc@b33to0,2g+10o@bi8cg0,2g+140@lio700,3z+10o@lres00,3z+xc@ne09o0|3z+xc,3z+u0@b33wg0,3z+xc@bi8f80,3z+10o@lio9s0,3z+xc@ne09o0|44+u0,44+qo@b33z80,44+u0@bi8i00,44+xc@liock0,44+u0@ne0cg0|,45+go@bi8t40,45+k0@liono0,45+go@ne0nk0|,0h+a0@b34d40,0h+dc@dfdrw0|0o-3c,1c+0@bzwaw0,0o-3c@c8qqs0|1c+0|0u-6o,0u-3c@32t740|3k-6o|1d-dc,1d-a0@6yf4g0,1d-dc@872gc0,1d-a0@l89fc0|0j+vo|0k+t6|0l+xc,2e+z0@5tp880|0m+qo|,2p+a0@blufs0,2p+dc@liot80,2p+a0@ne0t40,3f+dc@o4o580|2p+a0,1a+6o@am73s0|1l+0,4f+0@15kg80|1l+0,4g+0@15kg80|1a+6o,4e+a0@4fid00,1a+6o@7qp980,4e+a0@od3p00|2p+a0,1a+6o@a1bbw0,1b+a0@liow00,1a+6o@ne0vw0|2p+a0,1a+6o@ap2t40|,2p+a0@ne0t40|19+3c,1c+0@3ijk00,19+3c@bv7mo0,1c+0@dp3xg0|2p+a0,1a+6o@b34fw0,1b+a0@liow00,2p+a0@ne2ns0|2p+a0,1a+6o@b34fw0,2p+a0@bi8yo0,2p+dc@liot80,2p+a0@ne0t40|2p+a0,1a+6o@a1bbw0|,2p+a0@a1b940,1a+dc@b34fw0,3f+a0@bchbo0,3f+dc@bdkg00,3f+a0@kzyns0,3f+dc@liot80|,2p+a0@blufs0,2p+dc@liot80,2p+a0@ne0t40,3f+dc@ohmt80|2p+a0,1a+6o@ap2vw0,2p+a0@cp3bo0,1a+6o@e7txc0,2p+dc@n382o0,2p+a0@ne0t40|,2p+a0@bi8yo0,2p+dc@liot80,2p+a0@ne0t40,3f+dc@o4o580|2p+a0,1a+6o@a1bbw0,19+3c@eqk2s0,1a+6o@fkfxc0|40+dc,40+a0@9il6g0,40+dc@b34fw0,40+a0@blufs0,40+dc@liot80,40+a0@ne0t40,40+dc@pha580,2p+a0@qlyvs0|1w+go,1w+k0@dkgss0|2i+go|1g+go|2l+dc|0d-10o,0d+17c@m1r5k0|0w+16i|17-nc,17-k0@6d68c0|3x+10o|37-140,37-10o@535io0,37+17c@d1o980|3q-10o,3q+17c@lx0jw0|1e+140|1k+140|3u+140|2k+140|42+140|43+140|18-go,1h-k0@8cmlw0|1i-u0|3j+10o|3a+10o|1p+xc,0v+xc@g5z2w0|4c+xc,0v+xc@g5z2w0|4h-xc|3n-xc|2d-zk,2d-xc@535eyo,2d+1ao@d1o6g0|2a+140,2a+10o@f4tw00|2j-vo|2w-10o|,3g-10o@77ss00|2r+12c,2r+140@4r4dm0|2x+12c,2x+10o@nvney0|2t+10o|39-sc,39-qo@es2cy0|12-z0,12-xc@4sal20|3r+17c';
  const TZ_ZONE_ERAS = '0100010101010101010101010102030002020202020204030302020205060006060606060606060601070800080808080808080707090a0b0c0d0e0f060g0h0i0j0k0l0m0n000n0n0n0n0n0n0n0n0n0n0n0n0n0n0n0n0n0n0n0o0p0q0r0s0t0u0v0w0o0x0n0o0y0z101112130z1415161700180018180y00191a0r0y1b1100110z1c1d1e110018111f0y1g0o0n1h1i1j0y1k1l0n1m1n1o1p1q1r181s1t181u1v181w1x1y1z20001p0o210z0n0y1n222324250y260n0y27181818000l28292a2b2c2d180z2e2f0o0y002g1f2h2i2j0o2k2l2m2n0y0n20202o0y0l2p2q2r002s2t2u2v2w2w2x2y012z0i0i0i0i0i3031323334353603373738393a0039393b3c093d3e3f003g3h3i3j3k3l3m3n003n3o3p3q3r3s3s3t3u3v3w3x3y3z00404142434445464748494a094b4c4d364e4f4g4h4i4j394k4l4m4n4o4p4q4r4s4t4u4v00004w4x4y4z50510n52535252540055562u5656572u2u582u592u0i0i0i0i5a090i0i0i0i0i0i0i0i090i0i0i0i5b5c5d0101010i09095e5f5g5h5i0i0i5j0i0i5k5l0i0i0i5m5n5o095l0i5p0i5q5r0i5s5t005u5v5w2r5x5y5z6061626300646566676869006a006b6c6d6e6f006g6h656i6j6k006k6l6m6n6o6p6q0000000000';
  const REGIONS = words('America Asia Europe Africa Pacific Australia Antarctica Atlantic Indian Etc');
  const expand = (name) => name < 'A' ? REGIONS[name[0]] + '/' + name.slice(1) : name;
  const zoneCache = new Map();
  let zoneList;

  // Every zone name in TZ_ZONES, in order.
  function zoneNames() {
    return zoneList ??= TZ_ZONES.split('|').flatMap((entry) => entry.slice(0, entry.indexOf(':')).split(',').map(expand));
  }

  // The zone called `name`, in any case, or undefined: its canonical name
  // and its packed rules, which are unpacked when the zone is used. The
  // name is looked for among the names of each entry of TZ_ZONES, then
  // among the links.
  let zonesLower, linksLower;
  function findZone(name) {
    const key = name.toLowerCase();
    if (zoneCache.has(key)) return zoneCache.get(key);
    let z;
    if (/^[a-z][\w+-]*(\/[\w+-]+)*$/.test(key)) {
      const slash = key.indexOf('/');
      const region = slash < 0 ? -1 : REGIONS.findIndex((r) => r.toLowerCase() === key.slice(0, slash));
      const packed = region < 0 ? key : region + key.slice(slash + 1);
      // Both tables get a separator in front, so every name follows one.
      zonesLower ??= '|' + TZ_ZONES.toLowerCase();
      for (let i = zonesLower.indexOf(packed); i > 0 && !z; i = zonesLower.indexOf(packed, i + 1)) {
        const entry = zonesLower.lastIndexOf('|', i), after = zonesLower[i + packed.length];
        if (!'|,'.includes(zonesLower[i - 1]) || !',:'.includes(after) || zonesLower.indexOf(':', entry) < i) continue;
        const end = TZ_ZONES.indexOf('|', i);
        const [, start, eras] = TZ_ZONES.slice(entry, end < 0 ? undefined : end).split(':');
        z = { name: expand(TZ_ZONES.substr(i - 1, packed.length)), start, eras };
      }
      if (!z) {
        linksLower ??= ' ' + TZ_LINKS.toLowerCase();
        const i = linksLower.indexOf(' ' + packed + '=');
        if (i >= 0) z = findZone(zoneNames()[parseInt(linksLower.slice(i + packed.length + 2), 36)]);
      }
    }
    zoneCache.set(key, z);
    return z;
  }

  // Readies a zone: z.rules are its eras, still packed, z.slots those of
  // the era reached, and z.at/z.off the transitions computed so far
  // (seconds since the epoch, and the offset in seconds from then on).
  function openZone(z) {
    if (z.at) return;
    tzSlots ??= TZ_SLOTS.split(' ');
    z.rules = z.eras ? z.eras.split(',') : [];
    z.slots = [];
    z.at = [-Infinity];
    z.off = [parseInt(z.start, 36) * 30];
    z.year = 1970;
    z.era = -1;
    z.next = z.rules.length ? 1970 + parseInt(z.rules[0].slice(0, 2), 36) : Infinity;
  }
  let tzSlots;

  // Slot number `ref` as [month, day code, seconds into the day, change
  // in offset in seconds], unpacked when first used.
  function slot(ref) {
    const i = parseInt(ref, 36);
    let s = tzSlots[i];
    if (typeof s === 'string') {
      const sign = s.search(/[+-]/);
      s = tzSlots[i] = [parseInt(s[0], 36), parseInt(s.slice(1, 3), 36), parseInt(s.slice(3, sign), 36) * 30,
        (s[sign] === '-' ? -30 : 30) * parseInt(s.slice(sign + 1), 36)];
    }
    return s;
  }

  // Midnight UTC, in days since the epoch, of the day a slot's day code
  // names in a month: a fixed day, the last given weekday, or the first
  // given weekday on or after a day. 1 January 1970 was a Thursday.
  function slotDay(code, y, m) {
    if (code < 40) return Date.UTC(y, m, code) / 864e5;
    if (code < 50) {
      const last = Date.UTC(y, m + 1, 0) / 864e5;
      return last - ((last + 4) % 7 - (code - 40) + 7) % 7;
    }
    const from = Date.UTC(y, m, (code - 50) % 31 + 1) / 864e5;
    return from + (floor((code - 50) / 31) - (from + 4) % 7 + 7) % 7;
  }

  // The Gregorian calendar repeats every 400 years, and so do the rules
  // once the last of them has begun: the seconds to take off a time `s`
  // in a far year to bring it into the table.
  function zoneShift(s) {
    const y = new Date(s * 1000).getUTCFullYear();
    return y < 2500 ? 0 : floor((y - 2100) / 400) * 12622780800;
  }

  // The number of the transition of zone `z` in force at `s` seconds (less
  // its zoneShift), which is a time on the zone's own clock when `local`:
  // z.at[i] is when it happens and z.off[i] the offset from then on. A
  // clock time that happens twice is the later one, as in ICU.
  function zoneIndex(z, s, local) {
    openZone(z);
    s -= zoneShift(s);
    const y = new Date(s * 1000).getUTCFullYear();
    // A transition belongs to the year on the wall clock before it, which
    // is within a day of the UTC year.
    while (z.year <= y + 1) {
      while (z.year >= z.next) {
        const rule = z.rules[++z.era], after = z.rules[z.era + 1];
        z.slots = (rule.slice(2).match(/../g) || []).map(slot);
        z.next = after ? 1970 + parseInt(after.slice(0, 2), 36) : Infinity;
      }
      for (const [m, day, wall, delta] of z.slots) {
        const off = z.off[z.off.length - 1];
        z.at.push(slotDay(day, z.year, m) * 86400 + wall - off);
        z.off.push(off + delta);
      }
      z.year++;
    }
    let lo = 0, hi = z.at.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (z.at[mid] + (local ? z.off[mid] : 0) <= s) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  // The offset of zone `z` at `t` (ms), in seconds. Before 1970 it is the
  // zone's offset at the start of 1970.
  function zoneOffset(z, t) {
    const i = zoneIndex(z, floor(t / 1000));
    return z.off[i];
  }

  // A time zone option as { name } with `host` for the host's own zone,
  // `off` for a fixed offset in seconds, or `rec` for a zone with rules.
  // As in V8, names match in any case and report their canonical form,
  // and a name that is no zone is a RangeError.
  function timeZone(tz) {
    if (tz === undefined) return { host: true };
    tz = String(tz);
    if (/^(etc\/)?(utc|uct|gmt([+-]?0)?|zulu|universal|greenwich)$/i.test(tz)) return { name: 'UTC', off: 0 };
    const m = /^([+-])(\d\d)(?::?(\d\d))?$/.exec(tz);
    if (m && m[2] < 24 && (m[3] || 0) < 60) {
      const off = (m[1] === '-' ? -1 : 1) * (m[2] * 60 + +(m[3] || 0));
      return { name: offsetName(off), off: off * 60 };
    }
    const rec = findZone(tz);
    if (!rec) throw new RangeError('Invalid time zone specified: ' + tz);
    return { name: rec.name, rec };
  }

  // The host's zone, as { name, rec, names }: UTC, or the first zone in
  // the table whose offsets are the host's, twice a year since 1970 and
  // around each of its own transitions. Years are tried oldest first, so
  // a zone that is not the host's is dropped before its later years are
  // worked out. Zones that have kept the same time since 1970 cannot be
  // told apart, so a host in Oslo reports Europe/Berlin: `names` are all
  // the zones it could be. A host that matches no zone reports the
  // Etc/GMT zone (or offset) of its winter offset, without rules.
  let host;
  function hostZone() {
    if (host) return host;
    const at = (s) => -new Date(s * 1000).getTimezoneOffset() * 60;
    const now = Date.now() / 1000;
    const probes = [];
    for (let y = 1970, last = new Date().getUTCFullYear(); y <= last; y++) probes.push(Date.UTC(y, 0, 15) / 1000, Date.UTC(y, 6, 15) / 1000);
    const offsets = probes.map(at);
    const start = at(0);
    if (!start && offsets.every((o) => !o)) return host = { name: 'UTC' };
    const key = (start < 0 ? '-' : '') + (abs(start) / 30).toString(36);
    for (const entry of TZ_ZONES.split('|')) {
      const fields = entry.split(':');
      if (fields[1] !== key) continue;
      const z = findZone(expand(fields[0].split(',')[0]));
      if (!probes.every((p, i) => zoneOffset(z, p * 1000) === offsets[i])) continue;
      if (z.at.every((s, i) => !i || s > now || (at(s) === z.off[i] && at(s - 1) === z.off[i - 1]))) return host = { name: z.name, rec: z, names: fields[0].split(',').map(expand) };
    }
    const winter = at(946684800) / 60;
    return host = { name: !winter ? 'UTC' : winter % 60 ? offsetName(winter) : 'Etc/GMT' + (winter > 0 ? '-' : '+') + abs(winter / 60) };
  }

  // 'GMT-5' or, long, 'GMT-05:00', for an offset in seconds.
  function offsetText(off, long) {
    const hh = floor(abs(off) / 3600), mm = floor(abs(off) / 60) % 60, ss = abs(off) % 60;
    return 'GMT' + (off < 0 ? '-' : '+') + (long ? pad(hh, 2) + ':' + pad(mm, 2) : hh + (mm || ss ? ':' + pad(mm, 2) : '')) + (ss ? ':' + pad(ss, 2) : '');
  }

  // Zone names.

  // Metazone number `i` of TZ_METAZONES, unpacked when first used:
  // names[language] is its long [generic, standard, daylight] names.
  const ZONE_WORDS = [[' Time', ' Standard Time', ' Daylight Time', ' Summer Time'], ['Zeit', 'Normalzeit', 'Sommerzeit', 'Sommerzeit']];
  let metazones, eraLists;
  function metazone(i) {
    metazones ??= TZ_METAZONES.split('|');
    let m = metazones[i];
    if (typeof m === 'string') {
      const [en, de, golden, shorts, base] = m.split('~');
      const names = [en, de].map((text, language) => {
        if (text[0] === '=') return ['', text.slice(1), ''];
        const star = text.indexOf('*'), stem = text.slice(0, star), words = ZONE_WORDS[language];
        return star < 0 ? text.split(';') : [stem + words[0], stem + words[1], stem + words[text[star + 1] === 'S' ? 3 : 2]];
      });
      m = metazones[i] = { names, golden, shorts, base: base ? parseInt(base, 36) : i };
    }
    return m;
  }

  // Where zone record `z` stands at `t`, as [from, metazone number,
  // standard offset in seconds]: -1 where it is in no metazone, and NaN
  // where its standard offset is not known. The standard offsets come
  // from the names Chrome gives: the offset it calls standard time, and
  // for a time in no metazone one that makes the generic names after it
  // come out as Chrome's.
  const NO_META = [0, -1, NaN];
  function zoneMeta(z, t) {
    if (!z.metas) {
      eraLists ??= TZ_ERAS.split('|');
      const list = eraLists[parseInt(TZ_ZONE_ERAS.substr(zoneNames().indexOf(z.name) * 2, 2), 36)];
      z.metas = !list ? [] : list.split(',').map((era) => {
        const at = era.indexOf('@'), body = at < 0 ? era : era.slice(0, at);
        return [at < 0 ? -Infinity : parseInt(era.slice(at + 1), 36) * 1000, body && body[0] !== '-' ? parseInt(body.slice(0, 2), 36) : -1,
          body && body[2] !== '!' ? (body[2] === '-' ? -30 : 30) * parseInt(body.slice(3), 36) : NaN];
      });
    }
    for (let i = z.metas.length; i--;) if (z.metas[i][0] <= t) return z.metas[i];
    return NO_META;
  }

  // The host's zone is known only by its offsets: the other zones it
  // could be that have had names of their own, as Dublin has beside
  // London. Only zones named for a place count.
  function hostRivals() {
    const zone = hostZone();
    const eras = (name) => TZ_ZONE_ERAS.substr(zoneNames().indexOf(name) * 2, 2);
    return zone.rivals ??= zone.names.filter((name) => /^(?!Etc)\w+\//.test(name) && eras(name) !== eras(zone.name)).map(findZone);
  }

  // The short [standard, daylight, generic] names metazone `m` has in the
  // locale, if any.
  function shortNames(data, m) {
    const letters = data.zones.shorts;
    m.short ??= {};
    if (!(letters in m.short)) {
      const part = (m.shorts ? m.shorts.split(';') : []).find((part) => letters.includes(part[0]));
      m.short[letters] = part && part.slice(2).split(',');
    }
    return m.short[letters];
  }

  // The long name of metazone `m` numbered `which` in its names, spelt
  // the locale's way.
  function longName(data, m, which) {
    let name = m.names[data.zones.language][which];
    for (const [from, to] of data.zones.spelling || []) name = name.replace(from, to);
    return name;
  }

  // The standard or daylight name of metazone number `i`, or ''.
  function specificName(data, i, daylight, long) {
    const m = metazone(i);
    if (long) return longName(data, m, 1 + daylight);
    return (shortNames(data, m) || shortNames(data, metazone(m.base)) || [])[+daylight] || '';
  }

  // Whether zone record `z` is on daylight time within 184 days of `t`,
  // going by the change before it and the one after, as ICU does. A
  // change is one of offset or of standard offset: a zone can move to
  // another metazone's standard time from its own daylight time and keep
  // its clocks where they are.
  const HALF_YEAR = 184 * 86400;
  function savesNear(z, t) {
    const s = floor(t / 1000), shift = zoneShift(s), i = zoneIndex(z, s), now = s - shift;
    // Where the standard offset is not known, the zone counts as on it.
    const saves = (off, at) => {
      const std = zoneMeta(z, (at + shift) * 1000)[2];
      return std === std && off !== std;
    };
    let before = i ? z.at[i] : -Infinity, after = i + 1 < z.at.length ? z.at[i + 1] : Infinity;
    let was = z.off[i - 1], will = z.off[i + 1];
    z.metas.forEach(([from, , std], k) => {
      const last = k && z.metas[k - 1][2];
      from = from / 1000 - shift;
      if (!k || std !== std || last !== last || std === last) return;
      if (from <= now && from > before) [before, was] = [from, z.off[i]];
      if (from > now && from < after) [after, will] = [from, z.off[i]];
    });
    return (now - before < HALF_YEAR && saves(was, before - 1)) || (after - now < HALF_YEAR && saves(will, after));
  }

  // The generic name of zone record `z`, which is in metazone number `i`
  // with standard offset `std`, as ICU chooses it: the standard name where
  // the zone keeps one time all year, else the metazone's generic name if
  // the zone keeps the time of the metazone's own zone. The names ICU
  // makes from a country or city are not here, and are ''.
  function genericName(data, z, i, std, t, off, long) {
    const m = metazone(i), base = metazone(m.base);
    const generic = long ? longName(data, base, 0) : (shortNames(data, m) || shortNames(data, base) || [])[2] || '';
    if (off === std && !savesNear(z, t)) {
      const name = specificName(data, i, false, long);
      if (name && name !== generic) return name;
    }
    if (!generic || !base.golden) return '';
    const zones = base.golden.split(',');
    const golden = expand((zones.find((name) => name.startsWith(data.zones.region)) || '  ' + zones[0]).slice(2));
    if (golden !== z.name) {
      const g = findZone(golden), local = floor(t / 1000) + off;
      const j = zoneIndex(g, local, true), other = g.off[j];
      if (other !== off || !Object.is(zoneMeta(g, (local - other) * 1000)[2], std)) return '';
    }
    return generic;
  }

  // The zone's name for pattern letter `c` of width `n`: an offset for
  // O, a standard or daylight name for z and a generic one for v, or the
  // offset where the locale has no such name.
  function zoneName(data, zone, c, n, t, off) {
    const long = n > 3;
    if (c === 'O') return offsetText(off, long);
    if (zone.name === 'UTC') return c !== 'z' ? offsetText(off, long) : long ? data.utc : 'UTC';
    let name = '';
    if (zone.rec) {
      const named = (rec) => {
        const [, i, std] = zoneMeta(rec, t);
        return i < 0 ? '' : c === 'z' ? specificName(data, i, off !== std, long) : genericName(data, rec, i, std, t, off, long);
      };
      name = named(zone.rec);
      // The host has a name where every zone it could be has that name.
      if (name && zone.host && hostRivals().some((rival) => named(rival) !== name)) name = '';
    } else if (!off && !zone.host && c === 'z' && !data.zones.language) {
      // An offset of zero given as an offset.
      name = long ? 'Greenwich Mean Time' : 'GMT';
    }
    return name || offsetText(off, long);
  }

  function dateState(locales, o) {
    o = options(o);
    const [locale, data] = dateLocale(locales);
    const r = { locale, calendar: 'gregory', numberingSystem: 'latn' };
    const zone = timeZone(o.timeZone);
    // The host's zone is named when it is asked for.
    r.timeZone = zone.name;
    let hc = str(o, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    if (o.hour12 !== undefined) hc = o.hour12 ? 'h12' : 'h23';
    hc ||= data.hour === 'h' ? 'h12' : 'h23';
    const s = {};
    let any = false;
    for (const [k, values] of FIELDS) {
      const v = values ? str(o, k, values) : num(o, k, 1, 3);
      if (v === undefined) continue;
      s[k] = v;
      if (k !== 'era' && k !== 'timeZoneName') any = true;
    }
    const ds = str(o, 'dateStyle', STYLES), ts = str(o, 'timeStyle', STYLES);
    let pattern;
    if (ds || ts) {
      const set = Object.keys(s)[0];
      if (set) throw new TypeError("Can't set option " + set + ' when ' + (ds ? 'dateStyle' : 'timeStyle') + ' is used');
      const date = data.date[STYLES.indexOf(ds)], time = data.time[STYLES.indexOf(ts)];
      pattern = !time ? date : !date ? time : data.glue[STYLES.indexOf(ds)].replace('{0}', time).replace('{1}', date);
      // A style with another hour cycle is found again by its skeleton.
      if (time && HOUR_CHARS[hc] !== data.hour) {
        pattern = bestPattern(data, tokens(pattern).map((t) => typeof t === 'string' ? '' : (FIELD[t[0]] === 11 ? HOUR_CHARS[hc] : t[0]).repeat(t[1])).join(''));
      }
    } else {
      if (!any) s.year = s.month = s.day = 'numeric';
      pattern = bestPattern(data, skeletonOf(s, hc));
    }
    // The pattern's tokens, its hours in the hour cycle asked for.
    const lists = data.lists ??= new Map();
    let list = lists.get(pattern + hc);
    if (!list) {
      list = tokens(pattern);
      for (const t of list) {
        if (typeof t === 'string' || FIELD[t[0]] !== 11) continue;
        t[0] = HOUR_CHARS[hc];
        list.hour = true;
      }
      lists.set(pattern + hc, list);
    }
    if (list.hour) {
      r.hourCycle = hc;
      r.hour12 = hc === 'h11' || hc === 'h12';
    }
    if (ds) r.dateStyle = ds;
    if (ts) r.timeStyle = ts;
    if (!ds && !ts) for (const [k] of FIELDS) if (s[k] !== undefined) r[k] = s[k];
    return { r, data, zone, list };
  }

  function toTime(x) {
    const t = x === undefined ? Date.now() : Number(x);
    if (!isFinite(t) || abs(t) > 8.64e15) throw new RangeError('Invalid time value');
    return Math.trunc(t);
  }

  function dateParts({ data, zone, list }, x) {
    const t = toTime(x);
    if (zone.host && list.some((token) => FIELD[token[0]] === 15)) zone = { ...hostZone(), host: true };
    const off = zone.host ? -new Date(t).getTimezoneOffset() * 60 : zone.rec ? zoneOffset(zone.rec, t) : zone.off;
    const date = new Date(t + off * 1000);
    const y = date.getUTCFullYear(), mo = date.getUTCMonth(), h = date.getUTCHours();
    const out = [];
    for (const token of list) {
      if (typeof token === 'string') {
        const last = out[out.length - 1];
        if (last && last.type === 'literal') last.value += token;
        else out.push({ type: 'literal', value: token });
        continue;
      }
      const [c, n] = token;
      const width = n > 4 ? 2 : n === 4 ? 0 : 1;
      let value;
      switch (c) {
        case 'G': value = data.eras[width][+(y > 0)]; break;
        case 'y': value = pad((y > 0 ? y : 1 - y) % (n === 2 ? 100 : Infinity), n); break;
        case 'M': case 'L': value = n < 3 ? pad(mo + 1, n) : ((c === 'L' && data.monthsAlone) || data.months)[width][mo]; break;
        case 'd': value = pad(date.getUTCDate(), n); break;
        case 'E': case 'c': value = ((c === 'c' && data.daysAlone) || data.days)[width][date.getUTCDay()]; break;
        case 'a': value = data.ampm[+(h >= 12)]; break;
        case 'B': {
          const shows = (letter) => list.some((other) => other[0] === letter);
          const noon = h === 12 && data.noon && !(shows('m') && (date.getUTCMinutes() || (shows('s') && date.getUTCSeconds())));
          value = noon ? data.noon : data.periods.findLast((p) => p[0] <= h)[1];
          if (typeof value !== 'string') value = value[width];
          break;
        }
        case 'h': value = pad(h % 12 || 12, n); break;
        case 'K': value = pad(h % 12, n); break;
        case 'H': value = pad(h, n); break;
        case 'k': value = pad(h || 24, n); break;
        case 'm': value = pad(date.getUTCMinutes(), n); break;
        case 's': value = pad(date.getUTCSeconds(), n); break;
        case 'S': value = pad(date.getUTCMilliseconds(), 3).slice(0, n); break;
        default: value = zoneName(data, zone, c, n, t, off);
      }
      out.push({ type: PART[c], value });
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
      const r = { ...this.#state.r };
      r.timeZone ??= hostZone().name;
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

  const SUPPORTED = {
    calendar: () => ['gregory'],
    collation: () => [],
    currency: () => Object.keys(CURRENCIES).sort(),
    numberingSystem: () => ['latn'],
    timeZone: () => zoneNames().filter((name) => /^(?!Etc)\w+\//.test(name)).sort(),
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
