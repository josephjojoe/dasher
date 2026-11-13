//! Time zones: every IANA zone's rules from 1970 on, the host's zone, and
//! zone names, unpacked from the tables in `tzdata.rs`.
//!
//! Times here are `f64`, as in JS: seconds or milliseconds since the
//! epoch, and offsets in seconds. Several results are NaN where the JS
//! this replaced got NaN, and the formatting code relies on that.
//!
//! # The tables
//!
//! Numbers are base 36, and times of day and offsets count half minutes.
//!
//! - `SLOTS`: one transition a year, as month, day (two digits: a day of
//!   the month, 40 + weekday for the last such weekday, or 50 + 31 *
//!   weekday + day - 1 for the first such weekday on or after a day), time
//!   of day on the clock before it, and the change in offset.
//! - `ZONES`: `names:offset at the start of 1970:eras` per set of zones
//!   with the same rules, a leading digit in a name standing for a region
//!   (`REGIONS`). An era is the year it starts, less 1970, in two digits,
//!   then the two-digit numbers of its slots.
//! - `LINKS`: other names for zones, as `name=number of the zone`,
//!   counting the names in `ZONES`.
//! - `METAZONES`: `English~German~zones~short names~base` per metazone.
//!   Long names are `generic;standard;daylight`, `=name` for a standard
//!   name alone, or a stem and `*` for the usual three (`ZONE_WORDS`), `*S`
//!   being English summer rather than daylight time. The zones are the one
//!   whose clock the metazone's generic name means, then others for
//!   regions (`CA0Toronto`). Short names are `standard,daylight,generic`
//!   after a letter and a colon: e for English, c for Canada's alone, g
//!   for British English, d for German. A base is the number of the
//!   metazone a zone's own names (London's British Summer Time) stand in
//!   for.
//! - `ERAS`: the metazones a zone has been in, each as the metazone's
//!   number in two digits, the sign of its standard offset (`!` if
//!   unknown) and the offset, then `@` and the second it starts unless it
//!   is the first. Nothing before the `@` is a time in no metazone, and
//!   `--` for the number one in no metazone whose standard offset matters
//!   all the same.
//! - `ZONE_ERAS`: two digits per name in `ZONES`, the number of its entry
//!   in `ERAS`.
//!
//! The tables are unpacked once per process, on first use. A zone's
//! transitions are worked out a year at a time as far as times asked
//! about need, and kept per thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use super::tzdata::{ERAS, LINKS, METAZONES, SLOTS, ZONE_ERAS, ZONES};

const REGIONS: [&str; 10] = [
    "America",
    "Asia",
    "Europe",
    "Africa",
    "Pacific",
    "Australia",
    "Antarctica",
    "Atlantic",
    "Indian",
    "Etc",
];

/// The seconds in 400 Gregorian years, after which the calendar repeats.
const CYCLE: f64 = 12_622_780_800.0;
const HALF_YEAR: f64 = 184.0 * 86400.0;

/// `parseInt(s, 36)`: an optional sign, then base-36 digits up to the
/// first other character. NaN if there are none.
pub(super) fn parse36(s: &str) -> f64 {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut value = 0.0;
    let mut any = false;
    for c in digits.bytes() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'z' => c - b'a' + 10,
            b'A'..=b'Z' => c - b'A' + 10,
            _ => break,
        };
        value = value * 36.0 + f64::from(d);
        any = true;
    }
    match (any, neg) {
        (false, _) => f64::NAN,
        (true, true) => -value,
        (true, false) => value,
    }
}

/// `n.toString(36)` for a non-negative integer.
fn base36(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        let d = (n % 36) as u8;
        out.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The first two characters of a packed era, or all of it.
fn head(s: &str) -> &str {
    s.get(..2).unwrap_or(s)
}

/// A zone name as written in `ZONES`, its region spelt out.
fn expand(name: &str) -> String {
    match name.as_bytes().first() {
        Some(&b) if b.is_ascii_digit() => {
            format!("{}/{}", REGIONS[usize::from(b - b'0')], &name[1..])
        }
        _ => name.to_owned(),
    }
}

// Days and dates.

/// Days since the epoch of a proleptic Gregorian date (month 1 to 12).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since the epoch, as (year, month 0 to 11, day).
pub(super) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m - 1, d)
}

