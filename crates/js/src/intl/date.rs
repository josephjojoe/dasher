//! Date formatting for `Intl.DateTimeFormat` and `Date`'s toLocale*String.
//!
//! Formatting follows ICU, which is what V8 formats with: the options
//! become a skeleton (`yMMdd`), the locale's closest pattern is found and
//! its field widths adjusted (`y-MM-dd`), and the pattern is filled in.
//! [`best_pattern`] is ICU's `DateTimePatternGenerator::getBestPattern`;
//! the locale tables hold CLDR's patterns for en-US, en-CA, en-GB and de.
//! They were checked against Chrome 157 over 27,854 option combinations
//! per locale.
//!
//! `intl.js` reads and checks the options; a [`DateFormat`] is what they
//! resolve to, built once per set of options and reused.

use std::cell::RefCell;
use std::collections::HashMap;

use super::zones::{self, Zone, ZoneStyle};

/// A locale's data. Names are [wide, abbreviated, narrow]; the `_alone`
/// names are used when a month or weekday stands by itself, where they
/// differ. `date`, `time` and `glue` run from full to short; `formats`
/// maps skeletons to patterns (CLDR's availableFormats), the locale's own
/// first and then its parent's.
pub(super) struct Locale {
    /// The hour letter of the locale's own hour cycle.
    hour: u8,
    months: [[&'static str; 12]; 3],
    months_alone: Option<[[&'static str; 12]; 3]>,
    days: [[&'static str; 7]; 3],
    days_alone: Option<[[&'static str; 7]; 3]>,
    ampm: [&'static str; 2],
    eras: [[&'static str; 2]; 3],
    /// Day periods as (first hour, names), each running to the next, and
    /// the names of noon, which is the hour of 12 until the minutes or
    /// seconds shown say otherwise.
    periods: &'static [(u32, [&'static str; 3])],
    noon: Option<[&'static str; 3]>,
    date: [&'static str; 4],
    time: [&'static str; 4],
    glue: [&'static str; 4],
    formats: &'static [&'static str],
    /// How a field with no pattern of its own is added, by field number,
    /// and what the field is called.
    append: &'static [(i32, &'static str)],
    names: &'static [(i32, &'static str)],
    decimal: Option<&'static str>,
    zones: ZoneStyle,
}

const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const EN_MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const GB_MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec",
];
const EN_MONTHS_NARROW: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
const EN_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const EN_DAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const EN_DAYS_NARROW: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
const DE_MONTHS: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const DE_DAYS: [&str; 7] = [
    "Sonntag",
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
];

const EN_FORMATS: &str = "Bh=h B|Bhm=h:mm B|Bhms=h:mm:ss B|d=d|E=ccc|EBhm=E h:mm B|EBhms=E h:mm:ss B|Ed=d E|Eh=E h a|Ehm=E h:mm a|\
EHm=E HH:mm|Ehms=E h:mm:ss a|EHms=E HH:mm:ss|Gy=y G|GyMd=M/d/y G|GyMMM=MMM y G|GyMMMd=MMM d, y G|GyMMMEd=E, MMM d, y G|\
h=h a|H=HH|hm=h:mm a|Hm=HH:mm|hms=h:mm:ss a|Hms=HH:mm:ss|hmsv=h:mm:ss a v|Hmsv=HH:mm:ss v|hmv=h:mm a v|Hmv=HH:mm v|\
M=L|Md=M/d|MEd=E, M/d|MMM=LLL|MMMd=MMM d|MMMEd=E, MMM d|MMMMd=MMMM d|ms=mm:ss|y=y|yM=M/y|yMd=M/d/y|yMEd=E, M/d/y|\
yMMM=MMM y|yMMMd=MMM d, y|yMMMEd=E, MMM d, y|yMMMM=MMMM y";

const EN_PERIODS: &[(u32, [&str; 3])] = &[
    (0, ["in the morning"; 3]),
    (12, ["in the afternoon"; 3]),
    (18, ["in the evening"; 3]),
    (21, ["at night"; 3]),
];
const EN_APPEND: &[(i32, &str)] = &[
    (0, "{0} {1}"),
    (1, "{0} {1}"),
    (6, "{0} {1}"),
    (15, "{0} {1}"),
];
const EN_NAMES: &[(i32, &str)] = &[
    (3, "month"),
    (9, "day"),
    (10, "AM/PM"),
    (11, "hour"),
    (12, "minute"),
    (13, "second"),
];
const EN_UTC: &str = "Coordinated Universal Time";

/// The locales with tables, numbered as `intl.js` numbers them.
pub(super) static LOCALES: [Locale; 4] = [
    // en-US
    Locale {
        hour: b'h',
        months: [EN_MONTHS, EN_MONTHS_SHORT, EN_MONTHS_NARROW],
        months_alone: None,
        days: [EN_DAYS, EN_DAYS_SHORT, EN_DAYS_NARROW],
        days_alone: None,
        ampm: ["AM", "PM"],
        eras: [["Before Christ", "Anno Domini"], ["BC", "AD"], ["B", "A"]],
        periods: EN_PERIODS,
        noon: Some(["noon", "noon", "n"]),
        date: ["EEEE, MMMM d, y", "MMMM d, y", "MMM d, y", "M/d/yy"],
        time: ["h:mm:ss a zzzz", "h:mm:ss a z", "h:mm:ss a", "h:mm a"],
        glue: ["{1} 'at' {0}", "{1} 'at' {0}", "{1}, {0}", "{1}, {0}"],
        formats: &[EN_FORMATS],
        append: EN_APPEND,
        names: EN_NAMES,
        decimal: None,
        zones: ZoneStyle {
            language: 0,
            shorts: "e",
            region: "US",
            spelling: &[],
            utc: EN_UTC,
        },
    },
    // en-CA
    Locale {
        hour: b'h',
        months: [EN_MONTHS, EN_MONTHS_SHORT, EN_MONTHS_NARROW],
        months_alone: None,
        days: [EN_DAYS, EN_DAYS_SHORT, EN_DAYS_NARROW],
        days_alone: None,
        ampm: ["a.m.", "p.m."],
        eras: [["Before Christ", "Anno Domini"], ["BC", "AD"], ["B", "A"]],
        periods: &[
            (0, ["in the morning", "in the morning", "mor"]),
            (12, ["in the afternoon", "in the afternoon", "aft"]),
            (18, ["in the evening", "in the evening", "eve"]),
            (21, ["at night", "at night", "night"]),
        ],
        noon: Some(["noon", "noon", "n"]),
        date: ["EEEE, MMMM d, y", "MMMM d, y", "MMM d, y", "y-MM-dd"],
        time: ["h:mm:ss a zzzz", "h:mm:ss a z", "h:mm:ss a", "h:mm a"],
        glue: ["{1} 'at' {0}", "{1} 'at' {0}", "{1}, {0}", "{1}, {0}"],
        formats: &[
            "Ed=E d|Md=MM-dd|MEd=E, MM-dd|MMdd=MM-dd|yM=y-MM|yMd=y-MM-dd|yMEd=E, y-MM-dd",
            EN_FORMATS,
        ],
        append: EN_APPEND,
        names: EN_NAMES,
        decimal: None,
        zones: ZoneStyle {
            language: 0,
            shorts: "ec",
            region: "CA",
            spelling: &[
                ("St. Pierre & Miquelon", "Saint-Pierre-et-Miquelon"),
                (" & ", " and "),
            ],
            utc: EN_UTC,
        },
    },
    // en-GB
    Locale {
        hour: b'H',
        months: [EN_MONTHS, GB_MONTHS_SHORT, EN_MONTHS_NARROW],
        months_alone: None,
        days: [EN_DAYS, EN_DAYS_SHORT, EN_DAYS_NARROW],
        days_alone: None,
        ampm: ["am", "pm"],
        eras: [["Before Christ", "Anno Domini"], ["BC", "AD"], ["B", "A"]],
        periods: EN_PERIODS,
        noon: Some(["noon", "noon", "n"]),
        date: ["EEEE, d MMMM y", "d MMMM y", "d MMM y", "dd/MM/y"],
        time: ["HH:mm:ss zzzz", "HH:mm:ss z", "HH:mm:ss", "HH:mm"],
        glue: ["{1} 'at' {0}", "{1} 'at' {0}", "{1}, {0}", "{1}, {0}"],
        formats: &[
            "Ed=E d|GyMd=dd/MM/y G|GyMMMd=d MMM y G|GyMMMEd=E, d MMM y G|Md=dd/MM|MEd=E dd/MM|MMdd=dd/MM|\
MMMd=d MMM|MMMEd=E d MMM|MMMMd=d MMMM|yM=MM/y|yMd=dd/MM/y|yMEd=E, dd/MM/y|yMMMd=d MMM y|yMMMEd=E, d MMM y",
            EN_FORMATS,
        ],
        append: EN_APPEND,
        names: EN_NAMES,
        decimal: None,
        zones: ZoneStyle {
            language: 0,
            shorts: "g",
            region: "GB",
            spelling: &[("St. ", "St ")],
            utc: EN_UTC,
        },
    },
    // de
    Locale {
        hour: b'H',
        months: [
            DE_MONTHS,
            [
                "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.",
                "Nov.", "Dez.",
            ],
            ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"],
        ],
        months_alone: Some([
            DE_MONTHS,
            [
                "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
            ],
            ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"],
        ]),
        days: [
            DE_DAYS,
            ["So.", "Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa."],
            ["S", "M", "D", "M", "D", "F", "S"],
        ],
        days_alone: Some([
            DE_DAYS,
            ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
            ["S", "M", "D", "M", "D", "F", "S"],
        ]),
        ampm: ["AM", "PM"],
        eras: [
            ["v. Chr.", "n. Chr."],
            ["v. Chr.", "n. Chr."],
            ["v. Chr.", "n. Chr."],
        ],
        periods: &[
            (0, ["nachts"; 3]),
            (5, ["morgens"; 3]),
            (10, ["vormittags", "vorm.", "vorm."]),
            (12, ["mittags"; 3]),
            (13, ["nachmittags", "nachm.", "nachm."]),
            (18, ["abends"; 3]),
        ],
        noon: None,
        date: ["EEEE, d. MMMM y", "d. MMMM y", "dd.MM.y", "dd.MM.yy"],
        time: ["HH:mm:ss zzzz", "HH:mm:ss z", "HH:mm:ss", "HH:mm"],
        glue: ["{1} 'um' {0}", "{1} 'um' {0}", "{1}, {0}", "{1}, {0}"],
        formats: &[
            "Bh=h B|Bhm=h:mm B|Bhms=h:mm:ss B|d=d|E=ccc|EBhm=E h:mm B|EBhms=E h:mm:ss B|Ed=E, d.|Eh=E, h a|Ehm=E h:mm a|EHm=E, HH:mm|\
Ehms=E, h:mm:ss a|EHms=E, HH:mm:ss|Gy=y G|GyMd=dd.MM.y G|GyMMM=MMM y G|GyMMMd=d. MMM y G|GyMMMEd=E, d. MMM y G|\
h=h a|H=HH 'Uhr'|hm=h:mm a|Hm=HH:mm|hms=h:mm:ss a|Hms=HH:mm:ss|hmsv=h:mm:ss a v|Hmsv=HH:mm:ss v|hmv=h:mm a v|Hmv=HH:mm v|\
M=L|Md=d.M.|MEd=E, d.M.|MMd=dd.MM.|MMdd=dd.MM.|MMM=LLL|MMMd=d. MMM|MMMEd=E, d. MMM|MMMMd=d. MMMM|MMMMEd=E, d. MMMM|\
ms=mm:ss|y=y|yM=M/y|yMd=d.M.y|yMEd=E, d.M.y|yMM=MM/y|yMMdd=dd.MM.y|yMMM=MMM y|yMMMd=d. MMM y|yMMMEd=E, d. MMM y|yMMMM=MMMM y",
        ],
        append: &[
            (0, "{1} {0}"),
            (1, "{1} {0}"),
            (6, "{0} {1}"),
            (15, "{0} {1}"),
        ],
        names: &[
            (3, "Monat"),
            (9, "Tag"),
            (10, "Tageshälfte"),
            (11, "Stunde"),
            (12, "Minute"),
            (13, "Sekunde"),
        ],
        decimal: Some(","),
        zones: ZoneStyle {
            language: 1,
            shorts: "d",
            region: "DE",
            spelling: &[],
            utc: "Koordinierte Weltzeit",
        },
    },
];

// Patterns.

/// Pattern fields, numbered as ICU numbers them: era 0, year 1, month 3,
/// weekday 6, day 9, day period 10, hour 11, minute 12, second 13,
/// fractional second 14, zone 15. Fields 0-9 are the date.
fn field(c: u8) -> Option<usize> {
    Some(match c {
        b'G' => 0,
        b'y' => 1,
        b'M' | b'L' => 3,
        b'E' | b'c' => 6,
        b'd' => 9,
        b'a' | b'B' => 10,
        b'h' | b'K' | b'H' | b'k' => 11,
        b'm' => 12,
        b's' => 13,
        b'S' => 14,
        b'z' | b'O' | b'v' => 15,
        _ => return None,
    })
}

/// The part type a field's value is reported as.
fn part_type(c: u8) -> &'static str {
    match c {
        b'G' => "era",
        b'y' => "year",
        b'M' | b'L' => "month",
        b'E' | b'c' => "weekday",
        b'd' => "day",
        b'a' | b'B' => "dayPeriod",
        b'h' | b'K' | b'H' | b'k' => "hour",
        b'm' => "minute",
        b's' => "second",
        b'S' => "fractionalSecond",
        _ => "timeZoneName",
    }
}

/// The hour letter of an hour cycle.
pub(super) fn hour_char(hc: &str) -> u8 {
    match hc {
        "h11" => b'K',
        "h12" => b'h',
        "h24" => b'k',
        _ => b'H',
    }
}

/// A field's type: positive and growing with its width when numeric,
/// negative when text. Patterns are compared by these. How far a letter's
/// type sits from its field's first letter are ICU's field types, in
/// steps of 16.
fn field_type(c: u8, n: usize) -> i32 {
    if b"GEaBzOv".contains(&c) || (n > 2 && b"MLc".contains(&c)) {
        let shift = match c {
            b'L' | b'O' => 1,
            b'c' | b'v' => 2,
            b'B' => 3,
            _ => 0,
        };
        let base = match n {
            4 => -0x104,
            n if n > 4 => -0x101,
            _ => -0x103,
        };
        return base - 16 * shift;
    }
    let shift = match c {
        b'L' | b'K' => 1,
        b'c' => 2,
        b'H' => 10,
        b'k' => 11,
        _ => 0,
    };
    0x100 + 16 * shift + n as i32
}

/// A pattern token: literal text, or a field letter and its width.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Text(String),
    Field(u8, usize),
}

impl Token {
    /// The token's first character, which is what some checks look at
    /// whether it is a field or text.
    fn first(&self) -> Option<u8> {
        match self {
            Token::Text(s) => s.bytes().next(),
            Token::Field(c, _) => Some(*c),
        }
    }
}

/// A pattern as literal strings and fields: `'[^']*'|([A-Za-z])\1*|[^A-Za-z']+`.
fn tokens(pattern: &str) -> Vec<Token> {
    let b = pattern.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\'' {
            match b[i + 1..].iter().position(|&x| x == b'\'') {
                Some(len) => {
                    out.push(Token::Text(pattern[i + 1..i + 1 + len].to_owned()));
                    i += len + 2;
                }
                None => i += 1,
            }
        } else if c.is_ascii_alphabetic() {
            let n = b[i..].iter().take_while(|&&x| x == c).count();
            out.push(Token::Field(c, n));
            i += n;
        } else {
            let n = b[i..]
                .iter()
                .take_while(|&&x| !x.is_ascii_alphabetic() && x != b'\'')
                .count();
            out.push(Token::Text(pattern[i..i + n].to_owned()));
            i += n;
        }
    }
    out
}

/// What a skeleton or pattern asks for: per field its type, letter and
/// width (0 where unset), and a mask of the fields present.
#[derive(Clone)]
struct Matcher {
    ty: [i32; 16],
    chr: [u8; 16],
    len: [usize; 16],
    mask: i32,
}

impl Matcher {
    /// As in ICU, a 12-hour hour brings AM/PM with it and a 24-hour one
    /// drops any day period.
    fn new(pattern: &str) -> Self {
        let mut m = Matcher {
            ty: [0; 16],
            chr: [0; 16],
            len: [0; 16],
            mask: 0,
        };
        let b = pattern.as_bytes();
        let mut quoted = false;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            let mut j = i + 1;
            if c == b'\'' {
                quoted = !quoted;
            }
            if !quoted && field(c).is_some() {
                while b.get(j) == Some(&c) {
                    j += 1;
                }
                m.set(c, j - i);
            }
            i = j;
        }
        if m.ty[11] != 0 {
            if m.chr[11] == b'h' || m.chr[11] == b'K' {
                if m.ty[10] == 0 {
                    m.set(b'a', 1);
                }
            } else {
                m.ty[10] = 0;
                m.mask &= !0x400;
            }
        }
        if m.ty[14] != 0 && m.ty[13] == 0 {
            m.set(b's', 1);
        }
        m
    }

    fn set(&mut self, c: u8, n: usize) {
        let Some(f) = field(c) else { return };
        self.ty[f] = field_type(c, n);
        self.chr[f] = c;
        self.len[f] = n;
        self.mask |= 1 << f;
    }
}

/// A locale's pattern, with what it asks for and whether its skeleton was
/// written down rather than read off the pattern.
struct Pattern {
    m: Matcher,
    pattern: String,
    given: bool,
}

/// A locale's patterns and lookups, built when first needed.
#[derive(Default)]
struct Cache {
    patterns: Option<Vec<Pattern>>,
    keys: Option<HashMap<&'static str, &'static str>>,
    best: HashMap<String, String>,
}

thread_local! {
    static CACHES: RefCell<[Cache; 4]> = RefCell::default();
}

/// A locale's patterns, in the order ICU collects them: each field by
/// itself, the date and time styles, then `formats`, the locale's own
/// before its parent's and each sorted by skeleton.
fn patterns_of(data: &Locale) -> Vec<Pattern> {
    let mut list: Vec<Pattern> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut add = |skeleton: &str, pattern: &str, given: bool| {
        let m = Matcher::new(skeleton);
        let mut key = String::new();
        for i in 0..16 {
            if m.ty[i] != 0 {
                key.extend(std::iter::repeat_n(m.chr[i] as char, m.len[i]));
            }
        }
        let entry = Pattern {
            m,
            pattern: pattern.to_owned(),
            given,
        };
        match index.get(&key) {
            Some(&i) if given => list[i] = entry,
            Some(_) => {}
            None => {
                index.insert(key, list.len());
                list.push(entry);
            }
        }
    };
    for c in ["G", "y", "M", "E", "d", "a", "H", "m", "s", "S", "v"] {
        add(c, c, false);
    }
    for i in 0..4 {
        add(data.date[i], data.date[i], false);
        add(data.time[i], data.time[i], false);
    }
    let mut seen = std::collections::HashSet::new();
    for level in data.formats {
        let mut entries: Vec<&str> = level.split('|').collect();
        entries.sort_unstable();
        for entry in entries {
            let (skeleton, pattern) = entry.split_once('=').unwrap_or((entry, ""));
            if seen.insert(skeleton) {
                add(skeleton, pattern, true);
            }
        }
    }
    list
}

/// The pattern closest to the fields of `want` that are in `mask`, with
/// the fields it lacks and those it has that are not wanted. A missing
/// field costs more than any difference in width, and an unwanted one more
/// than that. Of two equally close, the one lacking the later field wins.
fn closest<'a>(patterns: &'a [Pattern], want: &Matcher, mask: i32) -> (&'a Pattern, i32, i32) {
    let mut best = &patterns[0];
    let (mut best_distance, mut best_missing, mut best_extra) = (i64::MAX, 0, 0);
    for entry in patterns {
        let (mut distance, mut missing, mut extra) = (0i64, 0i32, 0i32);
        for i in 0..16 {
            let mine = if mask >> i & 1 != 0 { want.ty[i] } else { 0 };
            let other = entry.m.ty[i];
            if mine == other {
                continue;
            }
            if mine == 0 {
                distance += 0x10000;
                extra |= 1 << i;
            } else if other == 0 {
                distance += 0x1000;
                missing |= 1 << i;
            } else {
                distance += i64::from((mine - other).abs());
            }
        }
        if distance < best_distance || (distance == best_distance && missing > best_missing) {
            best = entry;
            best_distance = distance;
            best_missing = missing;
            best_extra = extra;
            if distance == 0 {
                break;
            }
        }
    }
    (best, best_missing, best_extra)
}

/// `pattern` with its fields widened or narrowed to what `want` asks.
/// Minutes and seconds keep the pattern's width, and so does a field whose
/// width in the pattern's own skeleton is the one asked for: that is how
/// `y-MM-dd` survives a request for a numeric month. With `decimal`,
/// fractional seconds are added after the seconds.
fn adjust(entry: &Pattern, pattern: &str, want: &Matcher, decimal: Option<&str>) -> String {
    let mut out = String::new();
    for t in tokens(pattern) {
        let (mut c, mut n) = match t {
            Token::Text(t) => {
                if t.bytes().any(|b| b.is_ascii_alphabetic()) {
                    out.push('\'');
                    out.push_str(&t);
                    out.push('\'');
                } else {
                    out.push_str(&t);
                }
                continue;
            }
            Token::Field(c, n) => (c, n),
        };
        let f = field(c);
        if let (Some(decimal), Some(13)) = (decimal, f) {
            push_repeat(&mut out, c, n);
            out.push_str(decimal);
            push_repeat(&mut out, b'S', want.len[14]);
            continue;
        }
        if let Some(f) = f
            && want.ty[f] != 0
        {
            let mut asked = want.len[f];
            if want.chr[f] == b'E' && asked < 3 {
                asked = 3;
            }
            let own = entry.given
                && (entry.m.len[f] == asked || (field_type(c, n) > 0) != (entry.m.ty[f] > 0));
            if f != 12 && f != 13 && !own {
                n = asked;
            }
            if f != 11 && f != 3 && f != 6 && f != 1 {
                c = want.chr[f];
            }
        }
        push_repeat(&mut out, c, n);
    }
    out
}

fn push_repeat(out: &mut String, c: u8, n: usize) {
    out.extend(std::iter::repeat_n(c as char, n));
}

/// `template` with `{0}`, `{1}` and `{2}` replaced, each once.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = template.to_owned();
    for (i, arg) in args.iter().enumerate() {
        out = out.replacen(&format!("{{{i}}}"), arg, 1);
    }
    out
}

