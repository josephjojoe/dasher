//! The small Intl implementation and the built-in toLocale* methods it
//! backs: en-US throughout, and en-CA, en-GB and de-DE for dates.

use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn page() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

/// Evaluates each expression and compares it with the expected string.
fn check(cases: &[(&str, &str)]) {
    let js = page();
    for (source, expected) in cases {
        let out = eval(&js, &format!("String({source})"));
        // No expected string contains a quote or backslash.
        assert_eq!(out, format!("\"{expected}\""), "{source}");
    }
}

#[test]
fn namespace_shape() {
    let js = page();
    let out = eval(
        &js,
        r#"({
          tag: Object.prototype.toString.call(Intl),
          enumerable: Object.keys(globalThis).includes('Intl'),
          members: Object.getOwnPropertyNames(Intl).sort(),
          callable: [Intl.NumberFormat() instanceof Intl.NumberFormat,
                     Intl.DateTimeFormat() instanceof Intl.DateTimeFormat,
                     Intl.Collator() instanceof Intl.Collator],
          name: Intl.NumberFormat.name,
          ctor: new Intl.NumberFormat().constructor === Intl.NumberFormat,
          protoTag: String(new Intl.PluralRules()),
          needsNew: (() => { try { Intl.PluralRules(); return false; } catch (e) { return e instanceof TypeError; } })(),
        })"#,
    );
    assert_eq!(
        out,
        r#"{"tag":"[object Intl]","enumerable":false,"members":["Collator","DateTimeFormat","ListFormat","Locale","NumberFormat","PluralRules","RelativeTimeFormat","getCanonicalLocales","supportedValuesOf"],"callable":[true,true,true],"name":"NumberFormat","ctor":true,"protoTag":"[object Intl.PluralRules]","needsNew":true}"#
    );
}

#[test]
fn locales() {
    let js = page();
    let out = eval(
        &js,
        r#"[
          Intl.getCanonicalLocales(['EN-us', 'zh-hant-tw', 'en-US', 'de-de-u-co-phonebk']),
          Intl.NumberFormat.supportedLocalesOf(['de-DE', 'en-GB', 'en']),
          new Intl.NumberFormat('de-DE').resolvedOptions().locale,
          new Intl.NumberFormat(['fr', 'en-GB-u-nu-latn']).resolvedOptions().locale,
          new Intl.DateTimeFormat().resolvedOptions().locale,
          (() => { try { Intl.getCanonicalLocales('en_US'); } catch (e) { return e.name; } })(),
          Intl.supportedValuesOf('calendar'),
          Intl.supportedValuesOf('currency').includes('EUR'),
          (() => { try { Intl.supportedValuesOf('nope'); } catch (e) { return e.name; } })(),
        ]"#,
    );
    assert_eq!(
        out,
        r#"[["en-US","zh-Hant-TW","de-DE-u-co-phonebk"],["en-GB","en"],"en-US","en-GB","en-US","RangeError",["gregory"],true,"RangeError"]"#
    );
}

#[test]
fn decimal_numbers() {
    check(&[
        (
            "new Intl.NumberFormat().format(1234567.891)",
            "1,234,567.891",
        ),
        ("new Intl.NumberFormat().format(1234.5678)", "1,234.568"),
        ("new Intl.NumberFormat().format(-0.5)", "-0.5"),
        ("new Intl.NumberFormat().format(-0)", "-0"),
        (
            "new Intl.NumberFormat().format(1e21)",
            "1,000,000,000,000,000,000,000",
        ),
        ("new Intl.NumberFormat().format(0.0000001)", "0"),
        ("new Intl.NumberFormat().format(NaN)", "NaN"),
        ("new Intl.NumberFormat().format(-Infinity)", "-∞"),
        (
            "new Intl.NumberFormat().format(12345678901234567890n)",
            "12,345,678,901,234,567,890",
        ),
        ("new Intl.NumberFormat().format('1234.5')", "1,234.5"),
        (
            "new Intl.NumberFormat('en', { maximumFractionDigits: 2 }).format(1.005)",
            "1.01",
        ),
        (
            "new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(2.5)",
            "3",
        ),
        (
            "new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(-2.5)",
            "-3",
        ),
        (
            "new Intl.NumberFormat('en', { minimumFractionDigits: 2 }).format(5)",
            "5.00",
        ),
        (
            "new Intl.NumberFormat('en', { minimumFractionDigits: 2 }).format(9.9999)",
            "10.00",
        ),
        (
            "new Intl.NumberFormat('en', { minimumIntegerDigits: 3 }).format(7)",
            "007",
        ),
        (
            "new Intl.NumberFormat('en', { maximumSignificantDigits: 3 }).format(123456)",
            "123,000",
        ),
        (
            "new Intl.NumberFormat('en', { maximumSignificantDigits: 2 }).format(0.012345)",
            "0.012",
        ),
        (
            "new Intl.NumberFormat('en', { minimumSignificantDigits: 3 }).format(0)",
            "0.00",
        ),
        (
            "new Intl.NumberFormat('en', { minimumSignificantDigits: 3 }).format(0.05)",
            "0.0500",
        ),
        (
            "new Intl.NumberFormat('en', { useGrouping: false }).format(1234567)",
            "1234567",
        ),
        (
            "new Intl.NumberFormat('en', { useGrouping: 'min2' }).format(1234)",
            "1234",
        ),
        (
            "new Intl.NumberFormat('en', { useGrouping: 'min2' }).format(12345)",
            "12,345",
        ),
        (
            "new Intl.NumberFormat('en', { signDisplay: 'always' }).format(5)",
            "+5",
        ),
        (
            "new Intl.NumberFormat('en', { signDisplay: 'exceptZero' }).format(0)",
            "0",
        ),
        (
            "new Intl.NumberFormat('en', { signDisplay: 'exceptZero' }).format(-1)",
            "-1",
        ),
        (
            "new Intl.NumberFormat('en', { signDisplay: 'never' }).format(-1)",
            "1",
        ),
        (
            "new Intl.NumberFormat('en', { signDisplay: 'negative' }).format(-0)",
            "0",
        ),
        (
            "new Intl.NumberFormat('en', { minimumFractionDigits: 2, trailingZeroDisplay: 'stripIfInteger' }).format(5)",
            "5",
        ),
        ("new Intl.NumberFormat().formatRange(3, 5)", "3 – 5"),
    ]);
}

#[test]
fn percent_currency_unit_and_notation() {
    check(&[
        (
            "new Intl.NumberFormat('en', { style: 'percent' }).format(0.256)",
            "26%",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'percent', maximumFractionDigits: 1 }).format(0.07)",
            "7%",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'percent' }).format(-12.3456)",
            "-1,235%",
        ),
        (
            "new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(1234.5)",
            "$1,234.50",
        ),
        (
            "new Intl.NumberFormat('de-DE', { style: 'currency', currency: 'eur' }).format(-3)",
            "-€3.00",
        ),
        (
            "new Intl.NumberFormat('ja', { style: 'currency', currency: 'JPY' }).format(1234.5)",
            "¥1,235",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'CAD' }).format(1)",
            "CA$1.00",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'CAD', currencyDisplay: 'narrowSymbol' }).format(1)",
            "$1.00",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'CHF' }).format(1)",
            "CHF\u{a0}1.00",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'EUR', currencyDisplay: 'code' }).format(1)",
            "EUR\u{a0}1.00",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'USD', currencyDisplay: 'name' }).format(2)",
            "2.00 US dollars",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'USD', currencySign: 'accounting' }).format(-5)",
            "($5.00)",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'currency', currency: 'USD', maximumFractionDigits: 0 }).format(9.99)",
            "$10",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'unit', unit: 'kilometer' }).format(12.5)",
            "12.5 km",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'unit', unit: 'kilometer-per-hour', unitDisplay: 'long' }).format(50)",
            "50 kilometers per hour",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'unit', unit: 'day', unitDisplay: 'long' }).format(1)",
            "1 day",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'unit', unit: 'megabyte', unitDisplay: 'narrow' }).format(3)",
            "3MB",
        ),
        (
            "new Intl.NumberFormat('en', { style: 'unit', unit: 'percent' }).format(40)",
            "40%",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact' }).format(1234)",
            "1.2K",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact' }).format(12345)",
            "12K",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact' }).format(999999)",
            "1M",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact' }).format(999)",
            "999",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact' }).format(1.234)",
            "1.2",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact', compactDisplay: 'long' }).format(2500000)",
            "2.5 million",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'compact', style: 'currency', currency: 'USD' }).format(1500)",
            "$1.5K",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'scientific' }).format(123456)",
            "1.235E5",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'scientific' }).format(0.00012)",
            "1.2E-4",
        ),
        (
            "new Intl.NumberFormat('en', { notation: 'engineering' }).format(123456)",
            "123.456E3",
        ),
    ]);
}