/// `Date.UTC(y, m, d) / 864e5` for integral arguments, the month allowed
/// to run past either end of the year and the day past the month's.
fn day_utc(y: f64, m: f64, d: f64) -> f64 {
    let year = y + (m / 12.0).floor();
    let month = m.rem_euclid(12.0);
    (days_from_civil(year as i64, month as i64 + 1, 1) as f64) + d - 1.0
}

/// A time in ms as `new Date(ms)` keeps it: NaN out of range, else
/// truncated.
pub(super) fn time_clip(ms: f64) -> f64 {
    if ms.is_finite() && ms.abs() <= 8.64e15 {
        ms.trunc() + 0.0
    } else {
        f64::NAN
    }
}

/// `new Date(ms).getUTCFullYear()`.
fn year_of_ms(ms: f64) -> f64 {
    let ms = time_clip(ms);
    if ms.is_nan() {
        return f64::NAN;
    }
    civil_from_days((ms / 864e5).floor() as i64).0 as f64
}

// The tables.

/// A transition rule: month, day code, seconds into the day on the clock
/// before it, and the change in offset in seconds.
#[derive(Clone, Copy)]
struct Slot {
    month: f64,
    day: f64,
    wall: f64,
    delta: f64,
}

/// One set of zones with the same rules.
struct Entry {
    /// The number of its first name, and how many it has.
    first: usize,
    count: usize,
    /// The packed offset at the start of 1970, as written.
    start: &'static str,
    /// Its eras, still packed.
    rules: Vec<&'static str>,
}

struct Tables {
    /// Every name in `ZONES`, in order, with its region spelt out.
    names: Vec<String>,
    /// The entry each name belongs to.
    entry_of: Vec<usize>,
    entries: Vec<Entry>,
    /// Lowercased names, and link names, to the number of the name they
    /// stand for.
    lookup: HashMap<String, usize>,
    slots: Vec<&'static str>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut names = Vec::new();
        let mut entry_of = Vec::new();
        let mut entries = Vec::new();
        for entry in ZONES.split('|') {
            let mut fields = entry.split(':');
            let list = fields.next().unwrap_or_default();
            let start = fields.next().unwrap_or_default();
            let rules = fields
                .next()
                .map_or_else(Vec::new, |eras| eras.split(',').collect());
            let first = names.len();
            for name in list.split(',') {
                names.push(expand(name));
                entry_of.push(entries.len());
            }
            entries.push(Entry {
                first,
                count: names.len() - first,
                start,
                rules,
            });
        }
        let mut lookup = HashMap::with_capacity(names.len() + 160);
        for (i, name) in names.iter().enumerate() {
            lookup.entry(name.to_ascii_lowercase()).or_insert(i);
        }
        for link in LINKS.split(' ') {
            if let Some((name, target)) = link.split_once('=') {
                let target = parse36(target);
                if target >= 0.0 && (target as usize) < names.len() {
                    lookup
                        .entry(expand(name).to_ascii_lowercase())
                        .or_insert(target as usize);
                }
            }
        }
        Tables {
            names,
            entry_of,
            entries,
            lookup,
            slots: SLOTS.split(' ').collect(),
        }
    })
}

/// Every zone name, in table order.
pub(super) fn zone_names() -> &'static [String] {
    &tables().names
}

/// Whether a name is one `supportedValuesOf('timeZone')` lists: named for
/// a place, so not an `Etc` zone or a bare abbreviation.
pub(super) fn is_place(name: &str) -> bool {
    let word = name
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    word > 0 && name.as_bytes().get(word) == Some(&b'/') && !name.starts_with("Etc")
}

/// The number of the zone called `name`, in any case. Links lead to the
/// name they stand for.
fn find_zone(name: &str) -> Option<usize> {
    // Lowercased as JS does, so the Kelvin sign is a k.
    let key = name.to_lowercase();
    if !valid_zone_key(&key) {
        return None;
    }
    tables().lookup.get(&key).copied()
}

/// `/^[a-z][\w+-]*(\/[\w+-]+)*$/` on a lowercased name.
fn valid_zone_key(key: &str) -> bool {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'+' || b == b'-';
    let bytes = key.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_lowercase) {
        return false;
    }
    let mut parts = key.split('/');
    parts.next().is_some_and(|p| p.bytes().all(word))
        && parts.all(|p| !p.is_empty() && p.bytes().all(word))
}