/// The pattern for the fields in `mask`: the closest one, with each field
/// it lacks appended the way the locale appends that field.
fn appending(data: &Locale, patterns: &[Pattern], want: &Matcher, mask: i32) -> String {
    let (mut entry, mut missing, _) = closest(patterns, want, mask);
    let mut out = adjust(entry, &entry.pattern, want, None);
    let mut last = 0;
    while missing != 0 && missing != last {
        if (missing & 0x6000) == 0x4000 && (mask & 0x6000) == 0x6000 {
            out = adjust(entry, &out, want, Some(data.decimal.unwrap_or(".")));
            missing &= !0x4000;
            continue;
        }
        let asked = missing;
        (entry, missing, _) = closest(patterns, want, asked);
        let top = 31 - (asked & !missing).leading_zeros() as i32;
        let template = data
            .append
            .iter()
            .find(|a| a.0 == top)
            .map_or("{0} ({2}: {1})", |a| a.1);
        let name = data
            .names
            .iter()
            .find(|a| a.0 == top)
            .map_or("undefined", |a| a.1);
        out = fill(
            template,
            &[
                &out,
                &adjust(entry, &entry.pattern, want, None),
                &format!("'{name}'"),
            ],
        );
        last = missing;
    }
    out
}

/// A date and a time pattern glued, the glue chosen by the width of the
/// month and whether there is a weekday.
fn glue(data: &Locale, month: usize, weekday: bool, time: &str, date: &str) -> String {
    let i = match month {
        4 if weekday => 0,
        4 => 1,
        3 => 2,
        _ => 3,
    };
    fill(data.glue[i], &[time, date])
}