#[test]
fn number_parts_and_errors() {
    let js = page();
    let out = eval(
        &js,
        r#"[
          new Intl.NumberFormat('en', { style: 'currency', currency: 'USD' }).formatToParts(-1234.5),
          (() => { try { new Intl.NumberFormat('en', { style: 'currency' }); } catch (e) { return e.name; } })(),
          (() => { try { new Intl.NumberFormat('en', { style: 'bogus' }); } catch (e) { return e.name; } })(),
          (() => { try { new Intl.NumberFormat('en', { maximumFractionDigits: 101 }); } catch (e) { return e.name; } })(),
          (() => { try { new Intl.NumberFormat('en', { minimumFractionDigits: 3, maximumFractionDigits: 1 }); } catch (e) { return e.name; } })(),
          (() => { try { new Intl.NumberFormat('en', { style: 'unit', unit: 'parsec' }); } catch (e) { return e.name; } })(),
          new Intl.NumberFormat('en', { style: 'currency', currency: 'EUR' }).resolvedOptions(),
          typeof new Intl.NumberFormat().format,
          [1, 2].map(new Intl.NumberFormat('en', { minimumFractionDigits: 1 }).format),
        ]"#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[[{"type":"minusSign","value":"-"},{"type":"currency","value":"$"},{"type":"integer","value":"1"},"#,
            r#"{"type":"group","value":","},{"type":"integer","value":"234"},{"type":"decimal","value":"."},"#,
            r#"{"type":"fraction","value":"50"}],"TypeError","RangeError","RangeError","RangeError","RangeError","#,
            r#"{"locale":"en","numberingSystem":"latn","style":"currency","currency":"EUR","currencyDisplay":"symbol","#,
            r#""currencySign":"standard","minimumIntegerDigits":1,"minimumFractionDigits":2,"maximumFractionDigits":2,"#,
            r#""useGrouping":"auto","notation":"standard","signDisplay":"auto","trailingZeroDisplay":"auto","#,
            r#""roundingIncrement":1,"roundingMode":"halfExpand","roundingPriority":"auto"},"function",["1.0","2.0"]]"#
        )
    );
}

#[test]
fn number_to_locale_string() {
    check(&[
        ("(1234567.891).toLocaleString()", "1,234,567.891"),
        ("(1234.5).toLocaleString('de-DE')", "1,234.5"),
        ("(0.5).toLocaleString('en-US', { style: 'percent' })", "50%"),
        (
            "(19.9).toLocaleString(undefined, { style: 'currency', currency: 'USD' })",
            "$19.90",
        ),
        ("(123456789n).toLocaleString()", "123,456,789"),
        ("[1234, 5678.9].toLocaleString()", "1,234,5,678.9"),
    ]);
}

#[test]
fn plural_rules() {
    let js = page();
    let out = eval(
        &js,
        r#"{
          const card = new Intl.PluralRules('en-US');
          const ord = new Intl.PluralRules('en-US', { type: 'ordinal' });
          const fixed = new Intl.PluralRules('en-US', { minimumFractionDigits: 1 });
          ({
            cardinal: [0, 1, 2, 1.5, -1, '1'].map((n) => card.select(n)),
            ordinal: [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111].map((n) => ord.select(n)),
            fixed: fixed.select(1),
            options: ord.resolvedOptions(),
          })
        }"#,
    );
    assert_eq!(
        out,
        concat!(
            r#"{"cardinal":["other","one","other","other","one","one"],"#,
            r#""ordinal":["one","two","few","other","other","other","other","one","two","few","one","other"],"#,
            r#""fixed":"other","options":{"locale":"en-US","type":"ordinal","minimumIntegerDigits":1,"#,
            r#""minimumFractionDigits":0,"maximumFractionDigits":3,"pluralCategories":["few","one","other","two"],"#,
            r#""roundingIncrement":1,"roundingMode":"halfExpand","roundingPriority":"auto","trailingZeroDisplay":"auto"}}"#
        )
    );
}

// 2026-11-10T15:04:05.678Z, a Tuesday.
const T: &str = "1794323045678";

fn utc(options: &str) -> String {
    format!("new Intl.DateTimeFormat('en-US', {{ timeZone: 'UTC', {options} }}).format({T})")
}

#[test]
fn date_time_format() {
    let cases: Vec<(String, &str)> = vec![
        (utc(""), "11/10/2026"),
        (utc("dateStyle: 'full'"), "Tuesday, November 10, 2026"),
        (utc("dateStyle: 'long'"), "November 10, 2026"),
        (utc("dateStyle: 'medium'"), "Nov 10, 2026"),
        (utc("dateStyle: 'short'"), "11/10/26"),
        (utc("timeStyle: 'short'"), "3:04 PM"),
        (utc("timeStyle: 'medium'"), "3:04:05 PM"),
        (utc("timeStyle: 'long'"), "3:04:05 PM UTC"),
        (
            utc("timeStyle: 'full'"),
            "3:04:05 PM Coordinated Universal Time",
        ),
        (
            utc("dateStyle: 'medium', timeStyle: 'short'"),
            "Nov 10, 2026, 3:04 PM",
        ),
        (
            utc("dateStyle: 'long', timeStyle: 'short'"),
            "November 10, 2026 at 3:04 PM",
        ),
        (
            utc("year: 'numeric', month: 'long', day: 'numeric'"),
            "November 10, 2026",
        ),
        (utc("year: 'numeric', month: 'short'"), "Nov 2026"),
        (utc("month: 'long', day: 'numeric'"), "November 10"),
        (utc("month: 'long'"), "November"),
        (utc("weekday: 'long'"), "Tuesday"),
        (
            utc("weekday: 'short', year: 'numeric', month: 'short', day: 'numeric'"),
            "Tue, Nov 10, 2026",
        ),
        (
            utc("year: '2-digit', month: '2-digit', day: '2-digit'"),
            "11/10/26",
        ),
        (utc("month: 'numeric', day: 'numeric'"), "11/10"),
        (utc("hour: 'numeric', minute: '2-digit'"), "3:04 PM"),
        (
            utc("hour: '2-digit', minute: '2-digit', hour12: false"),
            "15:04",
        ),
        (utc("hour: 'numeric', hourCycle: 'h23'"), "15"),
        (utc("hour: 'numeric'"), "3 PM"),
        (
            utc("hour: 'numeric', minute: 'numeric', second: 'numeric', fractionalSecondDigits: 2"),
            "3:04:05.67 PM",
        ),
        (utc("timeZoneName: 'short'"), "11/10/2026, UTC"),
        (utc("era: 'short'"), "11/10/2026 AD"),
        (
            utc("hour: 'numeric', dayPeriod: 'long'"),
            "3 in the afternoon",
        ),
        (
            format!(
                "new Intl.DateTimeFormat('en-US', {{ timeZone: '+05:30', timeStyle: 'long' }}).format({T})"
            ),
            "8:34:05 PM GMT+5:30",
        ),
        (
            format!(
                "new Intl.DateTimeFormat('en-US', {{ timeZone: 'Etc/GMT+5', hour: 'numeric', timeZoneName: 'longOffset' }}).format({T})"
            ),
            "10 AM GMT-05:00",
        ),
        (
            format!("new Intl.DateTimeFormat('en', {{ timeZone: 'UTC' }}).format(new Date({T}))"),
            "11/10/2026",
        ),
        (
            format!(
                "new Intl.DateTimeFormat('en', {{ timeZone: 'UTC' }}).formatRange({T}, {T} + 864e5)"
            ),
            "11/10/2026 – 11/11/2026",
        ),
    ];
    let cases: Vec<(&str, &str)> = cases.iter().map(|(s, e)| (s.as_str(), *e)).collect();
    check(&cases);
}