fn slot(i: f64) -> Slot {
    let s = tables().slots.get(i as usize).copied().unwrap_or("");
    let sign = s.find(['+', '-']).unwrap_or(s.len());
    Slot {
        month: parse36(s.get(..1).unwrap_or("")),
        day: parse36(s.get(1..3).unwrap_or("")),
        wall: parse36(s.get(3..sign).unwrap_or("")) * 30.0,
        delta: if s.as_bytes().get(sign) == Some(&b'-') {
            -30.0
        } else {
            30.0
        } * parse36(s.get(sign + 1..).unwrap_or("")),
    }
}

/// Midnight UTC, in days since the epoch, of the day a slot's day code
/// names in a month: a fixed day, the last given weekday, or the first
/// given weekday on or after a day. 1 January 1970 was a Thursday.
fn slot_day(code: f64, y: f64, m: f64) -> f64 {
    if code < 40.0 {
        return day_utc(y, m, code);
    }
    if code < 50.0 {
        let last = day_utc(y, m + 1.0, 0.0);
        return last - ((last + 4.0) % 7.0 - (code - 40.0) + 7.0) % 7.0;
    }
    let from = day_utc(y, m, (code - 50.0) % 31.0 + 1.0);
    from + (((code - 50.0) / 31.0).floor() - (from + 4.0) % 7.0 + 7.0) % 7.0
}

/// The Gregorian calendar repeats every 400 years, and so do the rules
/// once the last of them has begun: the seconds to take off a time `s` in
/// a far year to bring it into the table.
fn zone_shift(s: f64) -> f64 {
    let y = year_of_ms(s * 1000.0);
    if y < 2500.0 {
        0.0
    } else {
        ((y - 2100.0) / 400.0).floor() * CYCLE
    }
}

/// The transitions of one entry worked out so far: `at[i]` is when one
/// happens (seconds since the epoch) and `off[i]` the offset from then on.
struct Transitions {
    slots: Vec<Slot>,
    at: Vec<f64>,
    off: Vec<f64>,
    year: f64,
    era: usize,
    next: f64,
}

impl Transitions {
    fn open(entry: &Entry) -> Self {
        Transitions {
            slots: Vec::new(),
            at: vec![f64::NEG_INFINITY],
            off: vec![parse36(entry.start) * 30.0],
            year: 1970.0,
            era: 0,
            next: entry
                .rules
                .first()
                .map_or(f64::INFINITY, |r| 1970.0 + parse36(head(r))),
        }
    }
}

thread_local! {
    static TRANSITIONS: RefCell<HashMap<usize, Transitions>> = RefCell::default();
}

/// Runs `f` on the transitions of the zone named by number `zone`, worked
/// out up to the year after the one `s` (seconds, less its zone shift)
/// falls in.
fn with_transitions<R>(zone: usize, s: f64, f: impl FnOnce(&Transitions) -> R) -> R {
    let tables = tables();
    let entry_number = tables.entry_of[zone];
    let entry = &tables.entries[entry_number];
    TRANSITIONS.with(|cell| {
        let mut map = cell.borrow_mut();
        let z = map
            .entry(entry_number)
            .or_insert_with(|| Transitions::open(entry));
        let y = year_of_ms(s * 1000.0);
        // A transition belongs to the year on the wall clock before it,
        // which is within a day of the UTC year.
        while z.year <= y + 1.0 {
            while z.year >= z.next {
                let rule = entry.rules.get(z.era).copied().unwrap_or("");
                z.slots = rule
                    .as_bytes()
                    .get(2..)
                    .unwrap_or_default()
                    .chunks_exact(2)
                    .map(|pair| slot(parse36(std::str::from_utf8(pair).unwrap_or(""))))
                    .collect();
                z.era += 1;
                z.next = match entry.rules.get(z.era) {
                    Some(after) if !after.is_empty() => 1970.0 + parse36(head(after)),
                    _ => f64::INFINITY,
                };
            }
            for i in 0..z.slots.len() {
                let Slot {
                    month,
                    day,
                    wall,
                    delta,
                } = z.slots[i];
                let off = z.off[z.off.len() - 1];
                z.at.push(slot_day(day, z.year, month) * 86400.0 + wall - off);
                z.off.push(off + delta);
            }
            z.year += 1.0;
        }
        f(z)
    })
}

