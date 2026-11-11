//! The small en-US Intl implementation and the built-in toLocale* methods
//! it backs.

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
        r#"{"tag":"[object Intl]","enumerable":false,"members":["Collator","DateTimeFormat","ListFormat","NumberFormat","PluralRules","RelativeTimeFormat","getCanonicalLocales","supportedValuesOf"],"callable":[true,true,true],"name":"NumberFormat","ctor":true,"protoTag":"[object Intl.PluralRules]","needsNew":true}"#
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
                zone === 'UTC' ? offset === 0 : /^(Etc\/GMT[+-]\d+|[+-]\d\d:\d\d)$/.test(zone),
              ]
            }}"#
        ),
    );
    assert_eq!(out, "[true,true,true]");
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
