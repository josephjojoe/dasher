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
//! engine's matcher only at the positions that qualify. What matches, and
//! what each group captures, is decided by the engine's matcher as before.
//! Only the positions it is asked about change, and a position is left out
//! only when the pattern cannot match there. Four things rule positions
//! out, all read from the bytecode into a [`Plan`] (the first two the
//! first time a pattern is searched with, the rest once it has been
//! searched with a few times):
//!
//! - the characters a match can start with ([`Start`]), an over-estimate,
//!   never an under-estimate;
//! - for a pattern that starts with an unbounded greedy repeat of one
//!   class, such as `[a-z0-9._%+-]+@…` ([`Plan::run`]): when the pattern
//!   fails at a position, it fails at every later position in the same run
//!   of class characters, because from there the repeat can only end at
//!   places it already ended at, and the rest of the pattern does not see
//!   where the match began. The search goes on after the run instead of
//!   retrying each suffix of a word, which is quadratic in its length;
//! - a set of characters every match contains at least one of
//!   ([`Plan::required`]): once none is left in the rest of the subject,
//!   no later position can match, which ends most searches over page text
//!   for the `@` of an email address at once;
//! - the first two characters ([`Plan::pairs`]): for each test a match can
//!   begin with, the characters that can follow it, so that
//!   `\b(?:casino|viagra)\b` is tried at `ca` and `vi` rather than at
//!   every `c` and `v`.

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};
use std::rc::Rc;

type Hook = unsafe extern "C" fn(
    *mut c_void,
    *mut *mut c_void,
    *const u8,
    usize,
    *const u8,
    c_int,
    c_int,
    c_int,
    c_int,
) -> c_int;

type FreePlan = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    fn lre_set_search_hook(hook: Option<Hook>, free_plan: Option<FreePlan>);
    fn lre_search_try(search: *mut c_void, index: c_int) -> c_int;
    fn lre_canonicalize(c: u32, is_unicode: bool) -> c_int;
    fn lre_set_fast_paths(on: bool);
}

/// Hands regexp searches in this process to [`search`] and turns on the
/// engine patch's other regexp fast paths (see `PATCHES.md`). Safe to
/// call more than once and from any thread: it stores the same values
/// each time.
pub(crate) fn install() {
    // SAFETY: `search` has the signature the engine calls the hook with,
    // and the engine reads the pointer only when it runs a regexp. The
    // flag is a plain global the engine reads.
    unsafe {
        lre_set_search_hook(Some(search), Some(free_plan));
        lre_set_fast_paths(true);
    }
}

/// Puts the engine's own search loop and RegExp methods back. For tests
/// that compare the two. The next [`JsRuntime`](crate::JsRuntime) created
/// installs dasher's again.
#[doc(hidden)]
pub fn uninstall() {
    // SAFETY: a null hook and the flag off are the engine's default.
    unsafe {
        lre_set_search_hook(None, None);
        lre_set_fast_paths(false);
    }
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
    pub(super) const DOT: u8 = 4;
    pub(super) const ANY: u8 = 5;
    pub(super) const LINE_START: u8 = 6;
    pub(super) const LINE_END: u8 = 7;
    pub(super) const GOTO: u8 = 8;
    pub(super) const SPLIT_GOTO_FIRST: u8 = 9;
    pub(super) const SPLIT_NEXT_FIRST: u8 = 10;
    pub(super) const MATCH: u8 = 11;
    pub(super) const SAVE_START: u8 = 12;
    pub(super) const SAVE_END: u8 = 13;
    pub(super) const SAVE_RESET: u8 = 14;
    pub(super) const LOOP: u8 = 15;
    pub(super) const PUSH_I32: u8 = 16;
    pub(super) const DROP: u8 = 17;
    pub(super) const WORD_BOUNDARY: u8 = 18;
    pub(super) const NOT_WORD_BOUNDARY: u8 = 19;
    pub(super) const BACK_REFERENCE: u8 = 20;
    pub(super) const BACKWARD_BACK_REFERENCE: u8 = 21;
    pub(super) const RANGE: u8 = 22;
    pub(super) const RANGE32: u8 = 23;
    pub(super) const LOOKAHEAD: u8 = 24;
    pub(super) const NEGATIVE_LOOKAHEAD: u8 = 25;
    pub(super) const PUSH_CHAR_POS: u8 = 26;
    pub(super) const CHECK_ADVANCE: u8 = 27;
    pub(super) const SIMPLE_GREEDY_QUANT: u8 = 29;
}