/// The number of the transition of zone `zone` in force at `s` seconds,
/// which is a time on the zone's own clock when `local`, and what
/// `f` makes of the transitions. A clock time that happens twice is the
/// later one, as in ICU.
fn zone_index<R>(zone: usize, s: f64, local: bool, f: impl FnOnce(&Transitions, usize) -> R) -> R {
    let s = s - zone_shift(s);
    with_transitions(zone, s, |z| {
        let (mut lo, mut hi) = (0, z.at.len() - 1);
        while lo < hi {
            let mid = (lo + hi + 1) >> 1;
            if z.at[mid] + if local { z.off[mid] } else { 0.0 } <= s {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        f(z, lo)
    })
}

/// The offset of zone `zone` at `t` (ms), in seconds. Before 1970 it is
/// the zone's offset at the start of 1970.
fn zone_offset(zone: usize, t: f64) -> f64 {
    zone_index(zone, (t / 1000.0).floor(), false, |z, i| z.off[i])
}

/// A time zone as a formatter keeps it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Zone {
    /// The host's own zone.
    Host,
    /// UTC, by any of its names.
    Utc,
    /// A fixed offset, in minutes.
    Fixed(i32),
    /// A zone with rules, by the number of its name.
    Rules(usize),
}

/// The zone a `timeZone` option names, or `None` if it is no zone. As in
/// V8, names match in any case and report their canonical form.
pub(super) fn time_zone(tz: &str) -> Option<Zone> {
    if is_utc_name(tz) {
        return Some(Zone::Utc);
    }
    if let Some(minutes) = parse_offset(tz) {
        return Some(Zone::Fixed(minutes));
    }
    find_zone(tz).map(Zone::Rules)
}

/// `/^(etc\/)?(utc|uct|gmt([+-]?0)?|zulu|universal|greenwich)$/i`.
fn is_utc_name(tz: &str) -> bool {
    let lower = tz.to_ascii_lowercase();
    let name = lower.strip_prefix("etc/").unwrap_or(&lower);
    matches!(
        name,
        "utc" | "uct" | "gmt" | "gmt0" | "gmt+0" | "gmt-0" | "zulu" | "universal" | "greenwich"
    )
}

/// `/^([+-])(\d\d)(?::?(\d\d))?$/` with hours below 24 and minutes below
/// 60, as minutes east of UTC.
fn parse_offset(tz: &str) -> Option<i32> {
    let b = tz.as_bytes();
    let sign = match b.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let two = |i: usize| -> Option<i32> {
        let (x, y) = (*b.get(i)?, *b.get(i + 1)?);
        (x.is_ascii_digit() && y.is_ascii_digit())
            .then(|| i32::from(x - b'0') * 10 + i32::from(y - b'0'))
    };
    let hours = two(1)?;
    let minutes = match b.len() {
        3 => 0,
        5 => two(3)?,
        6 if b[3] == b':' => two(4)?,
        _ => return None,
    };
    (hours < 24 && minutes < 60).then_some(sign * (hours * 60 + minutes))
}

/// The name a zone reports, as `resolvedOptions().timeZone` has it.
pub(super) fn zone_name(zone: Zone) -> String {
    match zone {
        Zone::Host => host().name.clone(),
        Zone::Utc => "UTC".to_owned(),
        Zone::Fixed(minutes) => offset_name(minutes),
        Zone::Rules(i) => tables().names[i].clone(),
    }
}

/// `+05:30` for an offset in minutes.
fn offset_name(off: i32) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", off.abs() / 60, off.abs() % 60)
}

/// The offset of a zone that is not the host's at `t` (ms), in seconds.
pub(super) fn offset(zone: Zone, t: f64) -> f64 {
    match zone {
        Zone::Host => host_offset(t),
        Zone::Utc => 0.0,
        Zone::Fixed(minutes) => f64::from(minutes) * 60.0,
        Zone::Rules(i) => zone_offset(i, t),
    }
}

// The host's zone.

/// The host's offset from UTC at `t` (ms), in seconds, as QuickJS's
/// `Date` works it out: `-new Date(t).getTimezoneOffset() * 60`.
pub(super) fn host_offset(t: f64) -> f64 {
    -timezone_offset_minutes(t) * 60.0
}

#[cfg(unix)]
fn timezone_offset_minutes(t: f64) -> f64 {
    // As QuickJS's getTimezoneOffset: whole seconds, rounded toward zero,
    // given to localtime_r.
    let time = (t.trunc() as i64 / 1000) as libc::time_t;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: both pointers are valid for the call; `tm` is zeroed, so it
    // is initialized whether or not localtime_r fills it in.
    let tm = unsafe {
        libc::localtime_r(&time, tm.as_mut_ptr());
        tm.assume_init()
    };
    ((-tm.tm_gmtoff) / 60) as f64
}

#[cfg(not(unix))]
fn timezone_offset_minutes(_t: f64) -> f64 {
    0.0
}

