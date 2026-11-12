//! Where a regular expression search tries to match.
//!
//! QuickJS compiles a search (any regexp without the sticky flag) as the
//! pattern with a loop in front: try here, else skip a character and try
//! again. Its interpreter runs that loop like any other part of the
//! pattern, so it saves a backtracking state at every position of the
//! subject and starts the pattern at each one, including the positions
//! where the first character already rules a match out. Pages search long
//! strings (user agents, cookies, URLs, whole stylesheets) with patterns
//! that start with a literal or a small class, and most of the time went
//! there.
//!
//! The engine patch in `vendor/rquickjs-sys` (see `PATCHES.md`) lets this
//! module run the loop instead. [`search`] reads the compiled pattern once
//! to find how a match can start ([`Start`]), then calls back into the
//! engine's matcher only at the positions that qualify. The matcher itself
//! is untouched, so what matches, and what each group captures, is decided
//! by the same code as before. Only the positions it is asked about change,
//! and a position is left out only when the pattern cannot match there:
//! the set of first characters is an over-estimate, never an under-estimate.

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};
use std::rc::Rc;

type Hook = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    *const u8,
    c_int,
    c_int,
    c_int,
    c_int,
) -> c_int;

unsafe extern "C" {
    fn lre_set_search_hook(hook: Option<Hook>);
    fn lre_search_try(search: *mut c_void, index: c_int) -> c_int;
    fn lre_canonicalize(c: u32, is_unicode: bool) -> c_int;
}

/// Hands regexp searches in this process to [`search`]. Safe to call more
/// than once and from any thread: it stores the same pointer each time.
pub(crate) fn install() {
    // SAFETY: `search` has the signature the engine calls the hook with,
    // and the engine reads the pointer only when it runs a regexp.
    unsafe { lre_set_search_hook(Some(search)) };
}

/// Puts the engine's own search loop back. For tests that compare the
/// two. The next [`JsRuntime`](crate::JsRuntime) created installs
/// [`search`] again.
#[doc(hidden)]
pub fn uninstall() {
    // SAFETY: a null hook is the engine's default.
    unsafe { lre_set_search_hook(None) };
}

thread_local! {
    static SEARCHES: Cell<u64> = const { Cell::new(0) };
}

/// How many searches [`search`] has run on this thread. For tests: an
/// engine update that changed how a search is compiled would leave the
/// hook installed and never called.
#[doc(hidden)]
pub fn searches() -> u64 {
    SEARCHES.get()
}

// `LRE_FLAG_*` in libregexp.h.
const IGNORE_CASE: c_int = 1 << 1;
const MULTILINE: c_int = 1 << 2;
const UNICODE: c_int = 1 << 4;

/// The opcodes of libregexp's bytecode. A static assertion beside the hook
/// in libregexp.c fails the build if the engine renumbers them.
mod op {
    pub(super) const CHAR8: u8 = 1;
    pub(super) const CHAR16: u8 = 2;
    pub(super) const CHAR32: u8 = 3;
    pub(super) const LINE_START: u8 = 6;
    pub(super) const LINE_END: u8 = 7;
    pub(super) const GOTO: u8 = 8;
    pub(super) const SPLIT_GOTO_FIRST: u8 = 9;
    pub(super) const SPLIT_NEXT_FIRST: u8 = 10;
    pub(super) const SAVE_START: u8 = 12;
    pub(super) const SAVE_END: u8 = 13;
    pub(super) const SAVE_RESET: u8 = 14;
    pub(super) const LOOP: u8 = 15;
    pub(super) const PUSH_I32: u8 = 16;
    pub(super) const DROP: u8 = 17;
    pub(super) const WORD_BOUNDARY: u8 = 18;
    pub(super) const NOT_WORD_BOUNDARY: u8 = 19;
    pub(super) const RANGE: u8 = 22;
    pub(super) const RANGE32: u8 = 23;
    pub(super) const LOOKAHEAD: u8 = 24;
    pub(super) const NEGATIVE_LOOKAHEAD: u8 = 25;
    pub(super) const PUSH_CHAR_POS: u8 = 26;
    pub(super) const CHECK_ADVANCE: u8 = 27;
    pub(super) const SIMPLE_GREEDY_QUANT: u8 = 29;
}