/// What the bytecode of a pattern says about where it can match.
#[derive(Debug, PartialEq)]
struct Plan {
    start: Start,
    /// Every match contains a character of this set, at or after where it
    /// starts. Only kept when it is rarer than the start's set.
    required: Option<Box<Chars>>,
    /// The pattern starts with an unbounded greedy repeat, at least once,
    /// of exactly the characters of [`Start::Chars`], with nothing in
    /// front of it: see the module documentation.
    run: bool,
    /// The pairs of characters a match can begin with, for a pattern that
    /// starts with [`Start::Chars`] and needs at least two characters.
    pairs: Option<Vec<Pair>>,
    /// Whether this is [`analyse`]'s plan, not [`analyse_quick`]'s.
    full: bool,
}

/// The most one-character tests a match can begin with that
/// [`second_chars`] keeps pairs for.
const PAIRS_MAX: usize = 16;

/// A character a match can begin with and one that can follow it, below
/// 256 and folded as the matcher compares them, as bit sets.
#[derive(Clone, Debug, PartialEq)]
struct Pair {
    first: [u64; 4],
    second: [u64; 4],
}

fn has_bit(bits: &[u64; 4], c: u32) -> bool {
    c < 256 && bits[(c / 64) as usize] & (1 << (c % 64)) != 0
}

/// Whether the characters at `at` could begin a match, by [`Pair`]s.
/// Characters from 256 up are not in the sets and always could.
fn pairs_allow(pairs: &[Pair], subject: Subject<'_>, at: usize) -> bool {
    let (first, width) = subject.char_at(at);
    let Some(second) = (at + width < subject.len()).then(|| subject.char_at(at + width).0) else {
        // A match needs two characters.
        return false;
    };
    if first >= 256 || second >= 256 {
        return true;
    }
    pairs
        .iter()
        .any(|pair| has_bit(&pair.first, first) && has_bit(&pair.second, second))
}

/// What the start of a pattern says about where it can match.
#[derive(Clone, Debug, PartialEq)]
enum Start {
    /// Nothing: every position is tried.
    Anywhere,
    /// The pattern begins with `^`: only the start of the subject can
    /// match or, with the `m` flag, the start of a line.
    Line,
    /// A match begins with one of these characters.
    Chars(Box<Chars>),
}

/// Inclusive ranges of characters.
type Ranges = Vec<(u32, u32)>;

/// A set of characters, as the matcher compares them: with the `i` flag
/// the ranges hold canonical (case-folded) characters and a subject
/// character is folded before it is looked up.
#[derive(Clone, Debug, PartialEq)]
struct Chars {
    /// Inclusive ranges, unsorted.
    ranges: Vec<(u32, u32)>,
    /// The answer for each character below 256, folding included.
    latin1: [bool; 256],
    /// The characters below 256 that are in the set, when there are at
    /// most three: `memchr` finds those faster than the table does.
    few: Option<Vec<u8>>,
    /// How many characters below 256 are in the set.
    count: usize,
}