/// The host's zone: UTC, the first zone in the table whose offsets are
/// the host's, or a fixed offset.
pub(super) struct Host {
    pub(super) name: String,
    /// The zone with rules it matched, and every name with the same rules.
    rec: Option<(usize, Vec<usize>)>,
    /// The other zones it could be that have had names of their own.
    rivals: OnceLock<Vec<usize>>,
}

/// The host's zone: UTC, or the first zone in the table whose offsets are
/// the host's, twice a year since 1970 and around each of its own
/// transitions. Years are tried oldest first, so a zone that is not the
/// host's is dropped before its later years are worked out. Zones that
/// have kept the same time since 1970 cannot be told apart, so a host in
/// Oslo reports Europe/Berlin. A host that matches no zone reports the
/// Etc/GMT zone (or offset) of its winter offset, without rules.
pub(super) fn host() -> &'static Host {
    static HOST: OnceLock<Host> = OnceLock::new();
    HOST.get_or_init(|| {
        let at = |s: f64| host_offset(s * 1000.0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let last = year_of_ms((now * 1000.0).floor());
        let mut probes = Vec::new();
        let mut y = 1970.0;
        while y <= last {
            probes.push(day_utc(y, 0.0, 15.0) * 86400.0);
            probes.push(day_utc(y, 6.0, 15.0) * 86400.0);
            y += 1.0;
        }
        let offsets: Vec<f64> = probes.iter().map(|&p| at(p)).collect();
        let start = at(0.0);
        let plain = |name: String| Host {
            name,
            rec: None,
            rivals: OnceLock::new(),
        };
        if start == 0.0 && offsets.iter().all(|&o| o == 0.0) {
            return plain("UTC".to_owned());
        }
        let key = (start % 30.0 == 0.0).then(|| {
            format!(
                "{}{}",
                if start < 0.0 { "-" } else { "" },
                base36((start.abs() / 30.0) as u64)
            )
        });
        let tables = tables();
        for entry in &tables.entries {
            if Some(entry.start) != key.as_deref() {
                continue;
            }
            let zone = entry.first;
            if !probes
                .iter()
                .zip(&offsets)
                .all(|(&p, &o)| zone_offset(zone, p * 1000.0) == o)
            {
                continue;
            }
            let fits = with_transitions(zone, f64::NEG_INFINITY, |z| {
                z.at.iter().enumerate().all(|(i, &s)| {
                    i == 0 || s > now || (at(s) == z.off[i] && at(s - 1.0) == z.off[i - 1])
                })
            });
            if fits {
                return Host {
                    name: tables.names[zone].clone(),
                    rec: Some((zone, (entry.first..entry.first + entry.count).collect())),
                    rivals: OnceLock::new(),
                };
            }
        }
        let winter = at(946_684_800.0) / 60.0;
        plain(if winter == 0.0 {
            "UTC".to_owned()
        } else if winter % 60.0 != 0.0 {
            offset_name(winter as i32)
        } else {
            format!(
                "Etc/GMT{}{}",
                if winter > 0.0 { '-' } else { '+' },
                (winter / 60.0).abs()
            )
        })
    })
}

impl Host {
    /// The other zones the host could be that have had names of their own,
    /// as Dublin has beside London. Only zones named for a place count.
    fn rivals(&self) -> &[usize] {
        self.rivals.get_or_init(|| {
            let Some((zone, names)) = &self.rec else {
                return Vec::new();
            };
            let tables = tables();
            let eras = |name: usize| ZONE_ERAS.get(name * 2..name * 2 + 2);
            names
                .iter()
                .filter(|&&name| is_place(&tables.names[name]) && eras(name) != eras(*zone))
                .filter_map(|&name| find_zone(&tables.names[name]))
                .collect()
        })
    }
}

// Zone names.

const ZONE_WORDS: [[&str; 4]; 2] = [
    [" Time", " Standard Time", " Daylight Time", " Summer Time"],
    ["Zeit", "Normalzeit", "Sommerzeit", "Sommerzeit"],
];

/// A metazone: its long [generic, standard, daylight] names per language,
/// the zones its generic name stands for, its short names per letter, and
/// the metazone whose names a zone's own stand in for.
struct Metazone {
    names: [[String; 3]; 2],
    golden: &'static str,
    shorts: Vec<&'static str>,
    base: usize,
}

/// Where a zone has been, as (from ms, metazone number or -1, standard
/// offset in seconds or NaN).
type Meta = (f64, i32, f64);