#[test]
fn date_parts_options_and_errors() {
    let js = page();
    let out = eval(
        &js,
        &format!(
            r#"[
              new Intl.DateTimeFormat('en-US', {{ timeZone: 'UTC', hour: 'numeric', minute: '2-digit' }}).formatToParts({T}),
              new Intl.DateTimeFormat('en-US', {{ timeZone: 'utc', hour: 'numeric' }}).resolvedOptions(),
              new Intl.DateTimeFormat('en-US', {{ dateStyle: 'short', timeZone: 'America/New_York' }}).resolvedOptions(),
              (() => {{ try {{ new Intl.DateTimeFormat('en', {{ dateStyle: 'short', year: 'numeric' }}); }} catch (e) {{ return e.name; }} }})(),
              (() => {{ try {{ new Intl.DateTimeFormat('en', {{ timeZone: 'not a zone' }}); }} catch (e) {{ return e.name; }} }})(),
              (() => {{ try {{ new Intl.DateTimeFormat().format(NaN); }} catch (e) {{ return e.name; }} }})(),
              typeof Intl.DateTimeFormat().resolvedOptions().timeZone,
            ]"#
        ),
    );
    assert_eq!(
        out,
        concat!(
            r#"[[{"type":"hour","value":"3"},{"type":"literal","value":":"},{"type":"minute","value":"04"},"#,
            r#"{"type":"literal","value":" "},{"type":"dayPeriod","value":"PM"}],"#,
            r#"{"locale":"en-US","calendar":"gregory","numberingSystem":"latn","timeZone":"UTC","hourCycle":"h12","hour12":true,"hour":"numeric"},"#,
            r#"{"locale":"en-US","calendar":"gregory","numberingSystem":"latn","timeZone":"America/New_York","dateStyle":"short"},"#,
            r#""TypeError","RangeError","RangeError","string"]"#
        )
    );
}

/// Without a timeZone option, dates format in the host's zone, matching
/// Date's local getters whatever zone the tests run in.
#[test]
fn host_time_zone() {
    let js = page();
    let out = eval(
        &js,
        &format!(
            r#"{{
              const d = new Date({T});
              const pad = (n) => String(n).padStart(2, '0');
              const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
              const offset = d.getTimezoneOffset();
              [
                new Intl.DateTimeFormat('en-US', {{ hour: 'numeric', minute: '2-digit', hour12: false }}).format(d)
                  === pad(d.getHours()) + ':' + pad(d.getMinutes()),
                d.toLocaleDateString() === (d.getMonth() + 1) + '/' + d.getDate() + '/' + d.getFullYear(),
                // A name from the zone table, or an offset where none fits.
                zone === 'UTC' ? offset === 0 : /^([A-Z]\w+(\/[\w+-]+)+|[+-]\d\d:\d\d)$/.test(zone),
                // The zone it names keeps the host's time all year round. Its
                // name is left out: a host that could be in either of two
                // zones with different names gets an offset instead.
                [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].every((month) => {{
                  const t = Date.UTC(2026, month, 15, 12);
                  const o = {{ dateStyle: 'short', timeStyle: 'medium' }};
                  return new Intl.DateTimeFormat('en-US', o).format(t)
                    === new Intl.DateTimeFormat('en-US', {{ ...o, timeZone: zone }}).format(t);
                }}),
              ]
            }}"#
        ),
    );
    assert_eq!(out, "[true,true,true,true]");
}

#[test]
fn date_to_locale_strings() {
    let js = page();
    let out = eval(
        &js,
        &format!(
            r#"{{
              const d = new Date({T});
              [
                d.toLocaleString('en-US', {{ timeZone: 'UTC' }}),
                d.toLocaleDateString('en-US', {{ timeZone: 'UTC' }}),
                d.toLocaleTimeString('en-US', {{ timeZone: 'UTC' }}),
                d.toLocaleDateString(undefined, {{ timeZone: 'UTC', month: 'long', year: 'numeric' }}),
                d.toLocaleTimeString([], {{ timeZone: 'UTC', hour: '2-digit', minute: '2-digit' }}),
                d.toLocaleDateString('en', {{ timeZone: 'UTC', hour: 'numeric' }}),
                new Date(NaN).toLocaleString(),
                typeof d.toLocaleString(),
              ]
            }}"#
        ),
    );
    assert_eq!(
        out,
        concat!(
            r#"["11/10/2026, 3:04:05 PM","11/10/2026","3:04:05 PM","November 2026","03:04 PM","#,
            r#""11/10/2026, 3 PM","Invalid Date","string"]"#
        )
    );
}

// The date cases from here to `pattern_selection` have the strings Chrome
// 157.0.8103.51 gives for the same expressions on macOS. Node 22.20 (V8
// 12.4, ICU 77.1) agrees except where a comment says what it gives.
//
// T is as above; V is 2026-11-11T17:30:00Z, S 2026-07-04T18:30:15Z and
// M 2027-03-05T00:09:07.004Z.
const DATES: &str = "const T = 1794323045678, V = 1794418200000, S = 1783189815000, M = 1804205347004; \
    const fmt = (locale, options, t) => new Intl.DateTimeFormat(locale, options).format(t);";

/// As `check`, with the instants and `fmt` in scope.
fn check_dates(cases: &[(&str, &str)]) {
    let js = page();
    for (source, expected) in cases {
        let out = eval(&js, &format!("{{ {DATES} String({source}) }}"));
        assert_eq!(out, format!("\"{expected}\""), "{source}");
    }
}

/// en-CA writes numeric dates year first, with or without a time. The first
/// case is the one pages compare against their server-rendered markup.
#[test]
fn en_ca_dates() {
    check_dates(&[
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit' }, V)",
            "2026-11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York' }, V)",
            "2026-11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: 'numeric', day: 'numeric' }, M)",
            "2027-03-04",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: '2-digit', month: '2-digit', day: '2-digit' }, V)",
            "26-11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', month: '2-digit', day: '2-digit' }, V)",
            "11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', month: 'numeric', day: 'numeric' }, M)",
            "03-04",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit' }, V)",
            "2026-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', dateStyle: 'short' }, V)",
            "2026-11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', weekday: 'short', year: 'numeric', month: '2-digit', day: '2-digit' }, V)",
            "Wed, 2026-11-11",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' }, V)",
            "2026-11-11, 12:30:00 p.m.",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }, V)",
            "2026-11-11, 12:30:00",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit', hour: 'numeric', minute: '2-digit' }, M)",
            "2027-03-04, 7:09 p.m.",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }, M)",
            "2027-03-04, 19:09",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'short' }, V)",
            "2026-11-11, 12:30 p.m.",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'medium', hour12: false }, V)",
            "2026-11-11, 12:30:00",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', dateStyle: 'medium' }, V)",
            "Nov 11, 2026",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', dateStyle: 'long', timeStyle: 'short' }, V)",
            "November 11, 2026 at 12:30 p.m.",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', timeStyle: 'long' }, V)",
            "12:30:00 p.m. EST",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/New_York', month: 'short', day: 'numeric' }, V)",
            "Nov 11",
        ),
        (
            "new Date(V).toLocaleDateString('en-CA', { timeZone: 'America/New_York' })",
            "2026-11-11",
        ),
        (
            "new Date(V).toLocaleString('en-CA', { timeZone: 'America/New_York' })",
            "2026-11-11, 12:30:00 p.m.",
        ),
        (
            "new Date(V).toLocaleTimeString('en-CA', { timeZone: 'America/New_York' })",
            "12:30:00 p.m.",
        ),
        (
            "new Date(V).toLocaleDateString('en-CA', { timeZone: 'UTC' })",
            "2026-11-11",
        ),
        (
            "new Intl.DateTimeFormat('en-CA', { timeZone: 'America/New_York', year: 'numeric', month: '2-digit', day: '2-digit' }).formatToParts(V).map((p) => p.type + '=' + p.value)",
            "year=2026,literal=-,month=11,literal=-,day=11",
        ),
        (
            "new Intl.DateTimeFormat('en-CA').resolvedOptions().locale",
            "en-CA",
        ),
    ]);
}