/// What the start of a pattern says about where it can match.
#[derive(Debug, PartialEq)]
enum Start {
    /// Nothing: every position is tried.
    Anywhere,
    /// The pattern begins with `^`: only the start of the subject can
    /// match or, with the `m` flag, the start of a line.
    Line,
    /// A match begins with one of these characters.
    Chars(Box<Chars>),
}

/// A set of characters, as the matcher compares them: with the `i` flag
/// the ranges hold canonical (case-folded) characters and a subject
/// character is folded before it is looked up.
#[derive(Debug, PartialEq)]
struct Chars {
    /// Inclusive ranges, unsorted.
    ranges: Vec<(u32, u32)>,
    /// The answer for each character below 256, folding included.
    latin1: [bool; 256],
    /// The characters below 256 that are in the set, when there are at
    /// most three: `memchr` finds those faster than the table does.
    few: Option<Vec<u8>>,
}

impl Chars {
    fn new(ranges: Vec<(u32, u32)>, flags: c_int) -> Self {
        let mut latin1 = [false; 256];
        let mut members = Vec::new();
        for c in 0..=255u8 {
            if in_ranges(&ranges, fold(u32::from(c), flags)) {
                latin1[usize::from(c)] = true;
                members.push(c);
            }
        }
        let few = (members.len() <= 3).then_some(members);
        Self {
            ranges,
            latin1,
            few,
        }
    }

    fn contains(&self, c: u32, flags: c_int) -> bool {
        match usize::try_from(c) {
            Ok(i) if i < 256 => self.latin1[i],
            _ => in_ranges(&self.ranges, fold(c, flags)),
        }
    }

    /// The first position at or after `from` in an 8-bit subject that
    /// holds a character of the set.
    fn find8(&self, subject: &[u8], from: usize) -> Option<usize> {
        let rest = subject.get(from..)?;
        let found = match self.few.as_deref() {
            Some([]) => None,
            Some([a]) => memchr::memchr(*a, rest),
            Some([a, b]) => memchr::memchr2(*a, *b, rest),
            Some([a, b, c]) => memchr::memchr3(*a, *b, *c, rest),
            _ => rest.iter().position(|&c| self.latin1[usize::from(c)]),
        };
        found.map(|i| from + i)
    }
}

fn in_ranges(ranges: &[(u32, u32)], c: u32) -> bool {
    ranges.iter().any(|&(low, high)| low <= c && c <= high)
}

/// The character the matcher compares `c` as.
fn fold(c: u32, flags: c_int) -> u32 {
    if flags & IGNORE_CASE == 0 {
        return c;
    }
    // SAFETY: `lre_canonicalize` is a pure function of its arguments.
    let folded = unsafe { lre_canonicalize(c, flags & UNICODE != 0) };
    u32::try_from(folded).unwrap_or(c)
}

fn u16_at(code: &[u8], at: usize) -> Option<u32> {
    let bytes = code.get(at..at.checked_add(2)?)?;
    Some(u32::from(u16::from_le_bytes([bytes[0], bytes[1]])))
}