/// Where zone record `z` stands outside its eras.
const NO_META: Meta = (0.0, -1, f64::NAN);

struct Names {
    metazones: Vec<Metazone>,
    eras: Vec<Vec<Meta>>,
}

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| {
        let metazones = METAZONES
            .split('|')
            .enumerate()
            .map(|(i, m)| {
                let mut parts = m.split('~');
                let texts = [parts.next().unwrap_or(""), parts.next().unwrap_or("")];
                let golden = parts.next().unwrap_or("");
                let shorts = parts
                    .next()
                    .map_or_else(Vec::new, |s| s.split(';').collect());
                let base = parts
                    .next()
                    .filter(|b| !b.is_empty())
                    .map_or(i, |b| parse36(b) as usize);
                let names = [0, 1].map(|language| {
                    let text = texts[language];
                    if let Some(name) = text.strip_prefix('=') {
                        return [String::new(), name.to_owned(), String::new()];
                    }
                    match text.find('*') {
                        None => {
                            let mut list = text.split(';').map(str::to_owned);
                            [0, 1, 2].map(|_| list.next().unwrap_or_default())
                        }
                        Some(star) => {
                            let stem = &text[..star];
                            let words = ZONE_WORDS[language];
                            let summer = text.as_bytes().get(star + 1) == Some(&b'S');
                            [
                                format!("{stem}{}", words[0]),
                                format!("{stem}{}", words[1]),
                                format!("{stem}{}", words[if summer { 3 } else { 2 }]),
                            ]
                        }
                    }
                });
                Metazone {
                    names,
                    golden,
                    shorts,
                    base,
                }
            })
            .collect();
        let eras = ERAS
            .split('|')
            .map(|list| {
                if list.is_empty() {
                    return Vec::new();
                }
                list.split(',')
                    .map(|era| {
                        let (body, from) = match era.split_once('@') {
                            None => (era, f64::NEG_INFINITY),
                            Some((body, at)) => (body, parse36(at) * 1000.0),
                        };
                        let b = body.as_bytes();
                        let meta = if !b.is_empty() && b[0] != b'-' {
                            parse36(head(body)) as i32
                        } else {
                            -1
                        };
                        let std = if !b.is_empty() && b.get(2) != Some(&b'!') {
                            let sign = if b.get(2) == Some(&b'-') { -30.0 } else { 30.0 };
                            sign * parse36(body.get(3..).unwrap_or(""))
                        } else {
                            f64::NAN
                        };
                        (from, meta, std)
                    })
                    .collect()
            })
            .collect();
        Names { metazones, eras }
    })
}

/// The metazones zone number `zone` has been in.
fn metas(zone: usize) -> &'static [Meta] {
    let list = ZONE_ERAS
        .get(zone * 2..zone * 2 + 2)
        .map_or(f64::NAN, parse36);
    names()
        .eras
        .get(list as usize)
        .filter(|_| list >= 0.0)
        .map_or(&[], Vec::as_slice)
}

/// Where zone `zone` stands at `t` (ms): -1 where it is in no metazone,
/// and NaN where its standard offset is not known. The standard offsets
/// come from the names Chrome gives: the offset it calls standard time,
/// and for a time in no metazone one that makes the generic names after
/// it come out as Chrome's.
fn zone_meta(zone: usize, t: f64) -> Meta {
    metas(zone)
        .iter()
        .rev()
        .find(|m| m.0 <= t)
        .copied()
        .unwrap_or(NO_META)
}

/// How a locale names zones.
pub(super) struct ZoneStyle {
    /// The language of the long names: 0 English, 1 German.
    pub(super) language: usize,
    /// The letters that mark the locale's short names.
    pub(super) shorts: &'static str,
    /// The region whose zones stand for a metazone.
    pub(super) region: &'static str,
    /// Words spelt the locale's way.
    pub(super) spelling: &'static [(&'static str, &'static str)],
    /// What UTC is called, long.
    pub(super) utc: &'static str,
}

/// The short [standard, daylight, generic] names metazone `m` has in the
/// locale, if any.
fn short_names(style: &ZoneStyle, m: &Metazone) -> Option<Vec<&'static str>> {
    let part = m.shorts.iter().find(|part| {
        part.chars()
            .next()
            .is_some_and(|c| style.shorts.contains(c))
    })?;
    Some(part.get(2..).unwrap_or("").split(',').collect())
}

