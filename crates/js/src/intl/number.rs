//! Number formatting for `Intl.NumberFormat`, `toLocaleString` and plural
//! rules: en-US whatever the locale. Numbers are rounded as decimal
//! strings, so rounding is exact.
//!
//! `intl.js` reads and checks the options into a resolvedOptions object;
//! a [`NumberFormat`] is built from that, once per set of options.

use rquickjs::{Object, Value};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Style {
    Decimal,
    Percent,
    Currency,
    Unit,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CurrencyDisplay {
    Code,
    Symbol,
    NarrowSymbol,
    Name,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum UnitDisplay {
    Short,
    Narrow,
    Long,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Notation {
    Standard,
    Scientific,
    Engineering,
    Compact,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Grouping {
    Off,
    Min2,
    Auto,
    Always,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SignDisplay {
    Auto,
    Never,
    Always,
    ExceptZero,
    Negative,
}

/// The digit options of a NumberFormat or PluralRules.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Digits {
    min_int: usize,
    min_frac: i64,
    max_frac: i64,
    /// Both set, or neither.
    min_sig: i64,
    max_sig: Option<i64>,
    strip_if_integer: bool,
}

/// What a set of NumberFormat options resolves to.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct NumberFormat {
    style: Style,
    currency: String,
    currency_display: CurrencyDisplay,
    accounting: bool,
    unit: String,
    unit_display: UnitDisplay,
    notation: Notation,
    compact_long: bool,
    digits: Digits,
    /// Compact notation's own rounding: two significant digits below 100.
    auto: bool,
    grouping: Grouping,
    sign: SignDisplay,
}

/// A value to format: a number, or a bigint as its decimal digits.
pub(super) enum Num {
    Float(f64),
    Big { neg: bool, digits: String },
}

impl Num {
    pub(super) fn from_js(value: &Value<'_>) -> rquickjs::Result<Self> {
        if let Some(s) = value.as_string() {
            let s = s.to_string()?;
            return Ok(match s.strip_prefix('-') {
                Some(digits) => Num::Big {
                    neg: true,
                    digits: digits.to_owned(),
                },
                None => Num::Big {
                    neg: false,
                    digits: s,
                },
            });
        }
        Ok(Num::Float(value.as_number().unwrap_or(f64::NAN)))
    }
}

/// Currency code, symbol and English name ('' when there is none).
const CURRENCIES: &[(&str, &str, &str)] = &[
    ("USD", "$", "US dollar"),
    ("EUR", "€", "euro"),
    ("GBP", "£", "British pound"),
    ("JPY", "¥", "Japanese yen"),
    ("CNY", "CN¥", "Chinese yuan"),
    ("INR", "₹", "Indian rupee"),
    ("KRW", "₩", "South Korean won"),
    ("CAD", "CA$", "Canadian dollar"),
    ("AUD", "A$", "Australian dollar"),
    ("NZD", "NZ$", "New Zealand dollar"),
    ("HKD", "HK$", "Hong Kong dollar"),
    ("TWD", "NT$", "New Taiwan dollar"),
    ("MXN", "MX$", "Mexican peso"),
    ("BRL", "R$", "Brazilian real"),
    ("ILS", "₪", "Israeli new shekel"),
    ("VND", "₫", "Vietnamese dong"),
    ("PHP", "₱", "Philippine peso"),
    ("XAF", "FCFA", "Central African CFA franc"),
    ("XOF", "F\u{202f}CFA", "West African CFA franc"),
    ("CHF", "CHF", "Swiss franc"),
    ("SEK", "SEK", "Swedish krona"),
    ("NOK", "NOK", "Norwegian krone"),
    ("DKK", "DKK", "Danish krone"),
    ("PLN", "PLN", "Polish zloty"),
    ("CZK", "CZK", "Czech koruna"),
    ("HUF", "HUF", "Hungarian forint"),
    ("RUB", "RUB", "Russian ruble"),
    ("TRY", "TRY", "Turkish lira"),
    ("ZAR", "ZAR", "South African rand"),
    ("SGD", "SGD", "Singapore dollar"),
    ("THB", "THB", "Thai baht"),
    ("IDR", "IDR", "Indonesian rupiah"),
    ("MYR", "MYR", "Malaysian ringgit"),
    ("AED", "AED", "UAE dirham"),
    ("SAR", "SAR", "Saudi riyal"),
];

/// Names that stay the same in the plural.
const NO_PLURAL: [&str; 11] = [
    "yen", "yuan", "won", "dong", "baht", "rand", "krona", "krone", "koruna", "franc", "real",
];

/// The currency codes with names, for `supportedValuesOf`.
pub(super) fn currencies() -> Vec<&'static str> {
    let mut list: Vec<&str> = CURRENCIES.iter().map(|c| c.0).collect();
    list.sort_unstable();
    list
}

/// Unit identifiers ECMA-402 sanctions, with short English labels. A
/// trailing '+' marks labels that take an 's' in the plural.
pub(super) const UNITS: &[(&str, &str)] = &[
    ("acre", "ac"),
    ("bit", "bit"),
    ("byte", "byte"),
    ("celsius", "°C"),
    ("centimeter", "cm"),
    ("day", "day+"),
    ("degree", "deg"),
    ("fahrenheit", "°F"),
    ("fluid-ounce", "fl oz"),
    ("foot", "ft"),
    ("gallon", "gal"),
    ("gigabit", "Gb"),
    ("gigabyte", "GB"),
    ("gram", "g"),
    ("hectare", "ha"),
    ("hour", "hr"),
    ("inch", "in"),
    ("kilobit", "kb"),
    ("kilobyte", "kB"),
    ("kilogram", "kg"),
    ("kilometer", "km"),
    ("liter", "L"),
    ("megabit", "Mb"),
    ("megabyte", "MB"),
    ("meter", "m"),
    ("microsecond", "μs"),
    ("mile", "mi"),
    ("mile-scandinavian", "smi"),
    ("milliliter", "mL"),
    ("millimeter", "mm"),
    ("millisecond", "ms"),
    ("minute", "min"),
    ("month", "mth+"),
    ("nanosecond", "ns"),
    ("ounce", "oz"),
    ("percent", "%"),
    ("petabyte", "PB"),
    ("pound", "lb"),
    ("second", "sec"),
    ("stone", "st"),
    ("terabit", "Tb"),
    ("terabyte", "TB"),
    ("week", "wk+"),
    ("yard", "yd"),
    ("year", "yr+"),
];

const COMPACT: [[&str; 5]; 2] = [
    ["", "K", "M", "B", "T"],
    ["", "thousand", "million", "billion", "trillion"],
];

fn unit_label(unit: &str, display: UnitDisplay, one: bool) -> String {
    let labels: Vec<String> = unit
        .split("-per-")
        .enumerate()
        .map(|(i, u)| {
            if display == UnitDisplay::Long {
                return if i > 0
                    || one
                    || u.ends_with('s')
                    || u.ends_with("ius")
                    || u.ends_with("heit")
                {
                    u.to_owned()
                } else {
                    format!("{u}s")
                };
            }
            let label = UNITS.iter().find(|x| x.0 == u).map_or("", |x| x.1);
            match label.strip_suffix('+') {
                Some(label) if i > 0 || one => label.to_owned(),
                Some(label) => format!("{label}s"),
                None => label.to_owned(),
            }
        })
        .collect();
    labels.join(if display == UnitDisplay::Long {
        " per "
    } else {
        "/"
    })
}

/// Reads an optional whole-number option of a resolvedOptions object.
fn int_opt(r: &Object<'_>, key: &str) -> rquickjs::Result<Option<i64>> {
    Ok(r.get::<_, Option<f64>>(key)?.map(|n| n as i64))
}

fn str_opt(r: &Object<'_>, key: &str) -> rquickjs::Result<String> {
    Ok(r.get::<_, Option<String>>(key)?.unwrap_or_default())
}

impl NumberFormat {
    /// The formatter for a resolvedOptions object as `intl.js` builds it
    /// (NumberFormat's, or PluralRules' digit options), `auto` when compact
    /// notation rounds its own way.
    pub(super) fn from_options(r: &Object<'_>, auto: bool) -> rquickjs::Result<Self> {
        let style = match str_opt(r, "style")?.as_str() {
            "percent" => Style::Percent,
            "currency" => Style::Currency,
            "unit" => Style::Unit,
            _ => Style::Decimal,
        };
        let grouping: Value<'_> = r.get("useGrouping")?;
        let grouping = match grouping
            .as_string()
            .map(|s| s.to_string())
            .transpose()?
            .as_deref()
        {
            Some("min2") => Grouping::Min2,
            Some("auto") => Grouping::Auto,
            Some("always") => Grouping::Always,
            _ => Grouping::Off,
        };
        Ok(NumberFormat {
            style,
            currency: str_opt(r, "currency")?,
            currency_display: match str_opt(r, "currencyDisplay")?.as_str() {
                "code" => CurrencyDisplay::Code,
                "narrowSymbol" => CurrencyDisplay::NarrowSymbol,
                "name" => CurrencyDisplay::Name,
                _ => CurrencyDisplay::Symbol,
            },
            accounting: str_opt(r, "currencySign")? == "accounting",
            unit: str_opt(r, "unit")?,
            unit_display: match str_opt(r, "unitDisplay")?.as_str() {
                "narrow" => UnitDisplay::Narrow,
                "long" => UnitDisplay::Long,
                _ => UnitDisplay::Short,
            },
            notation: match str_opt(r, "notation")?.as_str() {
                "scientific" => Notation::Scientific,
                "engineering" => Notation::Engineering,
                "compact" => Notation::Compact,
                _ => Notation::Standard,
            },
            compact_long: str_opt(r, "compactDisplay")? == "long",
            digits: Digits {
                min_int: int_opt(r, "minimumIntegerDigits")?.unwrap_or(1) as usize,
                min_frac: int_opt(r, "minimumFractionDigits")?.unwrap_or(0),
                max_frac: int_opt(r, "maximumFractionDigits")?.unwrap_or(0),
                min_sig: int_opt(r, "minimumSignificantDigits")?.unwrap_or(0),
                max_sig: int_opt(r, "maximumSignificantDigits")?,
                strip_if_integer: str_opt(r, "trailingZeroDisplay")? == "stripIfInteger",
            },
            auto,
            grouping,
            sign: match str_opt(r, "signDisplay")?.as_str() {
                "never" => SignDisplay::Never,
                "always" => SignDisplay::Always,
                "exceptZero" => SignDisplay::ExceptZero,
                "negative" => SignDisplay::Negative,
                _ => SignDisplay::Auto,
            },
        })
    }

    /// Gives each part of `x` to `out`, as (type, value).
    pub(super) fn emit(&self, x: &Num, out: &mut dyn FnMut(&'static str, &str)) {
        let (neg, nan, decimal) = match x {
            Num::Float(x) => {
                let neg = *x < 0.0 || (*x == 0.0 && x.is_sign_negative());
                let x = x.abs();
                (neg, x.is_nan(), x.is_finite().then(|| decimal_of(x)))
            }
            Num::Big { neg, digits } => (
                *neg,
                false,
                Some(if digits == "0" {
                    (b"0".to_vec(), 1)
                } else {
                    (digits.as_bytes().to_vec(), digits.len() as i64)
                }),
            ),
        };
        // The rounded digits and exponent, first: the sign depends on them.
        let rounded = decimal.map(|(d, point)| {
            let p = if self.style == Style::Percent {
                point + 2
            } else {
                point
            };
            let n = self.notation;
            let is_zero = d == b"0";
            let exponent = |q: i64| -> i64 {
                if n == Notation::Standard || is_zero {
                    0
                } else if n == Notation::Scientific {
                    q - 1
                } else if n == Notation::Engineering || q > 3 {
                    let e = (q - 1).div_euclid(3) * 3;
                    if n == Notation::Compact { e.min(12) } else { e }
                } else {
                    0
                }
            };
            let mut e = exponent(p);
            let (mut int, mut frac) = self.round(&d, p - e, self.auto);
            // Rounding up may carry into the next exponent (999,950 is 1M).
            if exponent(p + 1) != e && strip_leading_zeros(&int).len() as i64 > p - e {
                e = exponent(p + 1);
                (int, frac) = self.round(&d, p - e, self.auto);
            }
            (int, frac, e)
        });
        let (zero, one) = match &rounded {
            Some((int, frac, e)) => (
                !int.bytes()
                    .chain(frac.bytes())
                    .any(|b| (b'1'..=b'9').contains(&b)),
                int == "1" && frac.is_empty() && *e == 0,
            ),
            None => (false, false),
        };
        let sign = match self.sign {
            _ if nan => "",
            SignDisplay::Never => "",
            SignDisplay::Auto => {
                if neg {
                    "-"
                } else {
                    ""
                }
            }
            SignDisplay::Always => {
                if neg {
                    "-"
                } else {
                    "+"
                }
            }
            _ if zero => "",
            _ if neg => "-",
            SignDisplay::ExceptZero => "+",
            SignDisplay::Negative => "",
        };
        let accounting = sign == "-" && self.accounting;
        if accounting {
            out("literal", "(");
        } else if !sign.is_empty() {
            out(if sign == "-" { "minusSign" } else { "plusSign" }, sign);
        }
        let currency = if self.style == Style::Currency {
            self.currency.as_str()
        } else {
            ""
        };
        let known = CURRENCIES
            .iter()
            .find(|c| c.0 == currency)
            .map_or((currency, ""), |c| (c.1, c.2));
        if !currency.is_empty() && self.currency_display != CurrencyDisplay::Name {
            let mut symbol = if self.currency_display == CurrencyDisplay::Code {
                currency
            } else {
                known.0
            };
            if self.currency_display == CurrencyDisplay::NarrowSymbol {
                // /^[A-Z]+(?=[^A-Z])/
                let run = symbol.bytes().take_while(u8::is_ascii_uppercase).count();
                if run > 0 && run < symbol.len() {
                    symbol = &symbol[run..];
                }
            }
            out("currency", symbol);
            if symbol
                .bytes()
                .last()
                .is_some_and(|b| b.is_ascii_uppercase())
            {
                out("literal", "\u{a0}");
            }
        }
        match &rounded {
            None if nan => out("nan", "NaN"),
            None => out("infinity", "∞"),
            Some((int, frac, e)) => {
                let g = self.grouping;
                if g != Grouping::Off && int.len() > if g == Grouping::Min2 { 4 } else { 3 } {
                    let mut i = match int.len() % 3 {
                        0 => 3,
                        r => r,
                    };
                    out("integer", &int[..i]);
                    while i < int.len() {
                        out("group", ",");
                        out("integer", &int[i..i + 3]);
                        i += 3;
                    }
                } else {
                    out("integer", int);
                }
                if !frac.is_empty() {
                    out("decimal", ".");
                    out("fraction", frac);
                }
                if self.notation == Notation::Compact {
                    if *e != 0 {
                        if self.compact_long {
                            out("literal", " ");
                        }
                        out(
                            "compact",
                            COMPACT[usize::from(self.compact_long)][(e / 3) as usize],
                        );
                    }
                } else if self.notation != Notation::Standard {
                    out("exponentSeparator", "E");
                    if *e < 0 {
                        out("exponentMinusSign", "-");
                    }
                    out("exponentInteger", &e.abs().to_string());
                }
            }
        }
        match self.style {
            Style::Percent => out("percentSign", "%"),
            Style::Unit => {
                let label = unit_label(&self.unit, self.unit_display, one);
                if self.unit_display != UnitDisplay::Narrow && !label.starts_with(['%', '°']) {
                    out("literal", " ");
                }
                out("unit", &label);
            }
            _ if !currency.is_empty() && self.currency_display == CurrencyDisplay::Name => {
                let name = if known.1.is_empty() {
                    currency
                } else {
                    known.1
                };
                out("literal", " ");
                let same = one || known.1.is_empty() || NO_PLURAL.iter().any(|w| name.ends_with(w));
                if same {
                    out("currency", name);
                } else {
                    out("currency", &format!("{name}s"));
                }
            }
            _ => {}
        }
        if accounting {
            out("literal", ")");
        }
    }

    /// The parts of `x`, as (type, value).
    pub(super) fn parts(&self, x: &Num) -> Vec<(&'static str, String)> {
        let mut parts = Vec::new();
        self.emit(x, &mut |ty, value| parts.push((ty, value.to_owned())));
        parts
    }

    /// The formatted string of `x`.
    pub(super) fn format(&self, x: &Num) -> String {
        let mut s = String::new();
        self.emit(x, &mut |_, value| s.push_str(value));
        s
    }

    /// Rounds 0.d × 10^p half away from zero per the digit options and
    /// returns (integer digits, fraction digits).
    fn round(&self, d: &[u8], p: i64, auto: bool) -> (String, String) {
        let r = &self.digits;
        let mut sig = r.max_sig.is_some();
        let (mut mnsd, mut mxsd) = (r.min_sig, r.max_sig.unwrap_or(0));
        let (mut mnfd, mut mxfd) = (r.min_frac, r.max_frac);
        if auto {
            sig = p < 2;
            mnsd = 1;
            mxsd = 2;
            mnfd = 0;
            mxfd = 0;
        }
        let mut d: Vec<u8> = std::iter::once(b'0').chain(d.iter().copied()).collect();
        let mut p = p + 1;
        let keep = if sig { mxsd + 1 } else { p + mxfd };
        if d.len() as i64 > keep {
            let up = keep >= 0 && d[keep as usize] >= b'5';
            d.truncate(keep.max(0) as usize);
            if up {
                let mut i = d.len() - 1;
                while d[i] == b'9' {
                    i -= 1;
                }
                d[i] += 1;
                for x in &mut d[i + 1..] {
                    *x = b'0';
                }
            }
        }
        if p <= 0 {
            let mut padded = vec![b'0'; (1 - p) as usize];
            padded.extend_from_slice(&d);
            d = padded;
            p = 1;
        }
        let p = p as usize;
        if d.len() < p {
            d.resize(p, b'0');
        }
        let int = strip_leading_zeros(std::str::from_utf8(&d[..p]).unwrap_or("0")).to_owned();
        let mut frac = std::str::from_utf8(&d[p..])
            .unwrap_or("")
            .trim_end_matches('0')
            .to_owned();
        let need = if !sig {
            mnfd
        } else if int != "0" {
            mnsd - int.len() as i64
        } else if !frac.is_empty() {
            mnsd + frac.bytes().position(|b| b != b'0').unwrap_or(0) as i64
        } else {
            mnsd - 1
        };
        while (frac.len() as i64) < need {
            frac.push('0');
        }
        if r.strip_if_integer && !frac.bytes().any(|b| b != b'0') {
            frac.clear();
        }
        let int = if int.len() < r.min_int {
            "0".repeat(r.min_int - int.len()) + &int
        } else {
            int
        };
        (int, frac)
    }

    /// The English plural category of `n` (finite), cardinal or ordinal.
    pub(super) fn plural(&self, n: f64, ordinal: bool) -> &'static str {
        let (d, p) = decimal_of(n.abs());
        let (int, frac) = self.round(&d, p, false);
        if !ordinal {
            return if strip_leading_zeros(&int) == "1" && frac.is_empty() {
                "one"
            } else {
                "other"
            };
        }
        if !frac.is_empty() {
            return "other";
        }
        let a = &int[int.len() - 1..];
        let b = if int.len() >= 2 {
            int[int.len() - 2..].to_owned()
        } else {
            format!("0{int}")
        };
        match (a, b.as_str()) {
            ("1", b) if b != "11" => "one",
            ("2", b) if b != "12" => "two",
            ("3", b) if b != "13" => "few",
            _ => "other",
        }
    }
}

/// `/^0+(?=.)/` removed: leading zeros, keeping the last digit.
fn strip_leading_zeros(s: &str) -> &str {
    let zeros = s.bytes().take_while(|&b| b == b'0').count();
    &s[zeros.min(s.len().saturating_sub(1))..]
}

/// A non-negative finite number as (digits, point): the value is
/// 0.digits × 10^point, digits starting with a non-zero digit. The digits
/// are the shortest that round-trip, as `toExponential()` gives them.
fn decimal_of(x: f64) -> (Vec<u8>, i64) {
    if x == 0.0 {
        return (b"0".to_vec(), 1);
    }
    let s = format!("{x:e}");
    let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
    let digits = mantissa.bytes().filter(|&b| b != b'.').collect();
    (digits, exponent.parse::<i64>().unwrap_or(0) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals() {
        assert_eq!(decimal_of(1234.5), (b"12345".to_vec(), 4));
        assert_eq!(decimal_of(0.000123), (b"123".to_vec(), -3));
        assert_eq!(decimal_of(1e21), (b"1".to_vec(), 22));
        assert_eq!(strip_leading_zeros("000"), "0");
        assert_eq!(strip_leading_zeros("0012"), "12");
    }
}