/// Named zones: offsets on both sides of a change, half and quarter hours,
/// years past the end of the probed range, and how names are accepted.
#[test]
fn named_time_zones() {
    check_dates(&[
        (
            "fmt('en-US', { timeZone: 'America/Los_Angeles', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "Nov 10, 2026, 7:04 AM",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "Nov 10, 2026, 3:04 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', dateStyle: 'medium', timeStyle: 'short' }, S)",
            "Jul 4, 2026, 7:30 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Berlin', dateStyle: 'medium', timeStyle: 'short' }, S)",
            "Jul 4, 2026, 8:30 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Kolkata', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "Nov 10, 2026, 8:34 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Tokyo', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "Nov 11, 2026, 12:04 AM",
        ),
        (
            "fmt('en-US', { timeZone: 'Australia/Sydney', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "Nov 11, 2026, 2:04 AM",
        ),
        (
            "fmt('en-US', { timeZone: 'Australia/Sydney', dateStyle: 'medium', timeStyle: 'short' }, S)",
            "Jul 5, 2026, 4:30 AM",
        ),
        (
            "fmt('en-US', { timeZone: 'America/St_Johns', timeStyle: 'medium' }, S)",
            "4:00:15 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Kathmandu', timeStyle: 'medium' }, S)",
            "12:15:15 AM",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'long' }, 1772953199000)",
            "3/8/26, 1:59:59 AM EST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'long' }, 1772953200000)",
            "3/8/26, 3:00:00 AM EDT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'long' }, 489326400000)",
            "7/4/85, 8:00:00 AM EDT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'long' }, 3377937600000)",
            "1/15/77, 7:00:00 AM EST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', dateStyle: 'short', timeStyle: 'long' }, Date.UTC(6018, 6, 4, 12))",
            "7/4/18, 8:00:00 AM EDT",
        ),
        // British Columbia stays on daylight time from 2026 in Chrome's zone
        // data. Node 22 gives 1/15/27, 4:00:00 AM PST here.
        (
            "fmt('en-US', { timeZone: 'America/Vancouver', dateStyle: 'short', timeStyle: 'long' }, Date.UTC(2027, 0, 15, 12))",
            "1/15/27, 5:00:00 AM PDT",
        ),
        (
            "fmt('en-US', { timeZone: 'us/pacific', timeStyle: 'short' }, T)",
            "7:04 AM",
        ),
        (
            "fmt('en-US', { timeZone: '-08', timeStyle: 'long' }, T)",
            "7:04:05 AM GMT-8",
        ),
        (
            "fmt('en-US', { timeZone: '+0530', timeStyle: 'long' }, T)",
            "8:34:05 PM GMT+5:30",
        ),
        (
            "new Date(S).toLocaleString('en-US', { timeZone: 'America/Chicago' })",
            "7/4/2026, 1:30:15 PM",
        ),
        (
            "new Date(S).toLocaleTimeString('en-US', { timeZone: 'Europe/Paris', hour: '2-digit', minute: '2-digit', hour12: false })",
            "20:30",
        ),
        (
            "['us/pacific', 'AMERICA/new_york', 'Asia/Kolkata', 'Europe/Kyiv', 'GB', 'EST', 'Etc/GMT+5', 'etc/utc', 'GMT0', '-0800', '-00:00'].map((z) => new Intl.DateTimeFormat('en', { timeZone: z }).resolvedOptions().timeZone)",
            "America/Los_Angeles,America/New_York,Asia/Calcutta,Europe/Kiev,Europe/London,America/Panama,Etc/GMT+5,UTC,UTC,-08:00,+00:00",
        ),
        (
            "['America/Nowhere', 'Etc/Unknown', 'Etc/GMT+13', 'GMT+5', 'UTC+1', '+24:00', ' UTC', ''].map((z) => { try { new Intl.DateTimeFormat('en', { timeZone: z }); return 'ok'; } catch (e) { return e.name; } })",
            "RangeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError,RangeError",
        ),
        (
            "[Intl.supportedValuesOf('timeZone').length, Intl.supportedValuesOf('timeZone').slice(0, 2), Intl.supportedValuesOf('timeZone').includes('America/New_York'), Intl.supportedValuesOf('timeZone').includes('UTC')]",
            "418,Africa/Abidjan,Africa/Accra,true,false",
        ),
    ]);
}

/// Zone names in the styles pages use most. North American zones, Greenwich
/// Mean Time and UTC have short names in en-US; the rest have none there,
/// and Chrome writes their offset.
#[test]
fn time_zone_names() {
    check_dates(&[
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeStyle: 'long' }, T)",
            "10:04:05 AM EST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeStyle: 'long' }, S)",
            "2:30:15 PM EDT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeStyle: 'full' }, T)",
            "10:04:05 AM Eastern Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeStyle: 'full' }, S)",
            "2:30:15 PM Eastern Daylight Time",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeZoneName: 'short' }, T)",
            "11/10/2026, EST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeZoneName: 'shortOffset' }, T)",
            "11/10/2026, GMT-5",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeZoneName: 'longOffset' }, T)",
            "11/10/2026, GMT-05:00",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Chicago', timeStyle: 'long' }, S)",
            "1:30:15 PM CDT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Denver', timeStyle: 'long' }, T)",
            "8:04:05 AM MST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Phoenix', timeStyle: 'long' }, S)",
            "11:30:15 AM MST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Los_Angeles', timeStyle: 'full' }, S)",
            "11:30:15 AM Pacific Daylight Time",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Anchorage', timeStyle: 'long' }, T)",
            "6:04:05 AM AKST",
        ),
        (
            "fmt('en-US', { timeZone: 'Pacific/Honolulu', timeStyle: 'long' }, T)",
            "5:04:05 AM HST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Halifax', timeStyle: 'long' }, S)",
            "3:30:15 PM ADT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Toronto', timeStyle: 'long' }, T)",
            "10:04:05 AM EST",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', timeStyle: 'long' }, T)",
            "3:04:05 PM GMT",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', timeStyle: 'full' }, T)",
            "3:04:05 PM Greenwich Mean Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', timeStyle: 'long' }, S)",
            "7:30:15 PM GMT+1",
        ),
        // Node 22 gives 3:04:05 PM GMT here.
        (
            "fmt('en-US', { timeZone: 'Europe/Lisbon', timeStyle: 'long' }, T)",
            "3:04:05 PM GMT+0",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Berlin', timeStyle: 'long' }, T)",
            "4:04:05 PM GMT+1",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Kolkata', timeStyle: 'long' }, T)",
            "8:34:05 PM GMT+5:30",
        ),
        // Node 22 gives 11/10/2026, GMT here.
        (
            "fmt('en-US', { timeZone: 'UTC', timeZoneName: 'shortOffset' }, T)",
            "11/10/2026, GMT+0",
        ),
        // Node 22 gives 11/10/2026, GMT here.
        (
            "fmt('en-US', { timeZone: 'UTC', timeZoneName: 'longOffset' }, T)",
            "11/10/2026, GMT+00:00",
        ),
        (
            "fmt('en-US', { timeZone: '+00:00', timeStyle: 'full' }, T)",
            "3:04:05 PM Greenwich Mean Time",
        ),
        // Node 22 gives 2:30:15 p.m. Eastern Daylight Saving Time here.
        (
            "fmt('en-CA', { timeZone: 'America/New_York', timeStyle: 'full' }, S)",
            "2:30:15 p.m. Eastern Daylight Time",
        ),
        (
            "fmt('en-GB', { timeZone: 'America/New_York', timeStyle: 'long' }, T)",
            "10:04:05 GMT-5",
        ),
        (
            "fmt('en-GB', { timeZone: 'America/New_York', timeStyle: 'full' }, T)",
            "10:04:05 Eastern Standard Time",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/London', timeStyle: 'long' }, T)",
            "15:04:05 GMT",
        ),
        (
            "fmt('de-DE', { timeZone: 'America/New_York', timeStyle: 'long' }, T)",
            "10:04:05 GMT-5",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', timeStyle: 'full' }, T)",
            "15:04:05 Koordinierte Weltzeit",
        ),
    ]);
}