/// The short name number `which` of metazone number `i`, or of its base.
fn short_name(style: &ZoneStyle, i: usize, which: usize) -> &'static str {
    let all = &names().metazones;
    let m = &all[i];
    short_names(style, m)
        .or_else(|| short_names(style, &all[m.base]))
        .and_then(|list| list.get(which).copied())
        .unwrap_or("")
}

/// The long name of metazone `m` numbered `which` in its names, spelt the
/// locale's way.
fn long_name(style: &ZoneStyle, m: &Metazone, which: usize) -> String {
    let mut name = m.names[style.language][which].clone();
    for (from, to) in style.spelling {
        name = name.replacen(from, to, 1);
    }
    name
}

/// The standard or daylight name of metazone number `i`, or ''.
fn specific_name(style: &ZoneStyle, i: usize, daylight: bool, long: bool) -> String {
    if long {
        return long_name(style, &names().metazones[i], 1 + usize::from(daylight));
    }
    short_name(style, i, usize::from(daylight)).to_owned()
}

/// Whether zone `zone` is on daylight time within 184 days of `t`, going
/// by the change before it and the one after, as ICU does. A change is one
/// of offset or of standard offset: a zone can move to another metazone's
/// standard time from its own daylight time and keep its clocks where they
/// are.
fn saves_near(zone: usize, t: f64) -> bool {
    let s = (t / 1000.0).floor();
    let shift = zone_shift(s);
    let now = s - shift;
    let (mut before, mut after, mut was, mut will, current) = zone_index(zone, s, false, |z, i| {
        (
            if i > 0 { z.at[i] } else { f64::NEG_INFINITY },
            z.at.get(i + 1).copied().unwrap_or(f64::INFINITY),
            if i > 0 { z.off[i - 1] } else { f64::NAN },
            z.off.get(i + 1).copied().unwrap_or(f64::NAN),
            z.off[i],
        )
    });
    // Where the standard offset is not known, the zone counts as on it.
    let saves = |off: f64, at: f64| {
        let std = zone_meta(zone, (at + shift) * 1000.0).2;
        !std.is_nan() && off != std
    };
    let list = metas(zone);
    for (k, &(from, _, std)) in list.iter().enumerate() {
        let last = if k > 0 { list[k - 1].2 } else { 0.0 };
        let from = from / 1000.0 - shift;
        if k == 0 || std.is_nan() || last.is_nan() || std == last {
            continue;
        }
        if from <= now && from > before {
            (before, was) = (from, current);
        }
        if from > now && from < after {
            (after, will) = (from, current);
        }
    }
    (now - before < HALF_YEAR && saves(was, before - 1.0))
        || (after - now < HALF_YEAR && saves(will, after))
}

/// `Object.is` for numbers.
fn same(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a == b && a.is_sign_negative() == b.is_sign_negative())
}

/// The generic name of zone `zone`, which is in metazone number `i` with
/// standard offset `std`, as ICU chooses it: the standard name where the
/// zone keeps one time all year, else the metazone's generic name if the
/// zone keeps the time of the metazone's own zone. The names ICU makes
/// from a country or city are not here, and are ''.
fn generic_name(
    style: &ZoneStyle,
    zone: usize,
    i: usize,
    std: f64,
    t: f64,
    off: f64,
    long: bool,
) -> String {
    let all = &names().metazones;
    let base = &all[all[i].base];
    let generic = if long {
        long_name(style, base, 0)
    } else {
        short_name(style, i, 2).to_owned()
    };
    if off == std && !saves_near(zone, t) {
        let name = specific_name(style, i, false, long);
        if !name.is_empty() && name != generic {
            return name;
        }
    }
    if generic.is_empty() || base.golden.is_empty() {
        return String::new();
    }
    let zones: Vec<&str> = base.golden.split(',').collect();
    let golden = match zones.iter().find(|name| name.starts_with(style.region)) {
        Some(name) => expand(name.get(2..).unwrap_or("")),
        None => expand(zones[0]),
    };
    if golden != tables().names[zone] {
        let Some(g) = find_zone(&golden) else {
            return String::new();
        };
        let local = (t / 1000.0).floor() + off;
        let other = zone_index(g, local, true, |z, j| z.off[j]);
        if other != off || !same(zone_meta(g, (local - other) * 1000.0).2, std) {
            return String::new();
        }
    }
    generic
}