/// ICU's getBestPattern: the locale's pattern for a skeleton. A date and a
/// time with no pattern in common are found apart and glued, the glue
/// chosen by the width of the month. A skeleton the locale has a pattern
/// for, or a date and a time it has patterns for, need no search.
fn best_pattern(locale: usize, skeleton: &str) -> String {
    CACHES.with(|caches| {
        let mut caches = caches.borrow_mut();
        let cache = &mut caches[locale];
        if let Some(pattern) = cache.best.get(skeleton) {
            return pattern.clone();
        }
        let data = &LOCALES[locale];
        let keys = cache.keys.get_or_insert_with(|| {
            let mut keys = HashMap::new();
            for level in data.formats {
                for entry in level.split('|') {
                    let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
                    keys.entry(key).or_insert(value);
                }
            }
            keys
        });
        let mut pattern = keys.get(skeleton).map(|p| (*p).to_owned());
        let split = skeleton.find(|c| "BhHKkmsSzOv".contains(c));
        if pattern.is_none()
            && let Some(split) = split.filter(|&s| s > 0)
            && let (Some(date), Some(time)) =
                (keys.get(&skeleton[..split]), keys.get(&skeleton[split..]))
        {
            let month = skeleton
                .bytes()
                .skip_while(|&b| b != b'M')
                .take_while(|&b| b == b'M')
                .count();
            pattern = Some(glue(data, month, skeleton.contains('E'), time, date));
        }
        let pattern = pattern.unwrap_or_else(|| {
            let patterns = cache.patterns.get_or_insert_with(|| patterns_of(data));
            let want = Matcher::new(skeleton);
            let (entry, missing, extra) = closest(patterns, &want, -1);
            if missing == 0 && extra == 0 {
                return adjust(entry, &entry.pattern, &want, None);
            }
            let date = if want.mask & 0x3ff != 0 {
                appending(data, patterns, &want, want.mask & 0x3ff)
            } else {
                String::new()
            };
            let time = if want.mask & 0xfc00 != 0 {
                appending(data, patterns, &want, want.mask & 0xfc00)
            } else {
                String::new()
            };
            if date.is_empty() || time.is_empty() {
                date + &time
            } else {
                let month = if want.ty[3] != 0 { want.len[3] } else { 0 };
                glue(data, month, want.ty[6] != 0, &time, &date)
            }
        });
        cache.best.insert(skeleton.to_owned(), pattern.clone());
        pattern
    })
}