/// Long names, the short names en-CA, en-GB and German add, and generic
/// names. A zone moves between metazones over the years, and a generic name
/// is the standard one where the zone keeps one time all year. The generic
/// names Chrome makes from a country or city (Germany Time) are not
/// implemented and stay offsets, so none is here.
#[test]
fn metazone_names() {
    check_dates(&[
        (
            "fmt('en-US', { timeZone: 'Europe/Berlin', timeStyle: 'full' }, T)",
            "4:04:05 PM Central European Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Berlin', timeStyle: 'full' }, S)",
            "8:30:15 PM Central European Summer Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Kolkata', timeStyle: 'full' }, T)",
            "8:34:05 PM India Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Tokyo', timeZoneName: 'long' }, T)",
            "11/11/2026, Japan Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Australia/Sydney', timeZoneName: 'long' }, T)",
            "11/11/2026, Australian Eastern Daylight Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/London', timeZoneName: 'long' }, S)",
            "7/4/2026, British Summer Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Dublin', timeZoneName: 'long' }, S)",
            "7/4/2026, Irish Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Moscow', timeZoneName: 'long' }, T)",
            "11/10/2026, Moscow Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Moscow', timeZoneName: 'long' }, Date.UTC(2012, 0, 15))",
            "1/15/2012, Moscow Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Indiana/Knox', timeZoneName: 'long' }, Date.UTC(2000, 0, 15))",
            "1/14/2000, Eastern Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Almaty', timeZoneName: 'long' }, T)",
            "11/10/2026, Kazakhstan Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Africa/Casablanca', timeZoneName: 'long' }, S)",
            "7/4/2026, GMT+01:00",
        ),
        // Node 22 gives 11/10/2026, West Africa Standard Time here.
        (
            "fmt('en-US', { timeZone: 'Africa/Lagos', timeZoneName: 'long' }, T)",
            "11/10/2026, West Africa Time",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/St_Johns', timeStyle: 'long' }, T)",
            "11:34:05 a.m. NST",
        ),
        (
            "fmt('en-CA', { timeZone: 'America/Miquelon', timeZoneName: 'long' }, T)",
            "2026-11-10, Saint-Pierre-et-Miquelon Standard Time",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/London', timeStyle: 'long' }, S)",
            "19:30:15 BST",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/Paris', timeStyle: 'long' }, S)",
            "20:30:15 CEST",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/Athens', timeStyle: 'long' }, T)",
            "17:04:05 EET",
        ),
        (
            "fmt('en-GB', { timeZone: 'Asia/Dubai', timeStyle: 'long' }, T)",
            "19:04:05 GST",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/Paris', timeStyle: 'full' }, T)",
            "16:04:05 Central European Standard Time",
        ),
        (
            "fmt('en-GB', { timeZone: 'America/Miquelon', timeZoneName: 'long' }, T)",
            "10/11/2026, St Pierre & Miquelon Standard Time",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/Berlin', timeStyle: 'long' }, T)",
            "16:04:05 MEZ",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/Berlin', timeStyle: 'long' }, S)",
            "20:30:15 MESZ",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/Berlin', timeStyle: 'full' }, S)",
            "20:30:15 Mitteleuropäische Sommerzeit",
        ),
        (
            "fmt('de-DE', { timeZone: 'America/New_York', timeStyle: 'full' }, T)",
            "10:04:05 Nordamerikanische Ostküsten-Normalzeit",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/London', timeStyle: 'full' }, T)",
            "15:04:05 Mittlere Greenwich-Zeit",
        ),
        // Node 22 gives 15:04:05 GMT here.
        (
            "fmt('de-DE', { timeZone: 'Europe/London', timeStyle: 'long' }, T)",
            "15:04:05 GMT+0",
        ),
        (
            "fmt('de-DE', { timeZone: 'Asia/Tokyo', timeStyle: 'full' }, T)",
            "00:04:05 Japanische Normalzeit",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeZoneName: 'shortGeneric' }, T)",
            "11/10/2026, ET",
        ),
        (
            "fmt('en-US', { timeZone: 'America/New_York', timeZoneName: 'longGeneric' }, S)",
            "7/4/2026, Eastern Time",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Los_Angeles', hour: 'numeric', timeZoneName: 'shortGeneric' }, S)",
            "11 AM PT",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Phoenix', timeZoneName: 'shortGeneric' }, S)",
            "7/4/2026, MST",
        ),
        (
            "fmt('en-US', { timeZone: 'America/Phoenix', timeZoneName: 'longGeneric' }, S)",
            "7/4/2026, Mountain Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Europe/Berlin', timeZoneName: 'longGeneric' }, T)",
            "11/10/2026, Central European Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Asia/Kolkata', timeZoneName: 'longGeneric' }, T)",
            "11/10/2026, India Standard Time",
        ),
        (
            "fmt('en-US', { timeZone: 'Australia/Sydney', timeZoneName: 'longGeneric' }, T)",
            "11/11/2026, Australian Eastern Time",
        ),
        (
            "fmt('en-GB', { timeZone: 'Europe/Paris', timeZoneName: 'shortGeneric' }, T)",
            "10/11/2026, CET",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/Berlin', timeZoneName: 'shortGeneric' }, S)",
            "4.7.2026, MEZ",
        ),
        (
            "fmt('de-DE', { timeZone: 'Europe/Berlin', timeZoneName: 'longGeneric' }, S)",
            "4.7.2026, Mitteleuropäische Zeit",
        ),
    ]);
}