fn u32_at(code: &[u8], at: usize) -> Option<u32> {
    let bytes = code.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Where a jump with its 32-bit offset at `at` lands: offsets count from
/// the end of the operand.
fn target(code: &[u8], at: usize) -> Option<usize> {
    let bytes = code.get(at..at.checked_add(4)?)?;
    let offset = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let to = i64::try_from(at).ok()? + 4 + i64::from(offset);
    usize::try_from(to).ok()
}

/// Reads the start of a compiled pattern (the bytecode after the search
/// loop). Follows every way into the pattern as far as the first thing
/// that consumes a character, and collects those characters. Anything it
/// does not understand, or any way through that consumes nothing (the
/// pattern can then match the empty string, so it can match anywhere),
/// gives [`Start::Anywhere`].
fn analyse(code: &[u8], flags: c_int) -> Start {
    // Every pattern opens with `save_start 0`.
    if code.first() == Some(&op::SAVE_START) && code.get(2) == Some(&op::LINE_START) {
        return Start::Line;
    }
    first_chars(code).map_or(Start::Anywhere, |ranges| {
        Start::Chars(Box::new(Chars::new(ranges, flags)))
    })
}

fn first_chars(code: &[u8]) -> Option<Vec<(u32, u32)>> {
    let mut ranges = Vec::new();
    let mut seen = vec![false; code.len()];
    let mut todo = vec![0usize];
    while let Some(mut pc) = todo.pop() {
        loop {
            let opcode = *code.get(pc)?;
            if std::mem::replace(seen.get_mut(pc)?, true) {
                break;
            }
            match opcode {
                op::CHAR8 => {
                    let c = u32::from(*code.get(pc + 1)?);
                    ranges.push((c, c));
                    break;
                }
                op::CHAR16 => {
                    let c = u16_at(code, pc + 1)?;
                    ranges.push((c, c));
                    break;
                }
                op::CHAR32 => {
                    let c = u32_at(code, pc + 1)?;
                    ranges.push((c, c));
                    break;
                }
                op::RANGE => {
                    let count = usize::try_from(u16_at(code, pc + 1)?).ok()?;
                    for i in 0..count {
                        let low = u16_at(code, pc + 3 + 4 * i)?;
                        let mut high = u16_at(code, pc + 5 + 4 * i)?;
                        // In the last pair 0xffff stands for no upper end.
                        if i + 1 == count && high == 0xffff {
                            high = u32::MAX;
                        }
                        ranges.push((low, high));
                    }
                    break;
                }
                op::RANGE32 => {
                    let count = usize::try_from(u16_at(code, pc + 1)?).ok()?;
                    for i in 0..count {
                        let low = u32_at(code, pc + 3 + 8 * i)?;
                        let high = u32_at(code, pc + 7 + 8 * i)?;
                        ranges.push((low, high));
                    }
                    break;
                }
                // These consume nothing. Reading past an assertion as if
                // it always held only makes the set larger.
                op::SAVE_START | op::SAVE_END => pc += 2,
                op::SAVE_RESET => pc += 3,
                op::LINE_START
                | op::LINE_END
                | op::WORD_BOUNDARY
                | op::NOT_WORD_BOUNDARY
                | op::DROP
                | op::PUSH_CHAR_POS
                | op::CHECK_ADVANCE => pc += 1,
                op::PUSH_I32 => pc += 5,
                op::GOTO => pc = target(code, pc + 1)?,
                op::SPLIT_GOTO_FIRST | op::SPLIT_NEXT_FIRST | op::LOOP => {
                    todo.push(target(code, pc + 1)?);
                    pc += 5;
                }
                // A lookaround's own pattern follows the opcode and what
                // comes after the lookaround is at the jump target. The
                // lookaround consumes nothing, so only the latter counts.
                op::LOOKAHEAD | op::NEGATIVE_LOOKAHEAD => pc = target(code, pc + 1)?,
                // next, min, max and length as 32-bit operands, then the
                // repeated part. With a minimum of zero the pattern can
                // also go straight on to what follows.
                op::SIMPLE_GREEDY_QUANT => {
                    if u32_at(code, pc + 5)? == 0 {
                        let next = usize::try_from(u32_at(code, pc + 1)?).ok()?;
                        todo.push(pc.checked_add(17)?.checked_add(next)?);
                    }
                    pc += 17;
                }
                // `.` and `[^]` start with nearly anything, a back
                // reference with whatever was captured, and reaching the
                // end means the empty string matches.
                _ => return None,
            }
        }
    }
    Some(ranges)
}

/// How many patterns keep their [`Start`]. A page runs a handful of
/// patterns in its hot loops and the rest once or twice.
const CACHE_SLOTS: usize = 64;

struct Cached {
    code: Box<[u8]>,
    flags: c_int,
    start: Rc<Start>,
}

thread_local! {
    static CACHE: RefCell<Vec<Option<Cached>>> =
        RefCell::new((0..CACHE_SLOTS).map(|_| None).collect());
}

/// The [`Start`] of a pattern, worked out the first time it is searched
/// with. A slot is found by where the bytecode is and checked by what it
/// is, so a pattern freed and another put at its address is noticed.
fn start_of(code: &[u8], flags: c_int) -> Rc<Start> {
    let slot = (code.as_ptr().addr() >> 4 ^ code.len()) % CACHE_SLOTS;
    CACHE.with_borrow_mut(|cache| {
        if let Some(hit) = &cache[slot] {
            if hit.flags == flags && *hit.code == *code {
                return Rc::clone(&hit.start);
            }
        }
        let start = Rc::new(analyse(code, flags));
        cache[slot] = Some(Cached {
            code: code.into(),
            flags,
            start: Rc::clone(&start),
        });
        start
    })
}

const fn is_high_surrogate(unit: u16) -> bool {
    unit & 0xfc00 == 0xd800
}

const fn is_low_surrogate(unit: u16) -> bool {
    unit & 0xfc00 == 0xdc00
}

/// The subject of a search: Latin-1 bytes or UTF-16 code units.
#[derive(Clone, Copy)]
enum Subject<'a> {
    Narrow(&'a [u8]),
    /// `pairs` is set when the pattern has the `u` or `v` flag: a
    /// surrogate pair is then one character and no match starts inside it.
    Wide {
        units: &'a [u16],
        pairs: bool,
    },
}

impl Subject<'_> {
    fn len(self) -> usize {
        match self {
            Self::Narrow(bytes) => bytes.len(),
            Self::Wide { units, .. } => units.len(),
        }
    }

    /// The character at `at`, which is before the end, and how many code
    /// units it takes.
    fn char_at(self, at: usize) -> (u32, usize) {
        match self {
            Self::Narrow(bytes) => (u32::from(bytes[at]), 1),
            Self::Wide { units, pairs } => {
                let unit = units[at];
                let low = units
                    .get(at + 1)
                    .copied()
                    .filter(|&next| is_low_surrogate(next));
                match low {
                    Some(low) if pairs && is_high_surrogate(unit) => {
                        let c =
                            0x10000 + ((u32::from(unit) & 0x3ff) << 10) + (u32::from(low) & 0x3ff);
                        (c, 2)
                    }
                    _ => (u32::from(unit), 1),
                }
            }
        }
    }

    fn is_line_terminator(self, at: usize) -> bool {
        let unit = match self {
            Self::Narrow(bytes) => u32::from(bytes[at]),
            Self::Wide { units, .. } => u32::from(units[at]),
        };
        matches!(unit, 0x0a | 0x0d | 0x2028 | 0x2029)
    }
}