// Formatters.

/// What a set of DateTimeFormat options resolves to.
pub(super) struct DateFormat {
    locale: usize,
    zone: Zone,
    tokens: Vec<Token>,
    /// Whether the pattern shows the hour.
    pub(super) hour: bool,
}

/// The fields of a time, as `new Date(t)`'s getUTC* methods give them:
/// `None` where they are NaN, for times out of `Date`'s range.
struct Fields {
    year: i64,
    month: usize,
    day: i64,
    weekday: usize,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
}

fn fields_of(t: f64) -> Option<Fields> {
    let t = zones::time_clip(t);
    if t.is_nan() {
        return None;
    }
    let t = t as i64;
    let days = t.div_euclid(86_400_000);
    let ms = t.rem_euclid(86_400_000);
    let (year, month, day) = zones::civil_from_days(days);
    Some(Fields {
        year,
        month: month as usize,
        day,
        weekday: (days + 4).rem_euclid(7) as usize,
        hour: ms / 3_600_000,
        minute: ms / 60_000 % 60,
        second: ms / 1000 % 60,
        ms: ms % 1000,
    })
}

/// `String(n).padStart(k, '0')`, NaN for `None`, written over `buf`.
fn pad(buf: &mut String, n: Option<i64>, k: usize) {
    use std::fmt::Write;
    buf.clear();
    match n {
        Some(n) => {
            let _ = write!(buf, "{n}");
        }
        None => buf.push_str("NaN"),
    }
    if buf.len() < k {
        buf.insert_str(0, &"0".repeat(k - buf.len()));
    }
}