/// en-GB and de-DE: day first, their own separators, 24-hour times.
#[test]
fn en_gb_and_de_dates() {
    check_dates(&[
        ("fmt('en-GB', { timeZone: 'UTC' }, M)", "05/03/2027"),
        (
            "fmt('en-GB', { timeZone: 'UTC', year: 'numeric', month: '2-digit', day: '2-digit' }, M)",
            "05/03/2027",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', year: '2-digit', month: 'numeric', day: 'numeric' }, M)",
            "05/03/27",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', month: 'numeric', day: 'numeric' }, M)",
            "05/03",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'full' }, T)",
            "Tuesday, 10 November 2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'long' }, T)",
            "10 November 2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'medium' }, T)",
            "10 Nov 2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'short' }, T)",
            "10/11/2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "10 Nov 2026, 15:04",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', dateStyle: 'long', timeStyle: 'short' }, T)",
            "10 November 2026 at 15:04",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', timeStyle: 'medium', hour12: true }, T)",
            "03:04:05 pm",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', hour: 'numeric', minute: '2-digit' }, M)",
            "0:09",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', weekday: 'short', day: 'numeric', month: 'short' }, T)",
            "Tue 10 Nov",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' }, T)",
            "Tuesday, 10 November 2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', month: 'short', year: 'numeric' }, Date.UTC(2026, 8, 15))",
            "Sept 2026",
        ),
        (
            "fmt('en-GB', { timeZone: 'UTC', year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit', second: '2-digit' }, T)",
            "10/11/2026, 15:04:05",
        ),
        (
            "new Date(T).toLocaleDateString('en-GB', { timeZone: 'UTC' })",
            "10/11/2026",
        ),
        (
            "new Date(T).toLocaleString('en-GB', { timeZone: 'UTC' })",
            "10/11/2026, 15:04:05",
        ),
        ("fmt('de-DE', { timeZone: 'UTC' }, M)", "5.3.2027"),
        (
            "fmt('de-DE', { timeZone: 'UTC', year: 'numeric', month: '2-digit', day: '2-digit' }, M)",
            "05.03.2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', month: 'numeric', day: 'numeric' }, M)",
            "5.3.",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', year: 'numeric', month: 'numeric' }, M)",
            "3/2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'full' }, M)",
            "Freitag, 5. März 2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'long' }, M)",
            "5. März 2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'medium' }, M)",
            "05.03.2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'short' }, M)",
            "05.03.27",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'medium', timeStyle: 'short' }, T)",
            "10.11.2026, 15:04",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', dateStyle: 'long', timeStyle: 'short' }, T)",
            "10. November 2026 um 15:04",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', hour: 'numeric' }, T)",
            "15 Uhr",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', hour: 'numeric', minute: '2-digit', hour12: true }, T)",
            "3:04 PM",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', weekday: 'short', day: 'numeric', month: 'short' }, M)",
            "Fr., 5. März",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', month: 'short' }, M)",
            "Mär",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', weekday: 'short' }, M)",
            "Fr",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', month: 'long', year: 'numeric' }, M)",
            "März 2027",
        ),
        (
            "fmt('de-DE', { timeZone: 'UTC', minute: 'numeric', second: 'numeric', fractionalSecondDigits: 2 }, T)",
            "04:05,67",
        ),
        (
            "fmt('de', { timeZone: 'Europe/Berlin', dateStyle: 'medium', timeStyle: 'medium' }, S)",
            "04.07.2026, 20:30:15",
        ),
        (
            "new Date(M).toLocaleDateString('de-DE', { timeZone: 'UTC' })",
            "5.3.2027",
        ),
        (
            "[new Intl.DateTimeFormat('de-DE').resolvedOptions().locale, new Intl.DateTimeFormat('en-GB', { hour: 'numeric' }).resolvedOptions().hourCycle, new Intl.DateTimeFormat('de-DE', { hour: 'numeric', hour12: true }).resolvedOptions().hourCycle, Intl.DateTimeFormat.supportedLocalesOf(['de-DE', 'en-GB', 'de'])]",
            "de-DE,h23,h12,de-DE,en-GB,de",
        ),
    ]);
}

/// Field combinations that have no pattern of their own in the locale data
/// and are put together the way ICU does it.
#[test]
fn pattern_selection() {
    check_dates(&[
        (
            "fmt('en-US', { timeZone: 'UTC', year: 'numeric', month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' }, T)",
            "November 10, 2026 at 3:04 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', year: 'numeric', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }, T)",
            "Nov 10, 2026, 3:04 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', weekday: 'long', month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit', timeZoneName: 'short' }, T)",
            "Tuesday, November 10 at 3:04 PM UTC",
        ),
        // Node 22 gives Tue, 3 PM here.
        (
            "fmt('en-US', { timeZone: 'UTC', weekday: 'short', hour: 'numeric' }, T)",
            "Tue 3 PM",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', weekday: 'short', day: 'numeric' }, T)",
            "10 Tue",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', hour: 'numeric', hour12: false }, M)",
            "00",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', hour: 'numeric', minute: 'numeric', hour12: false }, M)",
            "00:09",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', minute: 'numeric' }, T)",
            "4",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', year: 'numeric', month: '2-digit' }, M)",
            "03/2027",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', dateStyle: 'long', timeStyle: 'short', hour12: false }, T)",
            "November 10, 2026 at 15:04",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', dateStyle: 'full', timeStyle: 'long' }, T)",
            "Tuesday, November 10, 2026 at 3:04:05 PM UTC",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', hour: 'numeric', dayPeriod: 'short' }, M)",
            "12 in the morning",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', hour: 'numeric', dayPeriod: 'long' }, 1804248000000)",
            "12 noon",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', hour: 'numeric', minute: 'numeric', dayPeriod: 'long' }, 1804249800000)",
            "12:30 in the afternoon",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', era: 'short', year: 'numeric' }, Date.UTC(-500, 5, 1))",
            "501 BC",
        ),
        (
            "fmt('en-US', { timeZone: 'UTC', year: 'numeric' }, Date.UTC(-500, 5, 1))",
            "501",
        ),
    ]);
}

#[test]
fn collator_and_locale_compare() {
    let js = page();
    let out = eval(
        &js,
        r#"[
          ['b', 'a', 'B', 'A', 'á', 'c'].sort(new Intl.Collator().compare),
          ['item10', 'item2', 'item1'].sort(new Intl.Collator('en', { numeric: true }).compare),
          new Intl.Collator('en', { sensitivity: 'base' }).compare('a', 'Á'),
          new Intl.Collator('en', { sensitivity: 'accent' }).compare('a', 'á'),
          new Intl.Collator('en', { sensitivity: 'case' }).compare('a', 'á'),
          new Intl.Collator('en').compare('a', 'a'),
          'a'.localeCompare('B'),
          'résumé'.localeCompare('RESUME', undefined, { sensitivity: 'base' }),
          'file10'.localeCompare('file9', undefined, { numeric: true }),
          new Intl.Collator().resolvedOptions(),
        ]"#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[["a","A","á","b","B","c"],["item1","item2","item10"],0,-1,0,0,-1,0,1,"#,
            r#"{"locale":"en-US","usage":"sort","sensitivity":"variant","ignorePunctuation":false,"#,
            r#""collation":"default","numeric":false,"caseFirst":"false"}]"#
        )
    );
}

#[test]
fn relative_time_format() {
    check(&[
        (
            "new Intl.RelativeTimeFormat('en').format(3, 'day')",
            "in 3 days",
        ),
        (
            "new Intl.RelativeTimeFormat('en').format(-1, 'days')",
            "1 day ago",
        ),
        (
            "new Intl.RelativeTimeFormat('en').format(-1234, 'hour')",
            "1,234 hours ago",
        ),
        (
            "new Intl.RelativeTimeFormat('en').format(1.5, 'year')",
            "in 1.5 years",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).format(-1, 'day')",
            "yesterday",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).format(0, 'day')",
            "today",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).format(1, 'week')",
            "next week",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).format(0, 'second')",
            "now",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { numeric: 'auto' }).format(-2, 'day')",
            "2 days ago",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { style: 'short' }).format(5, 'minute')",
            "in 5 min.",
        ),
        (
            "new Intl.RelativeTimeFormat('en', { style: 'short' }).format(-2, 'quarter')",
            "2 qtrs. ago",
        ),
    ]);
    let js = page();
    let out = eval(
        &js,
        r#"[
          new Intl.RelativeTimeFormat('en').formatToParts(-2, 'month'),
          (() => { try { new Intl.RelativeTimeFormat('en').format(1, 'fortnight'); } catch (e) { return e.name; } })(),
          new Intl.RelativeTimeFormat('en-US', { style: 'narrow' }).resolvedOptions(),
        ]"#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[[{"type":"integer","value":"2","unit":"month"},{"type":"literal","value":" months ago"}],"RangeError","#,
            r#"{"locale":"en-US","style":"narrow","numeric":"always","numberingSystem":"latn"}]"#
        )
    );
}