/// Runs a search. Called by `lre_exec` in place of the loop compiled in
/// front of the pattern.
///
/// `body` is the pattern's bytecode after that loop. `cbuf_type` is 0 for
/// a Latin-1 subject, 1 for UTF-16 and 2 for UTF-16 where surrogate pairs
/// count as one character. The search starts at code unit `cindex` of
/// `clen`. Returns what `lre_exec` returns: 1 for a match (the engine has
/// then filled in the captures), 0 for none, negative for an error.
///
/// # Safety
///
/// Only the engine calls this, with `search` its own state for
/// `lre_search_try`, `body` valid for `body_len` bytes and `cbuf` valid
/// for `clen` code units, aligned for them.
unsafe extern "C" fn search(
    search: *mut c_void,
    body: *const u8,
    body_len: usize,
    cbuf: *const u8,
    cbuf_type: c_int,
    cindex: c_int,
    clen: c_int,
    flags: c_int,
) -> c_int {
    let (Ok(from), Ok(len)) = (usize::try_from(cindex), usize::try_from(clen)) else {
        return 0;
    };
    // SAFETY: the caller's contract, for all three.
    let (code, subject) = unsafe {
        let code = std::slice::from_raw_parts(body, body_len);
        let subject = if cbuf_type == 0 {
            Subject::Narrow(std::slice::from_raw_parts(cbuf, len))
        } else {
            Subject::Wide {
                units: std::slice::from_raw_parts(cbuf.cast::<u16>(), len),
                pairs: cbuf_type == 2,
            }
        };
        (code, subject)
    };
    SEARCHES.set(SEARCHES.get() + 1);
    let start = start_of(code, flags);
    let try_at = |at: usize| -> c_int {
        // SAFETY: `search` is the engine's, and `at` is within the subject
        // or at its end. It fits a `c_int` because `clen` did.
        unsafe { lre_search_try(search, at as c_int) }
    };
    run(&start, subject, from, flags, try_at)
}