/// 'GMT-5' or, long, 'GMT-05:00', for an offset in seconds.
fn offset_text(off: f64, long: bool) -> String {
    let a = off.abs() as i64;
    let (hh, mm, ss) = (a / 3600, a / 60 % 60, a % 60);
    let sign = if off < 0.0 { '-' } else { '+' };
    let mut out = if long {
        format!("GMT{sign}{hh:02}:{mm:02}")
    } else if mm != 0 || ss != 0 {
        format!("GMT{sign}{hh}:{mm:02}")
    } else {
        format!("GMT{sign}{hh}")
    };
    if ss != 0 {
        out += &format!(":{ss:02}");
    }
    out
}

/// The zone's name for pattern letter `c` of width `n`: an offset for O, a
/// standard or daylight name for z and a generic one for v, or the offset
/// where the locale has no such name.
pub(super) fn display_name(
    style: &ZoneStyle,
    zone: Zone,
    c: u8,
    n: usize,
    t: f64,
    off: f64,
) -> String {
    let long = n > 3;
    if c == b'O' {
        return offset_text(off, long);
    }
    let host = matches!(zone, Zone::Host).then(host);
    let utc = match zone {
        Zone::Utc => true,
        Zone::Host => host.is_some_and(|h| h.name == "UTC"),
        _ => false,
    };
    if utc {
        return if c != b'z' {
            offset_text(off, long)
        } else if long {
            style.utc.to_owned()
        } else {
            "UTC".to_owned()
        };
    }
    let rec = match zone {
        Zone::Rules(i) => Some(i),
        Zone::Host => host.and_then(|h| h.rec.as_ref().map(|r| r.0)),
        _ => None,
    };
    let mut name = String::new();
    if let Some(rec) = rec {
        let named = |rec: usize| {
            let (_, i, std) = zone_meta(rec, t);
            if i < 0 {
                String::new()
            } else if c == b'z' {
                specific_name(style, i as usize, off != std, long)
            } else {
                generic_name(style, rec, i as usize, std, t, off, long)
            }
        };
        name = named(rec);
        // The host has a name where every zone it could be has that name.
        if let Some(host) = host
            && !name.is_empty()
            && host.rivals().iter().any(|&rival| named(rival) != name)
        {
            name.clear();
        }
    } else if off == 0.0 && host.is_none() && c == b'z' && style.language == 0 {
        // An offset of zero given as an offset.
        name = if long { "Greenwich Mean Time" } else { "GMT" }.to_owned();
    }
    if name.is_empty() {
        offset_text(off, long)
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_unpack() {
        let t = tables();
        assert_eq!(t.names.len() * 2, ZONE_ERAS.len());
        assert_eq!(
            find_zone("america/new_york").map(|i| t.names[i].as_str()),
            Some("America/New_York")
        );
        assert_eq!(
            find_zone("Asia/Calcutta").map(|i| t.names[i].as_str()),
            Some("Asia/Calcutta")
        );
        // A link leads to the name it stands for.
        assert!(find_zone("America/Buenos_Aires").is_some());
        assert_eq!(find_zone("Nope/Zone"), None);
        assert_eq!(find_zone("0Toronto"), None);
        assert_eq!(names().metazones.len(), METAZONES.split('|').count());
    }

    #[test]
    fn offsets_and_names() {
        let ny = find_zone("America/New_York").unwrap();
        // 2026-07-15 and 2026-01-15, noon UTC.
        assert_eq!(zone_offset(ny, 1_784_116_800_000.0), -4.0 * 3600.0);
        assert_eq!(zone_offset(ny, 1_768_478_400_000.0), -5.0 * 3600.0);
        assert_eq!(parse_offset("+05:30"), Some(330));
        assert_eq!(parse_offset("-0800"), Some(-480));
        assert_eq!(parse_offset("+01"), Some(60));
        assert_eq!(parse_offset("+24:00"), None);
        assert_eq!(offset_name(-0), "+00:00");
        assert_eq!(offset_text(19800.0, false), "GMT+5:30");
        assert_eq!(offset_text(-18000.0, true), "GMT-05:00");
        assert!(is_utc_name("Etc/GMT-0") && is_utc_name("ZULU") && !is_utc_name("Etc/GMT+1"));
    }

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(-1), (1969, 11, 31));
        assert_eq!(day_utc(2026.0, 12.0, 0.0), day_utc(2026.0, 11.0, 31.0));
        assert_eq!(year_of_ms(8.64e15), 275_760.0);
        assert!(year_of_ms(8.64e15 + 1.0).is_nan());
        assert_eq!(base36(1296), "100");
        assert_eq!(parse36("-3c"), -120.0);
        assert!(parse36("").is_nan());
    }
}