/// A formatted part: its type and its value, which is undefined where JS
/// looked up a name for a NaN field.
pub(super) type Part = (&'static str, Option<String>);

/// A part's value that could not be found: a day period of an invalid
/// time, which threw a TypeError.
pub(super) struct NoDayPeriod;

impl DateFormat {
    /// Resolves the pattern for a skeleton, or for date and time styles
    /// (0 full to 3 short), in hour cycle `hc`.
    pub(super) fn new(
        locale: usize,
        zone: Zone,
        hc: &str,
        date_style: Option<usize>,
        time_style: Option<usize>,
        skeleton: Option<&str>,
    ) -> Self {
        let data = &LOCALES[locale];
        let hour = hour_char(hc);
        let pattern = if date_style.is_some() || time_style.is_some() {
            let date = date_style.map(|i| data.date[i]);
            let time = time_style.map(|i| data.time[i]);
            let mut pattern = match (date, time) {
                (Some(d), Some(t)) => fill(data.glue[date_style.unwrap_or(0)], &[t, d]),
                (Some(d), None) => d.to_owned(),
                (None, Some(t)) => t.to_owned(),
                (None, None) => String::new(),
            };
            // A style with another hour cycle is found again by its
            // skeleton.
            if time.is_some() && hour != data.hour {
                let mut skeleton = String::new();
                for t in tokens(&pattern) {
                    if let Token::Field(c, n) = t {
                        push_repeat(
                            &mut skeleton,
                            if field(c) == Some(11) { hour } else { c },
                            n,
                        );
                    }
                }
                pattern = best_pattern(locale, &skeleton);
            }
            pattern
        } else {
            best_pattern(locale, skeleton.unwrap_or(""))
        };
        // The pattern's tokens, its hours in the hour cycle asked for.
        let mut list = tokens(&pattern);
        let mut has_hour = false;
        for t in &mut list {
            if let Token::Field(c, _) = t
                && field(*c) == Some(11)
            {
                *c = hour;
                has_hour = true;
            }
        }
        DateFormat {
            locale,
            zone,
            tokens: list,
            hour: has_hour,
        }
    }

    /// Gives each part of time `t` (ms, in range and integral) to `out`,
    /// as (type, value). Literal text comes as it is in the pattern, in
    /// one or more pieces.
    pub(super) fn emit(
        &self,
        t: f64,
        out: &mut dyn FnMut(&'static str, Option<&str>),
    ) -> Result<(), NoDayPeriod> {
        let data = &LOCALES[self.locale];
        let off = zones::offset(self.zone, t);
        let fields = fields_of(t + off * 1000.0);
        let f = fields.as_ref();
        let year = f.map(|f| f.year);
        let hour = f.map(|f| f.hour);
        let mut buf = String::new();
        for token in &self.tokens {
            let (c, n) = match token {
                Token::Text(text) => {
                    out("literal", Some(text));
                    continue;
                }
                &Token::Field(c, n) => (c, n),
            };
            let width = match n {
                4 => 0,
                n if n > 4 => 2,
                _ => 1,
            };
            let ty = part_type(c);
            let name: Option<&str> = match c {
                b'G' => Some(data.eras[width][usize::from(year.is_some_and(|y| y > 0))]),
                b'M' | b'L' if n >= 3 => {
                    let names = match (c, data.months_alone) {
                        (b'L', Some(alone)) => alone,
                        _ => data.months,
                    };
                    f.map(|f| names[width][f.month])
                }
                b'E' | b'c' => {
                    let names = match (c, data.days_alone) {
                        (b'c', Some(alone)) => alone,
                        _ => data.days,
                    };
                    f.map(|f| names[width][f.weekday])
                }
                b'a' => Some(data.ampm[usize::from(hour.is_some_and(|h| h >= 12))]),
                b'B' => {
                    let Some(f) = f else { return Err(NoDayPeriod) };
                    let shows = |letter: u8| self.tokens.iter().any(|t| t.first() == Some(letter));
                    let noon = match data.noon {
                        Some(noon)
                            if f.hour == 12
                                && !(shows(b'm')
                                    && (f.minute != 0 || (shows(b's') && f.second != 0))) =>
                        {
                            Some(noon)
                        }
                        _ => None,
                    };
                    let names = noon.unwrap_or_else(|| {
                        data.periods
                            .iter()
                            .rev()
                            .find(|p| i64::from(p.0) <= f.hour)
                            .map_or(["", "", ""], |p| p.1)
                    });
                    Some(names[width])
                }
                _ => {
                    match c {
                        b'y' => pad(
                            &mut buf,
                            year.map(|y| {
                                let y = if y > 0 { y } else { 1 - y };
                                if n == 2 { y % 100 } else { y }
                            }),
                            n,
                        ),
                        b'M' | b'L' => pad(&mut buf, f.map(|f| f.month as i64 + 1), n),
                        b'd' => pad(&mut buf, f.map(|f| f.day), n),
                        b'h' => pad(
                            &mut buf,
                            Some(hour.map_or(12, |h| if h % 12 == 0 { 12 } else { h % 12 })),
                            n,
                        ),
                        b'K' => pad(&mut buf, hour.map(|h| h % 12), n),
                        b'H' => pad(&mut buf, hour, n),
                        b'k' => pad(
                            &mut buf,
                            Some(hour.map_or(24, |h| if h == 0 { 24 } else { h })),
                            n,
                        ),
                        b'm' => pad(&mut buf, f.map(|f| f.minute), n),
                        b's' => pad(&mut buf, f.map(|f| f.second), n),
                        b'S' => {
                            pad(&mut buf, f.map(|f| f.ms), 3);
                            buf.truncate(n.min(buf.len()));
                        }
                        _ => buf = zones::display_name(&data.zones, self.zone, c, n, t, off),
                    }
                    Some(&buf)
                }
            };
            out(ty, name);
        }
        Ok(())
    }

    /// The parts of time `t`, adjacent literal text joined.
    pub(super) fn parts(&self, t: f64) -> Result<Vec<Part>, NoDayPeriod> {
        let mut parts: Vec<Part> = Vec::with_capacity(self.tokens.len());
        self.emit(t, &mut |ty, value| match (ty, parts.last_mut()) {
            ("literal", Some(("literal", Some(last)))) => last.push_str(value.unwrap_or_default()),
            _ => parts.push((ty, value.map(str::to_owned))),
        })?;
        Ok(parts)
    }

    /// Time `t` formatted.
    pub(super) fn format(&self, t: f64) -> Result<String, NoDayPeriod> {
        let mut s = String::new();
        self.emit(t, &mut |_, value| s.push_str(value.unwrap_or_default()))?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(locale: usize, skeleton: &str) -> String {
        best_pattern(locale, skeleton)
    }

    #[test]
    fn best_patterns() {
        assert_eq!(pattern(0, "yMd"), "M/d/y");
        assert_eq!(pattern(1, "yMMdd"), "y-MM-dd");
        assert_eq!(pattern(2, "yMMMMEEEEd"), "EEEE, d MMMM y");
        assert_eq!(pattern(3, "Hm"), "HH:mm");
        assert_eq!(pattern(0, "yMdhms"), "M/d/y, h:mm:ss a");
    }

    #[test]
    fn tokenizes() {
        assert_eq!(
            tokens("d 'de' MMM, y''x"),
            [
                Token::Field(b'd', 1),
                Token::Text(" ".into()),
                Token::Text("de".into()),
                Token::Text(" ".into()),
                Token::Field(b'M', 3),
                Token::Text(", ".into()),
                Token::Field(b'y', 1),
                Token::Text(String::new()),
                Token::Field(b'x', 1),
            ]
        );
    }
}