/// The search loop: tries the pattern at each position `start` allows,
/// from `from` to the end of the subject, until `try_at` reports something
/// other than "no match here".
fn run(
    start: &Start,
    subject: Subject<'_>,
    from: usize,
    flags: c_int,
    mut try_at: impl FnMut(usize) -> c_int,
) -> c_int {
    let len = subject.len();
    if from > len {
        return 0;
    }
    match start {
        // The end of the subject is a position too: `/$/` matches there.
        Start::Anywhere => {
            let mut at = from;
            loop {
                let found = try_at(at);
                if found != 0 || at >= len {
                    return found;
                }
                at += subject.char_at(at).1;
            }
        }
        Start::Line => {
            if from == 0 || (flags & MULTILINE != 0 && subject.is_line_terminator(from - 1)) {
                let found = try_at(from);
                if found != 0 {
                    return found;
                }
            }
            if flags & MULTILINE == 0 {
                return 0;
            }
            // A line terminator is never half of a surrogate pair, so the
            // position after one is always the start of a character.
            for at in from..len {
                if subject.is_line_terminator(at) {
                    let found = try_at(at + 1);
                    if found != 0 {
                        return found;
                    }
                }
            }
            0
        }
        Start::Chars(chars) => {
            let mut at = from;
            if let Subject::Narrow(bytes) = subject {
                while let Some(hit) = chars.find8(bytes, at) {
                    let found = try_at(hit);
                    if found != 0 {
                        return found;
                    }
                    at = hit + 1;
                }
                return 0;
            }
            while at < len {
                let (c, width) = subject.char_at(at);
                if chars.contains(c, flags) {
                    let found = try_at(at);
                    if found != 0 {
                        return found;
                    }
                }
                at += width;
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `save_start 0` then the given code.
    fn body(rest: &[u8]) -> Vec<u8> {
        let mut code = vec![op::SAVE_START, 0];
        code.extend_from_slice(rest);
        code
    }

    fn chars_of(start: &Start) -> Vec<u8> {
        match start {
            Start::Chars(chars) => (0..=255u8)
                .filter(|&c| chars.latin1[usize::from(c)])
                .collect(),
            other => panic!("expected a character set, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_starts_with_its_first_character() {
        let code = body(&[op::CHAR8, b'f', op::CHAR8, b'o', 11]);
        assert_eq!(chars_of(&analyse(&code, 0)), b"f");
    }

    #[test]
    fn an_alternation_starts_with_either_branch() {
        // split_next_first +7; char8 a; goto +2; char8 b; match
        let mut code = vec![op::SPLIT_NEXT_FIRST, 7, 0, 0, 0];
        code.extend_from_slice(&[op::CHAR8, b'a', op::GOTO, 2, 0, 0, 0]);
        code.extend_from_slice(&[op::CHAR8, b'b', 11]);
        assert_eq!(chars_of(&analyse(&body(&code), 0)), b"ab");
    }

    #[test]
    fn a_pattern_that_can_be_empty_starts_anywhere() {
        // split_next_first +2; char8 a; match
        let code = body(&[op::SPLIT_NEXT_FIRST, 2, 0, 0, 0, op::CHAR8, b'a', 11]);
        assert_eq!(analyse(&code, 0), Start::Anywhere);
    }

    #[test]
    fn a_dot_starts_anywhere() {
        assert_eq!(analyse(&body(&[4, 11]), 0), Start::Anywhere);
    }

    #[test]
    fn a_caret_starts_at_a_line() {
        assert_eq!(
            analyse(&body(&[op::LINE_START, op::CHAR8, b'a', 11]), 0),
            Start::Line
        );
    }

    #[test]
    fn truncated_bytecode_starts_anywhere() {
        assert_eq!(analyse(&body(&[op::CHAR16, b'a']), 0), Start::Anywhere);
        assert_eq!(
            analyse(&body(&[op::GOTO, 0xff, 0xff, 0xff, 0x7f]), 0),
            Start::Anywhere
        );
    }

    #[test]
    fn the_last_range_can_be_open_ended() {
        // range with one pair, 0x100 to "infinity"
        let code = body(&[op::RANGE, 1, 0, 0x00, 0x01, 0xff, 0xff, 11]);
        let Start::Chars(chars) = analyse(&code, 0) else {
            panic!("expected a character set");
        };
        assert!(chars.contains(0x1f600, 0));
        assert!(!chars.contains(u32::from(b'a'), 0));
        assert_eq!(chars.few.as_deref(), Some(&[][..]));
    }

    fn positions(start: &Start, subject: Subject<'_>, from: usize, flags: c_int) -> Vec<usize> {
        let mut tried = Vec::new();
        run(start, subject, from, flags, |at| {
            tried.push(at);
            0
        });
        tried
    }

    #[test]
    fn a_character_set_is_tried_only_where_it_occurs() {
        let start = analyse(&body(&[op::CHAR8, b'a', 11]), 0);
        let narrow = Subject::Narrow(b"banana");
        assert_eq!(positions(&start, narrow, 0, 0), [1, 3, 5]);
        assert_eq!(positions(&start, narrow, 2, 0), [3, 5]);
        let units: Vec<u16> = "b\u{4e2d}a\u{1f600}a".encode_utf16().collect();
        let wide = Subject::Wide {
            units: &units,
            pairs: true,
        };
        assert_eq!(positions(&start, wide, 0, 0), [2, 5]);
    }

    #[test]
    fn anywhere_includes_the_end_and_steps_over_pairs() {
        let units: Vec<u16> = "a\u{1f600}b".encode_utf16().collect();
        let pairs = Subject::Wide {
            units: &units,
            pairs: true,
        };
        assert_eq!(positions(&Start::Anywhere, pairs, 0, 0), [0, 1, 3, 4]);
        let units_only = Subject::Wide {
            units: &units,
            pairs: false,
        };
        assert_eq!(
            positions(&Start::Anywhere, units_only, 0, 0),
            [0, 1, 2, 3, 4]
        );
        // Starting inside a pair is the caller's choice and is kept.
        assert_eq!(positions(&Start::Anywhere, pairs, 2, 0), [2, 3, 4]);
    }

    #[test]
    fn a_caret_is_tried_at_line_starts_only() {
        let subject = Subject::Narrow(b"ab\ncd\n");
        assert_eq!(positions(&Start::Line, subject, 0, 0), [0]);
        assert_eq!(positions(&Start::Line, subject, 1, 0), [] as [usize; 0]);
        assert_eq!(positions(&Start::Line, subject, 0, MULTILINE), [0, 3, 6]);
        assert_eq!(positions(&Start::Line, subject, 3, MULTILINE), [3, 6]);
        assert_eq!(positions(&Start::Line, subject, 4, MULTILINE), [6]);
    }

    #[test]
    fn the_first_result_that_is_not_a_miss_ends_the_search() {
        let mut calls = 0;
        let found = run(&Start::Anywhere, Subject::Narrow(b"abc"), 0, 0, |at| {
            calls += 1;
            c_int::from(at == 1)
        });
        assert_eq!((found, calls), (1, 2));
        let failed = run(&Start::Anywhere, Subject::Narrow(b"abc"), 0, 0, |_| -2);
        assert_eq!(failed, -2);
    }
}