#[test]
fn list_format() {
    check(&[
        (
            "new Intl.ListFormat('en').format(['a', 'b', 'c'])",
            "a, b, and c",
        ),
        ("new Intl.ListFormat('en').format(['a', 'b'])", "a and b"),
        ("new Intl.ListFormat('en').format(['a'])", "a"),
        ("new Intl.ListFormat('en').format([])", ""),
        (
            "new Intl.ListFormat('en', { type: 'disjunction' }).format(['a', 'b', 'c'])",
            "a, b, or c",
        ),
        (
            "new Intl.ListFormat('en', { style: 'short' }).format(['a', 'b', 'c'])",
            "a, b, & c",
        ),
        (
            "new Intl.ListFormat('en', { type: 'unit' }).format(['5 lb', '3 oz'])",
            "5 lb, 3 oz",
        ),
        (
            "new Intl.ListFormat('en', { type: 'unit', style: 'narrow' }).format(['5lb', '3oz'])",
            "5lb 3oz",
        ),
        (
            "new Intl.ListFormat('en').format(new Set(['x', 'y']))",
            "x and y",
        ),
    ]);
    let js = page();
    let out = eval(
        &js,
        r#"[
          new Intl.ListFormat('en').formatToParts(['a', 'b']),
          (() => { try { new Intl.ListFormat('en').format([1]); } catch (e) { return e.name; } })(),
        ]"#,
    );
    assert_eq!(
        out,
        r#"[[{"type":"element","value":"a"},{"type":"literal","value":" and "},{"type":"element","value":"b"}],"TypeError"]"#
    );
}

/// Intl is evaluated on first use. Until then the global is an accessor;
/// a page that replaces it, or a built-in method, keeps its replacement.
// Expected values are Node 22's (ICU), for tags within what LIKELY knows.
#[test]
fn locale() {
    check(&[
        ("new Intl.Locale('en-US').language", "en"),
        ("new Intl.Locale('en-US').region", "US"),
        ("new Intl.Locale('en-US').script", "undefined"),
        ("new Intl.Locale('EN-latn-us').toString()", "en-Latn-US"),
        ("new Intl.Locale('en-Latn-US').baseName", "en-Latn-US"),
        ("new Intl.Locale('zh-hant-tw').script", "Hant"),
        ("new Intl.Locale('es-419').region", "419"),
        ("new Intl.Locale('de').region", "undefined"),
        (
            "new Intl.Locale('sl-rozaj-biske').baseName",
            "sl-biske-rozaj",
        ),
        (
            "new Intl.Locale('en-US-u-ca-buddhist-nu-thai-hc-h23').calendar",
            "buddhist",
        ),
        (
            "new Intl.Locale('en-US-u-ca-buddhist-nu-thai-hc-h23').numberingSystem",
            "thai",
        ),
        (
            "new Intl.Locale('en-US-u-ca-buddhist-nu-thai-hc-h23').hourCycle",
            "h23",
        ),
        (
            "new Intl.Locale('en-US-u-ca-buddhist-nu-thai-hc-h23').baseName",
            "en-US",
        ),
        (
            "new Intl.Locale('en-US-u-nu-thai-ca-buddhist').toString()",
            "en-US-u-ca-buddhist-nu-thai",
        ),
        ("new Intl.Locale('en-u-kn').numeric", "true"),
        ("new Intl.Locale('en-u-kn-true').toString()", "en-u-kn"),
        ("new Intl.Locale('en-u-kn-false').numeric", "false"),
        ("new Intl.Locale('en').numeric", "false"),
        ("new Intl.Locale('en').calendar", "undefined"),
        ("new Intl.Locale('de-u-co-phonebk').collation", "phonebk"),
        ("new Intl.Locale('en-u-kf-upper').caseFirst", "upper"),
        (
            "new Intl.Locale('en', { region: 'gb' }).toString()",
            "en-GB",
        ),
        (
            "new Intl.Locale('en-US', { language: 'fr', script: 'latn' }).toString()",
            "fr-Latn-US",
        ),
        (
            "new Intl.Locale('en-US', { calendar: 'gregory', hourCycle: 'h12', numeric: true, caseFirst: 'lower', collation: 'emoji', numberingSystem: 'latn' }).toString()",
            "en-US-u-ca-gregory-co-emoji-hc-h12-kf-lower-kn-nu-latn",
        ),
        (
            "new Intl.Locale('en-u-ca-buddhist', { calendar: 'gregory' }).toString()",
            "en-u-ca-gregory",
        ),
        (
            "new Intl.Locale('en', { numeric: false }).toString()",
            "en-u-kn-false",
        ),
        ("new Intl.Locale('en-x-private').toString()", "en-x-private"),
        (
            "new Intl.Locale('en-t-ja-u-ca-gregory').toString()",
            "en-t-ja-u-ca-gregory",
        ),
        (
            "new Intl.Locale(new Intl.Locale('fr-CA')).toString()",
            "fr-CA",
        ),
        ("String(new Intl.Locale('pt-br'))", "pt-BR"),
        (
            "Object.prototype.toString.call(new Intl.Locale('en'))",
            "[object Intl.Locale]",
        ),
        ("new Intl.Locale('en').maximize().toString()", "en-Latn-US"),
        (
            "new Intl.Locale('en-GB').maximize().toString()",
            "en-Latn-GB",
        ),
        ("new Intl.Locale('de').maximize().toString()", "de-Latn-DE"),
        ("new Intl.Locale('zh').maximize().toString()", "zh-Hans-CN"),
        (
            "new Intl.Locale('zh-TW').maximize().toString()",
            "zh-Hant-TW",
        ),
        (
            "new Intl.Locale('zh-Hant').maximize().toString()",
            "zh-Hant-TW",
        ),
        (
            "new Intl.Locale('zh-HK').maximize().toString()",
            "zh-Hant-HK",
        ),
        ("new Intl.Locale('ja').maximize().region", "JP"),
        ("new Intl.Locale('pt').maximize().toString()", "pt-Latn-BR"),
        (
            "new Intl.Locale('pt-PT').maximize().toString()",
            "pt-Latn-PT",
        ),
        ("new Intl.Locale('und').maximize().toString()", "en-Latn-US"),
        (
            "new Intl.Locale('en-u-ca-gregory').maximize().toString()",
            "en-Latn-US-u-ca-gregory",
        ),
        ("new Intl.Locale('en-Latn-US').minimize().toString()", "en"),
        (
            "new Intl.Locale('en-Latn-GB').minimize().toString()",
            "en-GB",
        ),
        (
            "new Intl.Locale('zh-Hant-TW').minimize().toString()",
            "zh-TW",
        ),
        ("new Intl.Locale('zh-Hans-CN').minimize().toString()", "zh"),
        (
            "new Intl.Locale('zh-Hant-HK').minimize().toString()",
            "zh-HK",
        ),
        (
            "new Intl.Locale('sr-Latn-RS').minimize().toString()",
            "sr-Latn",
        ),
        (
            "new Intl.Locale('de-DE-u-co-phonebk').minimize().toString()",
            "de-u-co-phonebk",
        ),
        ("new Intl.Locale('fr-CA').minimize().toString()", "fr-CA"),
        (
            "new Intl.NumberFormat(new Intl.Locale('en-US')).format(1234.5)",
            "1,234.5",
        ),
        (
            "Intl.getCanonicalLocales(new Intl.Locale('EN-gb'))",
            "en-GB",
        ),
        (
            "(() => { try { new Intl.Locale(); } catch (e) { return e.name; } })()",
            "TypeError",
        ),
        (
            "(() => { try { new Intl.Locale(5); } catch (e) { return e.name; } })()",
            "TypeError",
        ),
        (
            "(() => { try { new Intl.Locale('en_US'); } catch (e) { return e.name; } })()",
            "RangeError",
        ),
        (
            "(() => { try { new Intl.Locale(''); } catch (e) { return e.name; } })()",
            "RangeError",
        ),
        (
            "(() => { try { new Intl.Locale('en', { region: 'USA' }); } catch (e) { return e.name; } })()",
            "RangeError",
        ),
        (
            "(() => { try { new Intl.Locale('en', { hourCycle: 'h25' }); } catch (e) { return e.name; } })()",
            "RangeError",
        ),
        (
            "(() => { try { new Intl.Locale('en', { calendar: 'x' }); } catch (e) { return e.name; } })()",
            "RangeError",
        ),
        (
            "(() => { try { Intl.Locale('en'); } catch (e) { return e.name; } })()",
            "TypeError",
        ),
        (
            "typeof Object.getOwnPropertyDescriptor(Intl.Locale.prototype, 'language').get",
            "function",
        ),
        ("Object.keys(new Intl.Locale('en-US')).length", "0"),
        ("Intl.Locale.name", "Locale"),
    ]);
}