impl Chars {
    fn new(ranges: Vec<(u32, u32)>, flags: c_int) -> Self {
        let mut latin1 = [false; 256];
        if flags & IGNORE_CASE == 0 {
            for &(low, high) in &ranges {
                if low < 256 {
                    let high = high.min(255);
                    for c in low..=high {
                        latin1[c as usize] = true;
                    }
                }
            }
        } else {
            for c in 0..=255u8 {
                latin1[usize::from(c)] = in_ranges(&ranges, fold_latin1(c, flags));
            }
        }
        let count = latin1.iter().filter(|&&b| b).count();
        let few = (count <= 3).then(|| (0..=255u8).filter(|&c| latin1[usize::from(c)]).collect());
        Self {
            ranges,
            latin1,
            few,
            count,
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

    /// The first position at or after `from` that holds a character of the
    /// set, as the matcher reads characters: in a subject where surrogate
    /// pairs count, a pair is one character and the position is its first
    /// unit.
    fn find(&self, subject: Subject<'_>, from: usize, flags: c_int) -> Option<usize> {
        match subject {
            Subject::Narrow(bytes) => self.find8(bytes, from),
            Subject::Wide { .. } => {
                let mut at = from;
                while at < subject.len() {
                    let (c, width) = subject.char_at(at);
                    if self.contains(c, flags) {
                        return Some(at);
                    }
                    at += width;
                }
                None
            }
        }
    }

    /// The first position at or after `from` that does not hold a
    /// character of the set (or the end of the subject).
    fn skip(&self, subject: Subject<'_>, from: usize, flags: c_int) -> usize {
        match subject {
            Subject::Narrow(bytes) => bytes
                .get(from..)
                .and_then(|rest| rest.iter().position(|&c| !self.latin1[usize::from(c)]))
                .map_or(bytes.len().max(from), |i| from + i),
            Subject::Wide { .. } => {
                let mut at = from;
                while at < subject.len() {
                    let (c, width) = subject.char_at(at);
                    if !self.contains(c, flags) {
                        break;
                    }
                    at += width;
                }
                at
            }
        }
    }
}

fn in_ranges(ranges: &[(u32, u32)], c: u32) -> bool {
    ranges.iter().any(|&(low, high)| low <= c && c <= high)
}

/// [`fold`] for a character below 256, from a table.
fn fold_latin1(c: u8, flags: c_int) -> u32 {
    static TABLES: std::sync::OnceLock<[[u32; 256]; 2]> = std::sync::OnceLock::new();
    if flags & IGNORE_CASE == 0 {
        return u32::from(c);
    }
    let tables = TABLES.get_or_init(|| {
        let mut tables = [[0; 256]; 2];
        for (unicode, table) in tables.iter_mut().enumerate() {
            let flags = IGNORE_CASE | if unicode == 1 { UNICODE } else { 0 };
            for (c, folded) in (0..=255u32).zip(table.iter_mut()) {
                *folded = fold(c, flags);
            }
        }
        tables
    });
    tables[usize::from(flags & UNICODE != 0)][usize::from(c)]
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
fn analyse_start(code: &[u8], flags: c_int) -> Start {
    // Every pattern opens with `save_start 0`.
    if code.first() == Some(&op::SAVE_START) && code.get(2) == Some(&op::LINE_START) {
        return Start::Line;
    }
    first_chars(code).map_or(Start::Anywhere, |ranges| {
        Start::Chars(Box::new(Chars::new(ranges, flags)))
    })
}

/// Reads a compiled pattern (the bytecode after the search loop) into the
/// [`Plan`] its searches follow.
fn analyse(code: &[u8], flags: c_int) -> Plan {
    let Plan { start, run, .. } = analyse_quick(code, flags);
    let start_count = match &start {
        Start::Chars(chars) => chars.count,
        Start::Anywhere | Start::Line => usize::MAX,
    };
    let required = required_chars(code, flags)
        .filter(|chars| chars.count < start_count || chars.count <= 3 && start_count > 3)
        .map(Box::new);
    let pairs = if matches!(start, Start::Chars(_)) && !run {
        second_chars(code, flags)
    } else {
        None
    };
    Plan {
        start,
        required,
        run,
        pairs,
        full: true,
    }
}

/// The part of a [`Plan`] that takes no longer to work out than a search
/// takes: what a pattern used once or twice gets.
fn analyse_quick(code: &[u8], flags: c_int) -> Plan {
    let start = analyse_start(code, flags);
    let run = matches!(start, Start::Chars(_)) && leading_run(code);
    Plan {
        start,
        required: None,
        run,
        pairs: None,
        full: false,
    }
}

/// The size of the instruction at `pc`, if it is one this module knows.
fn op_size(code: &[u8], pc: usize) -> Option<usize> {
    let size = match *code.get(pc)? {
        op::DOT
        | op::ANY
        | op::LINE_START
        | op::LINE_END
        | op::MATCH
        | op::DROP
        | op::WORD_BOUNDARY
        | op::NOT_WORD_BOUNDARY
        | op::PUSH_CHAR_POS
        | op::CHECK_ADVANCE => 1,
        op::CHAR8 | op::SAVE_START | op::SAVE_END | op::BACK_REFERENCE => 2,
        op::BACKWARD_BACK_REFERENCE => 2,
        op::CHAR16 | op::SAVE_RESET => 3,
        op::CHAR32
        | op::GOTO
        | op::SPLIT_GOTO_FIRST
        | op::SPLIT_NEXT_FIRST
        | op::LOOP
        | op::PUSH_I32
        | op::LOOKAHEAD
        | op::NEGATIVE_LOOKAHEAD => 5,
        op::RANGE => 3 + 4 * usize::try_from(u16_at(code, pc + 1)?).ok()?,
        op::RANGE32 => 3 + 8 * usize::try_from(u16_at(code, pc + 1)?).ok()?,
        op::SIMPLE_GREEDY_QUANT => 17,
        _ => return None,
    };
    (pc + size <= code.len()).then_some(size)
}

/// The characters the one-character test at `pc` accepts, before folding,
/// or `None` if it is not such a test or accepts nearly anything.
fn test_chars(code: &[u8], pc: usize) -> Option<Vec<(u32, u32)>> {
    match *code.get(pc)? {
        op::CHAR8 => {
            let c = u32::from(*code.get(pc + 1)?);
            Some(vec![(c, c)])
        }
        op::CHAR16 => {
            let c = u16_at(code, pc + 1)?;
            Some(vec![(c, c)])
        }
        op::CHAR32 => {
            let c = u32_at(code, pc + 1)?;
            Some(vec![(c, c)])
        }
        op::RANGE => {
            let count = usize::try_from(u16_at(code, pc + 1)?).ok()?;
            let mut ranges = Vec::with_capacity(count);
            for i in 0..count {
                let low = u16_at(code, pc + 3 + 4 * i)?;
                let mut high = u16_at(code, pc + 5 + 4 * i)?;
                if i + 1 == count && high == 0xffff {
                    high = u32::MAX;
                }
                ranges.push((low, high));
            }
            Some(ranges)
        }
        op::RANGE32 => {
            let count = usize::try_from(u16_at(code, pc + 1)?).ok()?;
            let mut ranges = Vec::with_capacity(count);
            for i in 0..count {
                ranges.push((u32_at(code, pc + 3 + 8 * i)?, u32_at(code, pc + 7 + 8 * i)?));
            }
            Some(ranges)
        }
        _ => None,
    }
}

/// The most instructions [`required_chars`] reads in a pattern.
const REQUIRED_MAX_OPS: usize = 4000;

/// A set of characters that every match of the pattern contains one of:
/// the rarest of the one-character tests that every way from the start of
/// the pattern to its end passes through (that dominate its `match`).
/// Lookarounds are passed over, as is everything inside a repeat that may
/// run no times.
fn required_chars(code: &[u8], flags: c_int) -> Option<Chars> {
    // The pattern's instructions as far as its own `match`, numbered in
    // depth-first order, and where each can go next. A lookaround's body
    // and a simple repeat's body are not entered: the instruction stands
    // for them.
    const NONE: u32 = u32::MAX;
    let mut node_of = vec![NONE; code.len()];
    let mut pcs: Vec<usize> = Vec::new();
    let mut succ: Vec<[usize; 2]> = Vec::new();
    let mut todo = vec![0usize];
    let mut end = None;
    while let Some(pc) = todo.pop() {
        if *node_of.get(pc)? != NONE {
            continue;
        }
        if pcs.len() >= REQUIRED_MAX_OPS {
            return None;
        }
        let next = pc + op_size(code, pc)?;
        let to = match code[pc] {
            op::MATCH => {
                end = Some(pcs.len());
                [usize::MAX; 2]
            }
            op::GOTO | op::LOOKAHEAD | op::NEGATIVE_LOOKAHEAD => {
                [target(code, pc + 1)?, usize::MAX]
            }
            op::SPLIT_GOTO_FIRST | op::SPLIT_NEXT_FIRST | op::LOOP => [next, target(code, pc + 1)?],
            op::SIMPLE_GREEDY_QUANT => {
                let body = usize::try_from(u32_at(code, pc + 1)?).ok()?;
                [next.checked_add(body)?, usize::MAX]
            }
            // Only a lookbehind's body steps backwards.
            op::BACKWARD_BACK_REFERENCE => return None,
            _ => [next, usize::MAX],
        };
        node_of[pc] = u32::try_from(pcs.len()).ok()?;
        pcs.push(pc);
        succ.push(to);
        todo.extend(to.into_iter().filter(|&to| to != usize::MAX));
    }
    let end = end?;
    let count = pcs.len();
    let succ: Vec<[usize; 2]> = succ
        .iter()
        .map(|to| {
            to.map(|pc| {
                if pc == usize::MAX {
                    usize::MAX
                } else {
                    node_of[pc] as usize
                }
            })
        })
        .collect();

    // Dominators, by Cooper, Harvey and Kennedy's iteration over reverse
    // postorder.
    let mut postorder = Vec::with_capacity(count);
    let mut number = vec![usize::MAX; count];
    let mut stack = vec![(0usize, 0usize)];
    let mut visited = vec![false; count];
    visited[0] = true;
    while let Some(&mut (node, ref mut edge)) = stack.last_mut() {
        if let Some(&next) = succ[node].get(*edge) {
            *edge += 1;
            if next != usize::MAX && !visited[next] {
                visited[next] = true;
                stack.push((next, 0));
            }
        } else {
            number[node] = postorder.len();
            postorder.push(node);
            stack.pop();
        }
    }
    let mut preds = vec![Vec::new(); count];
    for (node, to) in succ.iter().enumerate() {
        for &next in to.iter().filter(|&&next| next != usize::MAX) {
            preds[next].push(node);
        }
    }
    let mut idom = vec![usize::MAX; count];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &node in postorder.iter().rev().skip(1) {
            let mut new = usize::MAX;
            for &pred in &preds[node] {
                if idom[pred] == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    pred
                } else {
                    let (mut a, mut b) = (new, pred);
                    while a != b {
                        while number[a] < number[b] {
                            a = idom[a];
                        }
                        while number[b] < number[a] {
                            b = idom[b];
                        }
                    }
                    a
                };
            }
            if new != idom[node] {
                idom[node] = new;
                changed = true;
            }
        }
    }

    // The one-character tests on the way from `match` up its dominators.
    let mut best: Option<(u32, Vec<(u32, u32)>)> = None;
    let mut node = end;
    while node != 0 {
        node = idom[node];
        if node == usize::MAX {
            return None;
        }
        let pc = pcs[node];
        let ranges = match code[pc] {
            op::SIMPLE_GREEDY_QUANT if u32_at(code, pc + 5)? > 0 => {
                // The first one-character test of the repeated part. Only
                // assertions can come before it there.
                let mut at = pc + 17;
                loop {
                    match *code.get(at)? {
                        op::LINE_START
                        | op::LINE_END
                        | op::WORD_BOUNDARY
                        | op::NOT_WORD_BOUNDARY => {
                            at += 1;
                        }
                        _ => break test_chars(code, at),
                    }
                }
            }
            _ => test_chars(code, pc),
        };
        if let Some(ranges) = ranges {
            let score = commonness(&ranges, flags);
            if best.as_ref().is_none_or(|(best, _)| score < *best) {
                best = Some((score, ranges));
            }
        }
    }
    best.map(|(_, ranges)| Chars::new(ranges, flags))
}

/// How often a set of characters can be expected in page text, roughly:
/// [`required_chars`] picks the set with the lowest. Spaces and lower-case
/// letters are everywhere, an `@` is rare.
fn commonness(ranges: &[(u32, u32)], flags: c_int) -> u32 {
    let weight = |c: u8| -> u32 {
        match c {
            b' ' => 100,
            b'a'..=b'z' => 20,
            b'A'..=b'Z' | b'0'..=b'9' => 5,
            b'.' | b',' | b'-' | b'/' | b':' | b';' | b'(' | b')' | b'"' | b'\'' | b'=' | b'_'
            | b'\n' | b'\t' => 5,
            _ => 1,
        }
    };
    let mut seen = [false; 256];
    if flags & IGNORE_CASE != 0 {
        for c in 0..=255u8 {
            seen[usize::from(c)] = in_ranges(ranges, fold_latin1(c, flags));
        }
    } else {
        for &(low, high) in ranges {
            for c in low.min(256)..=high.min(255) {
                seen[c as usize] = true;
            }
        }
    }
    (0..=255u8)
        .filter(|&c| seen[usize::from(c)])
        .map(weight)
        .sum()
}

/// Whether the pattern is an unbounded greedy repeat, at least once, of a
/// single one-character test, straight after `save_start 0`, with no
/// group around it: the condition for [`Plan::run`].
fn leading_run(code: &[u8]) -> bool {
    let quant = 2;
    if code.first() != Some(&op::SAVE_START) || code.get(quant) != Some(&op::SIMPLE_GREEDY_QUANT) {
        return false;
    }
    let (Some(body), Some(min), Some(max)) = (
        u32_at(code, quant + 1),
        u32_at(code, quant + 5),
        u32_at(code, quant + 9),
    ) else {
        return false;
    };
    let test = quant + 17;
    let Some(size) = op_size(code, test) else {
        return false;
    };
    min >= 1
        && max == i32::MAX as u32
        && test_chars(code, test).is_some()
        && usize::try_from(body).is_ok_and(|body| body == size + 1)
        && code.get(test + size) == Some(&op::MATCH)
}

fn first_chars(code: &[u8]) -> Option<Vec<(u32, u32)>> {
    let tests = first_tests(code, 0)?;
    Some(tests.into_iter().flat_map(|(_, ranges)| ranges).collect())
}

/// The one-character tests a match can begin with, from `from`, each with
/// where it is and what it accepts. `None` when the pattern can begin
/// with something else: see [`first_chars`].
fn first_tests(code: &[u8], from: usize) -> Option<Vec<(usize, Ranges)>> {
    let mut tests = Vec::new();
    let mut seen = vec![false; code.len()];
    let mut todo = vec![from];
    while let Some(mut pc) = todo.pop() {
        loop {
            let opcode = *code.get(pc)?;
            if std::mem::replace(seen.get_mut(pc)?, true) {
                break;
            }
            match opcode {
                op::CHAR8 | op::CHAR16 | op::CHAR32 | op::RANGE | op::RANGE32 => {
                    tests.push((pc, test_chars(code, pc)?));
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
    Some(tests)
}

/// What the second character of a match can be, given the first: for
/// each one-character test a match can begin with, the characters below
/// 256 it accepts and those the pattern can test for next. `None` when
/// that is not known for every such test.
fn second_chars(code: &[u8], flags: c_int) -> Option<Vec<Pair>> {
    let tests = first_tests(code, 0)?;
    if tests.len() > PAIRS_MAX {
        return None;
    }
    let bits = |ranges: &[(u32, u32)]| -> [u64; 4] {
        let chars = Chars::new(ranges.to_vec(), flags);
        let mut bits = [0u64; 4];
        for (c, &member) in chars.latin1.iter().enumerate() {
            if member {
                bits[c / 64] |= 1 << (c % 64);
            }
        }
        bits
    };
    let mut pairs = Vec::with_capacity(tests.len());
    for (pc, first) in tests {
        let next = first_tests(code, pc + op_size(code, pc)?)?;
        let second: Vec<(u32, u32)> = next.into_iter().flat_map(|(_, ranges)| ranges).collect();
        pairs.push(Pair {
            first: bits(&first),
            second: bits(&second),
        });
    }
    Some(pairs)
}

/// How many patterns keep their [`Plan`], in sets of [`CACHE_WAYS`]. A
/// page runs a handful of patterns in its hot loops and the rest once or
/// twice.
const CACHE_SLOTS: usize = 64;
const CACHE_WAYS: usize = 4;

/// How many searches a pattern runs with [`analyse_quick`]'s plan before
/// it gets [`analyse`]'s. A page runs most of its patterns a few times and
/// a few of them thousands of times.
const FULL_PLAN_AFTER: u32 = 4;

struct Cached {
    code: Box<[u8]>,
    flags: c_int,
    plan: Rc<Plan>,
    searches: u32,
}

thread_local! {
    static CACHE: RefCell<Vec<Option<Cached>>> =
        RefCell::new((0..CACHE_SLOTS).map(|_| None).collect());
}

/// The [`Plan`] of a pattern, worked out the first time it is searched
/// with, and in full once it has been searched with a few times. A set of
/// slots is found by where the bytecode is, and a slot is
/// checked by what the bytecode is, so a pattern freed and another put at
/// its address is noticed. A new plan goes in the set's last slot and the
/// others move up, so the set keeps the patterns used last.
fn plan_of(code: &[u8], flags: c_int) -> Rc<Plan> {
    let hash = (code.as_ptr().addr() ^ code.len()).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let set = (hash >> 40) % (CACHE_SLOTS / CACHE_WAYS) * CACHE_WAYS;
    CACHE.with_borrow_mut(|cache| {
        let ways = &mut cache[set..set + CACHE_WAYS];
        for way in ways.iter_mut().flatten() {
            if way.flags == flags && *way.code == *code {
                way.searches = way.searches.saturating_add(1);
                if way.searches == FULL_PLAN_AFTER {
                    way.plan = Rc::new(analyse(code, flags));
                }
                return Rc::clone(&way.plan);
            }
        }
        let plan = Rc::new(analyse_quick(code, flags));
        ways.rotate_left(1);
        ways[CACHE_WAYS - 1] = Some(Cached {
            code: code.into(),
            flags,
            plan: Rc::clone(&plan),
            searches: 1,
        });
        plan
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
/// `plan` is null or where the RegExp being searched with keeps its
/// pattern's plan, null until this stores one: a full [`Plan`], as an
/// `Rc` turned into a pointer, which [`free_plan`] frees. A RegExp's plan
/// is then found without [`plan_of`]'s lookup, which compares the whole
/// bytecode. `body` is the pattern's bytecode after that loop. `cbuf_type` is 0 for
/// a Latin-1 subject, 1 for UTF-16 and 2 for UTF-16 where surrogate pairs
/// count as one character. The search starts at code unit `cindex` of
/// `clen`. Returns what `lre_exec` returns: 1 for a match (the engine has
/// then filled in the captures), 0 for none, negative for an error.
///
/// # Safety
///
/// Only the engine calls this, with `search` its own state for
/// `lre_search_try`, `plan` null or valid and holding null or what this
/// stored there for the same bytecode, `body` valid for `body_len` bytes
/// and `cbuf` valid for `clen` code units, aligned for them.
unsafe extern "C" fn search(
    search: *mut c_void,
    plan: *mut *mut c_void,
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
    #[cfg(test)]
    tests::LAST_CODE.with_borrow_mut(|last| *last = (code.to_vec(), flags));
    let try_at = |at: usize| -> c_int {
        // SAFETY: `search` is the engine's, and `at` is within the subject
        // or at its end. It fits a `c_int` because `clen` did.
        unsafe { lre_search_try(search, at as c_int) }
    };
    // SAFETY: the caller's contract. A stored plan stays alive for the
    // search: only finalizing or recompiling the RegExp frees it, and no
    // script runs while it searches.
    let kept = unsafe { plan.as_ref() }.map(|slot| slot.cast::<Plan>().cast_const());
    if let Some(kept) = kept.filter(|kept| !kept.is_null()) {
        // SAFETY: as above.
        return run(unsafe { &*kept }, subject, from, flags, try_at);
    }
    let found = plan_of(code, flags);
    if found.full && !plan.is_null() {
        // SAFETY: as above; the slot is null.
        unsafe { *plan = Rc::into_raw(Rc::clone(&found)).cast_mut().cast() };
    }
    run(&found, subject, from, flags, try_at)
}

/// Frees a plan [`search`] stored with a RegExp.
///
/// # Safety
///
/// Only the engine calls this, once for each plan stored, on the thread
/// that stored it.
unsafe extern "C" fn free_plan(plan: *mut c_void) {
    // SAFETY: `plan` came from `Rc::into_raw` in `search`.
    drop(unsafe { Rc::from_raw(plan.cast_const().cast::<Plan>()) });
}

/// The search loop: tries the pattern at each position `plan` allows, from
/// `from` to the end of the subject, until `try_at` reports something other
/// than "no match here".
fn run(
    plan: &Plan,
    subject: Subject<'_>,
    from: usize,
    flags: c_int,
    mut try_at: impl FnMut(usize) -> c_int,
) -> c_int {
    let len = subject.len();
    if from > len {
        return 0;
    }
    if let Some(required) = &plan.required {
        // Where the next character of the required set is. A match that
        // starts at `at` needs one at or after `at`.
        let Some(mut next) = required.find(subject, from, flags) else {
            return 0;
        };
        let mut gated = |at: usize| -> c_int {
            if at > next {
                match required.find(subject, at, flags) {
                    Some(found) => next = found,
                    None => return NO_MORE,
                }
            }
            try_at(at)
        };
        let found = run_from(plan, subject, from, flags, &mut gated);
        return if found == NO_MORE { 0 } else { found };
    }
    run_from(plan, subject, from, flags, &mut try_at)
}

/// What a `try_at` of [`run_from`] returns to end the search with no
/// match. `lre_search_try` never returns it: it returns 1, 0 or a
/// negative `LRE_RET_*`.
const NO_MORE: c_int = c_int::MIN;

fn run_from(
    plan: &Plan,
    subject: Subject<'_>,
    from: usize,
    flags: c_int,
    try_at: &mut impl FnMut(usize) -> c_int,
) -> c_int {
    let len = subject.len();
    match &plan.start {
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
            while let Some(hit) = chars.find(subject, at, flags) {
                if let Some(pairs) = &plan.pairs {
                    if !pairs_allow(pairs, subject, hit) {
                        at = hit + subject.char_at(hit).1;
                        continue;
                    }
                }
                let found = try_at(hit);
                if found != 0 {
                    return found;
                }
                at = hit + subject.char_at(hit).1;
                if plan.run {
                    at = chars.skip(subject, at, flags);
                }
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// The bytecode and flags of the last search on this thread.
        pub(super) static LAST_CODE: RefCell<(Vec<u8>, c_int)> = const { RefCell::new((Vec::new(), 0)) };
    }

    /// The bytecode the engine compiles `/source/flags` to, as the hook
    /// sees it, and the flags.
    fn compiled(source: &str, flags: &str) -> (Vec<u8>, c_int) {
        let js = crate::JsRuntime::new(crate::RuntimeOptions::default()).expect("runtime");
        let script = format!("new RegExp({source:?}, {flags:?}).exec('')");
        js.eval_json(&script).expect("compiles");
        LAST_CODE.with_borrow(Clone::clone)
    }

    fn required_of(source: &str, flags: &str) -> Option<Vec<u8>> {
        let (code, flags) = compiled(source, flags);
        analyse(&code, flags).required.map(|chars| {
            (0..=255u8)
                .filter(|&c| chars.latin1[usize::from(c)])
                .collect()
        })
    }

    #[test]
    fn a_character_every_match_needs_is_required() {
        let email = "[a-zA-Z0-9.!#$%&'*+\\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+";
        assert_eq!(required_of(email, "g").as_deref(), Some(&b"@"[..]));
        assert_eq!(required_of("\\w+@\\w+", "").as_deref(), Some(&b"@"[..]));
        assert_eq!(required_of("(?:a|c)bx?", "").as_deref(), Some(&b"b"[..]));
        // The same character on two branches is two tests, neither of
        // which every match passes.
        assert_eq!(required_of("(?:ab|cb)x?", ""), None);
        assert_eq!(required_of("x*[ab]", "").as_deref(), Some(&b"ab"[..]));
        assert_eq!(required_of(".q(?=@)", "").as_deref(), Some(&b"q"[..]));
        assert_eq!(
            required_of("\\s+[=:]\\s*\\d", "i").as_deref(),
            Some(&b":="[..])
        );
        // Nothing every match needs, or nothing rarer than the start.
        assert_eq!(required_of("a?b?", ""), None);
        assert_eq!(required_of("@\\w+", ""), None);
        assert_eq!(required_of("(?:a|b)+", ""), None);
        assert_eq!(required_of("(?<=@).x", "").as_deref(), Some(&b"x"[..]));
        assert_eq!(required_of("(\\w)\\1@", "").as_deref(), Some(&b"@"[..]));
        // A required character the start already looks for adds nothing.
        assert_eq!(required_of("q(?=@)", ""), None);
    }

    #[test]
    fn an_alternation_is_tried_only_where_a_branch_can_begin() {
        let (code, flags) = compiled("\\b(?:casino|viagra|loans?)\\b", "gi");
        let plan = analyse(&code, flags);
        assert!(plan.pairs.is_some());
        // "cl" and a final "c" cannot begin a branch; "ca", "vi" and "lo"
        // can.
        let subject = Subject::Narrow(b"cat vim Casino lot LOAN clc");
        assert_eq!(tried_by(&plan, subject, 0, flags), [0, 4, 8, 15, 19]);
        // A pattern that can match one character has no pairs.
        for source in ["ab?", "a$", "a\\b", "a|bc", "a.", "(a)\\1"] {
            let (code, flags) = compiled(source, "");
            assert_eq!(analyse(&code, flags).pairs, None, "{source}");
        }
    }

    #[test]
    fn a_leading_unbounded_repeat_skips_its_run() {
        let run_of = |source: &str| {
            let (code, flags) = compiled(source, "");
            analyse(&code, flags).run
        };
        assert!(run_of("[a-z]+@"));
        assert!(run_of("\\w+@\\w+"));
        assert!(!run_of("[a-z]*@"));
        assert!(!run_of("[a-z]{1,9}@"));
        assert!(!run_of("([a-z]+)@\\1"));
        assert!(!run_of("x[a-z]+@"));
    }

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
        assert_eq!(chars_of(&analyse_start(&code, 0)), b"f");
    }

    #[test]
    fn an_alternation_starts_with_either_branch() {
        // split_next_first +7; char8 a; goto +2; char8 b; match
        let mut code = vec![op::SPLIT_NEXT_FIRST, 7, 0, 0, 0];
        code.extend_from_slice(&[op::CHAR8, b'a', op::GOTO, 2, 0, 0, 0]);
        code.extend_from_slice(&[op::CHAR8, b'b', 11]);
        assert_eq!(chars_of(&analyse_start(&body(&code), 0)), b"ab");
    }

    #[test]
    fn a_pattern_that_can_be_empty_starts_anywhere() {
        // split_next_first +2; char8 a; match
        let code = body(&[op::SPLIT_NEXT_FIRST, 2, 0, 0, 0, op::CHAR8, b'a', 11]);
        assert_eq!(analyse_start(&code, 0), Start::Anywhere);
    }

    #[test]
    fn a_dot_starts_anywhere() {
        assert_eq!(analyse_start(&body(&[4, 11]), 0), Start::Anywhere);
    }

    #[test]
    fn a_caret_starts_at_a_line() {
        assert_eq!(
            analyse_start(&body(&[op::LINE_START, op::CHAR8, b'a', 11]), 0),
            Start::Line
        );
    }

    #[test]
    fn truncated_bytecode_starts_anywhere() {
        assert_eq!(
            analyse_start(&body(&[op::CHAR16, b'a']), 0),
            Start::Anywhere
        );
        assert_eq!(
            analyse_start(&body(&[op::GOTO, 0xff, 0xff, 0xff, 0x7f]), 0),
            Start::Anywhere
        );
    }

    #[test]
    fn the_last_range_can_be_open_ended() {
        // range with one pair, 0x100 to "infinity"
        let code = body(&[op::RANGE, 1, 0, 0x00, 0x01, 0xff, 0xff, 11]);
        let Start::Chars(chars) = analyse_start(&code, 0) else {
            panic!("expected a character set");
        };
        assert!(chars.contains(0x1f600, 0));
        assert!(!chars.contains(u32::from(b'a'), 0));
        assert_eq!(chars.few.as_deref(), Some(&[][..]));
    }

    fn only(start: &Start) -> Plan {
        Plan {
            start: start.clone(),
            required: None,
            run: false,
            pairs: None,
            full: false,
        }
    }

    fn positions(start: &Start, subject: Subject<'_>, from: usize, flags: c_int) -> Vec<usize> {
        tried_by(&only(start), subject, from, flags)
    }

    fn tried_by(plan: &Plan, subject: Subject<'_>, from: usize, flags: c_int) -> Vec<usize> {
        let mut tried = Vec::new();
        run(plan, subject, from, flags, |at| {
            tried.push(at);
            0
        });
        tried
    }

    #[test]
    fn a_character_set_is_tried_only_where_it_occurs() {
        let start = analyse_start(&body(&[op::CHAR8, b'a', 11]), 0);
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
        let found = run(
            &only(&Start::Anywhere),
            Subject::Narrow(b"abc"),
            0,
            0,
            |at| {
                calls += 1;
                c_int::from(at == 1)
            },
        );
        assert_eq!((found, calls), (1, 2));
        let failed = run(
            &only(&Start::Anywhere),
            Subject::Narrow(b"abc"),
            0,
            0,
            |_| -2,
        );
        assert_eq!(failed, -2);
    }
}