#[test]
fn loads_lazily() {
    let js = page();
    let out = eval(
        &js,
        r#"[
          typeof Object.getOwnPropertyDescriptor(globalThis, 'Intl').get,
          typeof Intl,
          'value' in Object.getOwnPropertyDescriptor(globalThis, 'Intl'),
          Intl === Intl,
        ]"#,
    );
    assert_eq!(out, r#"["function","object",true,true]"#);

    let js = page();
    let out = eval(
        &js,
        r#"
          globalThis.Intl = { polyfilled: true };
          const original = Number.prototype.toLocaleString;
          Number.prototype.toLocaleString = function () { return 'mine ' + original.call(this); };
          [Intl.polyfilled, (1234).toLocaleString(), Intl.polyfilled, (5).toLocaleString.name, original.name]
        "#,
    );
    assert_eq!(out, r#"[true,"mine 1,234",true,"","toLocaleString"]"#);
}

/// Formatters are resolved once per set of options and kept: formatters
/// with the same options, made apart or per call, and the cached states of
/// toLocale*String format the same, and caches keep locales and options
/// apart.
#[test]
fn formatters_are_reused_without_mixing_options() {
    check(&[
        (
            "[0, 1, 2].map((i) => new Intl.DateTimeFormat('en-GB', { month: 'long', timeZone: 'UTC' }).format(864e5 * 40 * i)).join()",
            "January,February,March",
        ),
        (
            "[0, 1, 2].map((i) => new Intl.DateTimeFormat(i ? 'de' : 'en-US', { weekday: 'long', timeZone: i > 1 ? 'Asia/Tokyo' : 'UTC' }).format(0)).join()",
            "Thursday,Donnerstag,Donnerstag",
        ),
        (
            "['en-GB', 'de', 'en-GB', undefined].map((l) => new Date(Date.UTC(2026, 0, 2, 15)).toLocaleDateString(l, { timeZone: 'UTC' })).join(' ')",
            "02/01/2026 2.1.2026 02/01/2026 1/2/2026",
        ),
        (
            "['en-GB', 'de', 'en-GB'].map((l) => new Date(Date.UTC(2026, 0, 2, 15)).toLocaleTimeString(l, { timeZone: 'UTC' }) + '|' + new Date(Date.UTC(2026, 0, 2, 15)).toLocaleString(l, { timeZone: 'UTC' })).join(' ')",
            "15:00:00|02/01/2026, 15:00:00 15:00:00|2.1.2026, 15:00:00 15:00:00|02/01/2026, 15:00:00",
        ),
        (
            "[1234.5, 1234.5].map((n, i) => n.toLocaleString(i ? 'de' : 'en', { style: i ? 'percent' : 'decimal' })).join(' ')",
            "1,234.5 123,450%",
        ),
        (
            "[new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(2.5), new Intl.NumberFormat('en', { maximumFractionDigits: 0 }).format(-2.5), (2.5).toLocaleString('en')].join()",
            "3,-3,2.5",
        ),
    ]);
}

/// What format and formatToParts do with their argument: Number() it
/// (through valueOf), use the page's Date.now() for undefined, and throw
/// for times out of range. A time in range whose local time is not has
/// NaN fields and no names, and no day period at all.
#[test]
fn format_arguments() {
    let js = page();
    let out = eval(
        &js,
        r#"
        const f = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', year: 'numeric', month: 'short', hour: 'numeric' });
        const now = Date.now;
        Date.now = () => 0;
        const r = [f.format(), f.format({ valueOf: () => 864e5 }), f.format(new Date(1.5e12)), f.format('86400000')];
        Date.now = now;
        for (const x of [NaN, Infinity, 8.64e15 + 1]) {
          try { f.format(x); r.push('no error'); } catch (e) { r.push(e.name + ': ' + e.message); }
        }
        const far = new Intl.DateTimeFormat('en-US', { timeZone: 'Asia/Tokyo', month: 'long', day: 'numeric', hour: 'numeric' });
        r.push(far.format(8.64e15), JSON.stringify(far.formatToParts(8.64e15)));
        const period = new Intl.DateTimeFormat('en-US', { timeZone: 'Asia/Tokyo', dayPeriod: 'long' });
        try { period.format(8.64e15); } catch (e) { r.push(e.name + ': ' + e.message); }
        const n = new Intl.NumberFormat('en');
        r.push(n.format({ valueOf: () => 1e6 }), n.format('12.5'), n.format(12345678901234567890n), n.format(-0));
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["Jan 1970, 12 AM","Jan 1970, 12 AM","Jul 2017, 2 AM","Jan 1970, 12 AM","RangeError: Invalid time value","RangeError: Invalid time value","RangeError: Invalid time value"," NaN at 12 AM","[{\"type\":\"month\"},{\"type\":\"literal\",\"value\":\" \"},{\"type\":\"day\",\"value\":\"NaN\"},{\"type\":\"literal\",\"value\":\" at \"},{\"type\":\"hour\",\"value\":\"12\"},{\"type\":\"literal\",\"value\":\" \"},{\"type\":\"dayPeriod\",\"value\":\"AM\"}]","TypeError: cannot read property '1' of undefined","1,000,000","12.5","12,345,678,901,234,567,890","-0"]"#
    );
}

/// The built-in methods Intl makes locale-sensitive keep their shape, pass
/// their receiver and arguments on, and still check their receiver.
#[test]
fn locale_methods_pass_their_receiver_and_arguments() {
    let js = page();
    let out = eval(
        &js,
        r#"[
          Number.prototype.toLocaleString.length, Date.prototype.toLocaleDateString.length,
          String.prototype.localeCompare.length, Date.prototype.toLocaleTimeString.name,
          Number.prototype.toLocaleString.call(new Number(1234), 'en', { minimumFractionDigits: 1 }),
          BigInt.prototype.toLocaleString.call(10n ** 6n),
          'a'.localeCompare('B', undefined, { sensitivity: 'base' }),
          'item10'.localeCompare('item9', 'en', { numeric: true }),
          new Date(NaN).toLocaleString(),
          (() => { try { Date.prototype.toLocaleString.call({}); } catch (e) { return e.name; } })(),
          (() => { try { Number.prototype.toLocaleString.call('1'); } catch (e) { return e.name; } })(),
        ]"#,
    );
    assert_eq!(
        out,
        r#"[0,0,0,"toLocaleTimeString","1,234.0","1,000,000",-1,1,"Invalid Date","TypeError","TypeError"]"#
    );
}

/// The lists supportedValuesOf gives come from the Rust tables.
#[test]
fn supported_values_come_from_the_tables() {
    let js = page();
    let out = eval(
        &js,
        r#"{
          const zones = Intl.supportedValuesOf('timeZone');
          const units = Intl.supportedValuesOf('unit');
          const currencies = Intl.supportedValuesOf('currency');
          [zones.length > 400, zones.includes('America/New_York'), zones.some((z) => z.startsWith('Etc') || !z.includes('/')),
           zones.join() === zones.slice().sort().join(), units.length, units[0], units.includes('kilometer'),
           currencies.length, currencies[0], currencies.join() === currencies.slice().sort().join(),
           zones !== Intl.supportedValuesOf('timeZone')]
        }"#,
    );
    assert_eq!(
        out,
        r#"[true,true,false,true,45,"acre",true,35,"AED",true,true]"#
    );
}
