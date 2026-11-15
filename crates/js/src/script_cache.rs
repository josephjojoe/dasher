//! Compiled scripts kept across pages: [`ScriptCache`].
//!
//! Pages meet the same bundles again and again: a site's own on every one
//! of its pages, and frameworks, tag managers and analytics on many sites.
//! Compiling a script is the larger part of what running it costs the first
//! time, so a long-running process keeps what compiling gave, keyed by a
//! hash of the script's text, and later pages read that back instead of
//! parsing the text again.
//!
//! # What an entry holds
//!
//! Functions are compiled on their first call, so compiling a script
//! compiles only
//! its top level and the functions it is likely to call at once. The rest
//! are kept as records of where they are in the text. An entry is the
//! script's text and its compiled code in QuickJS's bytecode format
//! (`JS_WRITE_OBJ_SCRIPT`, compressed with zstd), with the functions that
//! are not compiled written as those records and every function's text as
//! a place in the script's.
//!
//! A script compiled from its text is written out at once, before any of
//! it runs. The page then notes which of its functions it compiled on
//! their first call, and the entry keeps that list. The next page to use
//! the entry reads it back, compiles those functions on their first call
//! too, and when it is dropped compiles the script again, in a runtime of
//! its own, with every function the two pages ran compiled with it
//! ("hints"); the entry is replaced. From the third page on, the functions
//! pages run are compiled already and the rest still wait for their first
//! call. A script seen once costs only the writing, and one seen twice
//! pays for compiling it again once. The hints grow when pages run
//! functions the entry lacks, at most [`MAX_REBUILDS`] times per entry.
//! Inline scripts are only kept once they come back (most belong to one
//! page), and scripts under [`MIN_SCRIPT_LEN`] bytes not at all.
//!
//! Every function read back shares one copy of the script's text, and that
//! copy is the cache's own: pages point into it rather than copying it into
//! their heaps (`JSScriptSource`), and it is freed when the last page using
//! it and the cache have both let it go. Classic scripts and modules are
//! both kept; the key says which, and whether functions are compiled on
//! first call at all. The file name (a module's name) is not part of it:
//! the same text at another URL finds the entry, and its functions get the
//! URL they were loaded from.
//!
//! # What pages can see
//!
//! Nothing a page does reaches another page. An entry is made from the
//! script's text alone: written right after it is compiled, before it
//! runs, or compiled again in a runtime with nothing in it, never from the
//! objects of a page that ran the script. The hints only say which
//! functions to compile now rather than later, which a script cannot tell
//! apart. A page reads the entry into objects of its own. The key is the
//! SHA-256 of the text, so a page only finds the entry of a script whose
//! text it fetched itself.
//!
//! What a page can do is time itself: a script whose entry exists runs
//! sooner, so a page can tell, roughly, whether a script it already holds
//! the whole text of was run by an earlier page in the process. And a page
//! that loads many large scripts of its own evicts other entries, which
//! costs later pages time but changes nothing they compute.
//!
//! # Bounds
//!
//! The entries in memory are bounded by [`ScriptCacheOptions::memory_bytes`]
//! (least recently used go first). They are counted whole, text included,
//! and the cache is part of the process's idle memory once it has filled.
//! With [`ScriptCacheOptions::dir`] set, entries are also written there, so
//! that a new process starts warm. Files carry the engine build they were
//! written by and a checksum, and one that does not match is ignored.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_char, c_void};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use ring::digest::{SHA256, digest};
use rquickjs::{Ctx, qjs};

/// Default for [`ScriptCacheOptions::memory_bytes`]: 32 MiB.
pub const DEFAULT_SCRIPT_CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Default for [`ScriptCacheOptions::disk_bytes`]: 256 MiB.
pub const DEFAULT_SCRIPT_CACHE_DISK_BYTES: u64 = 256 * 1024 * 1024;

/// Scripts shorter than this are compiled every time: they compile in
/// microseconds, and an entry would cost more than it saves.
const MIN_SCRIPT_LEN: usize = 1024;
/// How often an entry is compiled again with more hints.
const MAX_REBUILDS: u8 = 3;
/// An entry is compiled again when a page ran at least this share (1/n)
/// more functions than its hints name, or any when it has none.
const REBUILD_SHARE: usize = 8;
/// How many inline scripts seen once are remembered (see `seen`).
const MAX_SEEN: usize = 4096;

/// The engine build, from `build.rs`.
const ENGINE_ID: &str = env!("DASHER_ENGINE_ID");
const FILE_MAGIC: &[u8; 8] = b"dshrjsc2";
/// zstd's fastest level: written once per entry, it still halves the code.
const CODE_LEVEL: i32 = 1;

/// How a [`ScriptCache`] is bounded, and where it keeps files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptCacheOptions {
    /// Bytes of compiled scripts kept in memory, text included. `0` keeps
    /// none in memory (with [`dir`](Self::dir) set, scripts are still read
    /// from and written to disk).
    pub memory_bytes: usize,
    /// A directory to keep compiled scripts in as well, such as `/tmp` on
    /// AWS Lambda, so that a new process finds the scripts an earlier one
    /// compiled. It is created if missing.
    pub dir: Option<PathBuf>,
    /// Bytes of files kept in [`dir`](Self::dir); the oldest are deleted
    /// past it.
    pub disk_bytes: u64,
}

impl Default for ScriptCacheOptions {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_SCRIPT_CACHE_BYTES,
            dir: None,
            disk_bytes: DEFAULT_SCRIPT_CACHE_DISK_BYTES,
        }
    }
}

/// Counts kept by a [`ScriptCache`] since it was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptCacheStats {
    /// Scripts found compiled.
    pub hits: u64,
    /// Of those, the ones read from disk.
    pub disk_hits: u64,
    /// Scripts compiled from their text (long enough to be cached).
    pub misses: u64,
    /// Bytes of script text found compiled.
    pub hit_bytes: u64,
    /// Bytes of script text compiled.
    pub miss_bytes: u64,
    /// Entries written: when a script is first compiled, and again with
    /// the functions pages ran compiled in.
    pub builds: u64,
    /// Functions of scripts read from the cache that pages still compiled
    /// on their first call, because the entry did not have them compiled.
    pub compiled_late: u64,
    /// Entries dropped from memory to make room.
    pub evictions: u64,
    /// Entries in memory now.
    pub entries: usize,
    /// Bytes they hold.
    pub bytes: usize,
}

/// Compiled scripts shared by the pages of a process, keyed by their
/// text's hash (see the module docs).
///
/// Cheap to clone: clones share the cache. Give one to every page through
/// [`RuntimeOptions::script_cache`](crate::RuntimeOptions::script_cache).
#[derive(Clone)]
pub struct ScriptCache {
    inner: Arc<Inner>,
}

impl fmt::Debug for ScriptCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptCache")
            .field("options", &self.inner.options)
            .field("stats", &self.stats())
            .finish()
    }
}

struct Inner {
    options: ScriptCacheOptions,
    state: Mutex<State>,
    disk: Option<Disk>,
}

#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    clock: u64,
    /// Inline scripts seen once. Most inline scripts belong to one page
    /// (they carry its data), so one is only cached when it comes back.
    seen: HashSet<Key>,
    stats: ScriptCacheStats,
}

/// SHA-256 of the engine build, the kind of script and its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key([u8; 32]);

impl Key {
    fn new(kind: Kind, lazy: bool, text: &[u8]) -> Self {
        let mut input = Vec::with_capacity(ENGINE_ID.len() + 16 + text.len());
        input.extend_from_slice(ENGINE_ID.as_bytes());
        // What changes what the text compiles to: a classic script (always
        // sloppy at its start; a "use strict" is in the text) or a module,
        // and whether functions wait for their first call
        // (`DASHER_EAGER_JS` turns that off).
        input.extend_from_slice(match kind {
            Kind::Classic => b"\0classic",
            Kind::Module => b"\0module\0",
        });
        input.extend_from_slice(if lazy { b"\0lazy\0" } else { b"\0eager" });
        input.extend_from_slice(text);
        let mut key = [0; 32];
        key.copy_from_slice(digest(&SHA256, &input).as_ref());
        Self(key)
    }

    fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// A script's text, ending in the `'\0'` QuickJS wants, shared with the
/// functions compiled from it.
pub(crate) struct Text {
    bytes: Box<[u8]>,
}

impl Text {
    fn new(text: &[u8]) -> Arc<Self> {
        let mut bytes = Vec::with_capacity(text.len() + 1);
        bytes.extend_from_slice(text);
        bytes.push(0);
        Arc::new(Self {
            bytes: bytes.into_boxed_slice(),
        })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.bytes.len() - 1]
    }

    fn len(&self) -> usize {
        self.bytes.len() - 1
    }
}

#[derive(Clone)]
struct Entry {
    text: Arc<Text>,
    /// The compiled script, compressed (zstd): bytecode compresses to
    /// about half, and a page reads it once. The text is not compressed,
    /// since the pages using the script share it as it is.
    code: Arc<[u8]>,
    /// Bytes of `code` uncompressed.
    code_len: usize,
    /// Ascending offsets of the functions compiled with the script.
    hints: Arc<[u32]>,
    /// Ascending offsets of functions pages compiled on their first call,
    /// for the next time the entry is made.
    wanted: Arc<[u32]>,
    rebuilds: u8,
    used: u64,
}

impl Entry {
    fn new(text: Arc<Text>, code: &[u8], hints: Vec<u32>, rebuilds: u8) -> Option<Self> {
        Some(Self {
            text,
            code: zstd::bulk::compress(code, CODE_LEVEL).ok()?.into(),
            code_len: code.len(),
            hints: hints.into(),
            wanted: Arc::new([]),
            rebuilds,
            used: 0,
        })
    }

    /// The compiled script, for `JS_ReadObject`.
    fn code(&self) -> Option<Vec<u8>> {
        zstd::bulk::decompress(&self.code, self.code_len)
            .ok()
            .filter(|code| code.len() == self.code_len)
    }

    fn size(&self) -> usize {
        // the text, the code, the offsets and the map's bookkeeping
        self.text.bytes.len() + self.code.len() + (self.hints.len() + self.wanted.len()) * 4 + 128
    }
}

impl ScriptCache {
    /// An empty cache.
    pub fn new(options: ScriptCacheOptions) -> Self {
        let disk = options
            .dir
            .clone()
            .map(|dir| Disk::new(dir, options.disk_bytes));
        Self {
            inner: Arc::new(Inner {
                options,
                state: Mutex::default(),
                disk,
            }),
        }
    }

    /// How it is bounded.
    pub fn options(&self) -> &ScriptCacheOptions {
        &self.inner.options
    }

    /// What it has done so far.
    pub fn stats(&self) -> ScriptCacheStats {
        let state = self.state();
        ScriptCacheStats {
            entries: state.entries.len(),
            bytes: state.bytes,
            ..state.stats
        }
    }

    /// Drops every entry held in memory (files on disk stay).
    pub fn clear(&self) {
        let mut state = self.state();
        state.entries.clear();
        state.bytes = 0;
        state.seen.clear();
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The entry for `key`, from memory or disk.
    fn lookup(&self, key: &Key, len: usize) -> Option<Entry> {
        {
            let mut state = self.state();
            state.clock += 1;
            let now = state.clock;
            if let Some(entry) = state.entries.get_mut(key) {
                entry.used = now;
                let entry = entry.clone();
                state.stats.hits += 1;
                state.stats.hit_bytes += len as u64;
                return Some(entry);
            }
        }
        let entry = self.inner.disk.as_ref()?.load(key)?;
        let mut state = self.state();
        state.stats.hits += 1;
        state.stats.disk_hits += 1;
        state.stats.hit_bytes += len as u64;
        self.insert_locked(&mut state, *key, entry.clone());
        Some(entry)
    }

    /// Counts a miss and says whether to make an entry for it.
    fn note_miss(&self, key: &Key, len: usize, inline: bool) -> bool {
        let mut state = self.state();
        state.stats.misses += 1;
        state.stats.miss_bytes += len as u64;
        if !inline || state.seen.remove(key) {
            return true;
        }
        if state.seen.len() >= MAX_SEEN {
            state.seen.clear();
        }
        state.seen.insert(*key);
        false
    }

    fn insert(&self, key: Key, entry: Entry) {
        if let Some(disk) = &self.inner.disk {
            disk.store(&key, &entry);
        }
        let mut state = self.state();
        state.stats.builds += 1;
        self.insert_locked(&mut state, key, entry);
    }

    /// Notes functions a page compiled on their first call, for when the
    /// entry is next made.
    fn want(&self, key: &Key, ran: &[u32]) {
        let updated = {
            let mut state = self.state();
            let state = &mut *state;
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            let before = entry.size();
            entry.wanted = merge(&entry.wanted, ran).into();
            state.bytes = state.bytes - before + entry.size();
            entry.clone()
        };
        if let Some(disk) = &self.inner.disk {
            disk.store(key, &updated);
        }
    }

    fn insert_locked(&self, state: &mut State, key: Key, mut entry: Entry) {
        let limit = self.inner.options.memory_bytes;
        if let Some(old) = state.entries.remove(&key) {
            state.bytes -= old.size();
        }
        let size = entry.size();
        if size > limit {
            return;
        }
        while state.bytes + size > limit {
            let Some((&oldest, _)) = state.entries.iter().min_by_key(|(_, e)| e.used) else {
                break;
            };
            if let Some(old) = state.entries.remove(&oldest) {
                state.bytes -= old.size();
                state.stats.evictions += 1;
            }
        }
        state.clock += 1;
        entry.used = state.clock;
        state.bytes += size;
        state.entries.insert(key, entry);
    }
}

// ---------------------------------------------------------------------
// The engine's side (see `JSScriptSource` in quickjs.h).

#[repr(C)]
struct JSScriptSource {
    buf: *const c_char,
    len: usize,
    filename: *const c_char,
    tag: u64,
    retain: Option<unsafe extern "C" fn(*mut c_void)>,
    release: Option<unsafe extern "C" fn(*mut c_void)>,
    opaque: *mut c_void,
    hints: *const u32,
    hint_count: usize,
}

type LazyCompileHook = unsafe extern "C" fn(*mut c_void, u64, u32);

unsafe extern "C" {
    fn JS_SetScriptSource(rt: *mut qjs::JSRuntime, src: *const JSScriptSource);
    fn JS_SetLazyCompileHook(
        rt: *mut qjs::JSRuntime,
        hook: Option<LazyCompileHook>,
        opaque: *mut c_void,
    );
    fn JS_SetLazyFunctions(rt: *mut qjs::JSRuntime, enable: bool);
}

const JS_WRITE_OBJ_SCRIPT: i32 = 1 << 6;
const JS_READ_OBJ_SCRIPT: i32 = 1 << 6;

unsafe extern "C" fn retain_text(opaque: *mut c_void) {
    // SAFETY: `opaque` is `Arc::as_ptr` of a `Text` that the caller of
    // `JS_SetScriptSource` keeps alive for the call that retains it.
    unsafe { Arc::increment_strong_count(opaque.cast::<Text>().cast_const()) };
}

unsafe extern "C" fn release_text(opaque: *mut c_void) {
    // SAFETY: balances a `retain_text`.
    unsafe { Arc::decrement_strong_count(opaque.cast::<Text>().cast_const()) };
}

impl JSScriptSource {
    fn new(text: &Arc<Text>, filename: *const c_char, tag: u64, hints: &[u32]) -> Self {
        Self {
            buf: text.bytes.as_ptr().cast(),
            len: text.len(),
            filename,
            tag,
            retain: Some(retain_text),
            release: Some(release_text),
            opaque: Arc::as_ptr(text).cast_mut().cast(),
            hints: hints.as_ptr(),
            hint_count: hints.len(),
        }
    }
}

/// Sets `src` on the runtime for the length of `f`.
fn with_source<R>(rt: *mut qjs::JSRuntime, src: &JSScriptSource, f: impl FnOnce() -> R) -> R {
    // SAFETY: `src` outlives the calls `f` makes, and is unset after.
    unsafe { JS_SetScriptSource(rt, src) };
    let result = f();
    // SAFETY: as above.
    unsafe { JS_SetScriptSource(rt, std::ptr::null()) };
    result
}

// ---------------------------------------------------------------------
// A page's side.

/// The scripts a page took from, or compiled for, the cache, and the
/// functions of each it compiled on their first call.
pub(crate) struct PageScripts {
    cache: ScriptCache,
    lazy: bool,
    used: RefCell<Vec<Used>>,
}

struct Used {
    key: Key,
    kind: Kind,
    how: How,
    /// Offsets of the functions compiled on their first call.
    ran: Vec<u32>,
}

/// How a page came by a script.
enum How {
    /// Compiled from its text; no entry made.
    Compiled,
    /// Compiled from its text, and an entry made from that.
    Created,
    /// Read from this entry.
    Read(Entry),
}

/// What a script is compiled as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Classic,
    Module,
}

impl Kind {
    fn eval_flags(self) -> i32 {
        let kind = match self {
            Self::Classic => qjs::JS_EVAL_TYPE_GLOBAL,
            Self::Module => qjs::JS_EVAL_TYPE_MODULE,
        };
        (kind | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32
    }
}

/// Where a script came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// From a URL, or given by the host.
    External,
    /// From the page's own markup.
    Inline,
}

impl PageScripts {
    pub(crate) fn new(cache: ScriptCache, lazy: bool) -> Rc<Self> {
        Rc::new(Self {
            cache,
            lazy,
            used: RefCell::default(),
        })
    }

    /// Has the engine tell this page which functions it compiles on their
    /// first call. `self` must outlive the hook: unset it with
    /// [`detach`](Self::detach).
    pub(crate) fn attach(self: &Rc<Self>, ctx: &Ctx<'_>) {
        // SAFETY: the context is live and locked. The hook's opaque pointer
        // is this `PageScripts`, which the runtime's owner keeps alive
        // until it calls `detach`.
        unsafe {
            let rt = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
            JS_SetLazyCompileHook(
                rt,
                Some(on_lazy_compile),
                Rc::as_ptr(self).cast_mut().cast(),
            );
        }
    }

    pub(crate) fn detach(ctx: &Ctx<'_>) {
        // SAFETY: the context is live and locked; this clears the hook.
        unsafe {
            let rt = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
            JS_SetLazyCompileHook(rt, None, std::ptr::null_mut());
        }
    }

    /// Evaluates the classic script `source` as a global script named
    /// `name`, from the cache when it has the script, and returns
    /// `Ok(false)` when it is too short to go through the cache (the
    /// caller then evaluates it as usual).
    pub(crate) fn eval_classic(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        name: &str,
        origin: Origin,
    ) -> rquickjs::Result<bool> {
        match self.compile(ctx, source, name, Kind::Classic, origin) {
            None => Ok(false),
            Some(function) => run(ctx, function?).map(|()| true),
        }
    }

    /// Compiles the module `source` named `name`, or reads it from the
    /// cache, and returns it; `None` when it is too short to go through the
    /// cache. Its imports are loaded on the way, as `JS_Eval` loads them.
    pub(crate) fn compile_module(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        name: &str,
    ) -> Option<rquickjs::Result<NonNull<qjs::JSModuleDef>>> {
        let value = match self.compile(ctx, source, name, Kind::Module, Origin::External)? {
            Ok(value) => value,
            Err(e) => return Some(Err(e)),
        };
        // SAFETY: a module value: QuickJS's list of loaded modules holds the
        // module too, so the value's own reference is freed once its
        // pointer is read.
        let module = unsafe {
            let module = qjs::JS_VALUE_GET_PTR(value).cast::<qjs::JSModuleDef>();
            qjs::JS_FreeValue(ctx.as_raw().as_ptr(), value);
            module
        };
        Some(NonNull::new(module).ok_or(rquickjs::Error::Unknown))
    }

    /// The compiled script, read from the cache or compiled from `source`
    /// (and then noted for an entry), not run yet.
    fn compile(
        &self,
        ctx: &Ctx<'_>,
        source: &str,
        name: &str,
        kind: Kind,
        origin: Origin,
    ) -> Option<rquickjs::Result<qjs::JSValue>> {
        if source.len() < MIN_SCRIPT_LEN || source.len() > u32::MAX as usize / 2 {
            return None;
        }
        let Ok(filename) = CString::new(name) else {
            return Some(Err(rquickjs::Error::Unknown));
        };
        let key = Key::new(kind, self.lazy, source.as_bytes());
        let raw = ctx.as_raw().as_ptr();
        // SAFETY: `raw` is the live context `ctx` wraps, locked for this call.
        let rt = unsafe { qjs::JS_GetRuntime(raw) };

        if let Some(entry) = self.cache.lookup(&key, source.len()) {
            // The slot is taken first: a module's imports are read while it
            // is, and they take slots of their own.
            let tag = self.note(Used {
                key,
                kind,
                how: How::Read(entry.clone()),
                ran: Vec::new(),
            });
            let src = JSScriptSource::new(&entry.text, filename.as_ptr(), tag, &[]);
            let code = entry.code().unwrap_or_default();
            // SAFETY: the code was written by this engine build (files say
            // which build wrote them, and are checksummed), for this text,
            // which `src` describes and `entry` keeps alive. Empty code
            // fails to read.
            let value = with_source(rt, &src, || unsafe {
                qjs::JS_ReadObject(
                    raw,
                    code.as_ptr(),
                    code.len() as _,
                    (qjs::JS_READ_OBJ_BYTECODE as i32) | JS_READ_OBJ_SCRIPT,
                )
            });
            drop(code);
            // SAFETY: inspects the tag of a value.
            if !unsafe { qjs::JS_IsException(value) } {
                return Some(Ok(value));
            }
            // Out of memory, or an import failed to load: compiling says
            // which, as it would have without the cache.
            drop(ctx.catch());
            self.used.borrow_mut()[slot(tag)].how = How::Compiled;
        }

        let build = self
            .cache
            .note_miss(&key, source.len(), origin == Origin::Inline);
        let text = Text::new(source.as_bytes());
        let tag = self.note(Used {
            key,
            kind,
            how: How::Compiled,
            ran: Vec::new(),
        });
        let src = JSScriptSource::new(&text, filename.as_ptr(), tag, &[]);
        // SAFETY: `src` describes `text`, which outlives the call; the
        // input is exactly `src.buf`, as the engine checks.
        let value = with_source(rt, &src, || unsafe {
            qjs::JS_Eval(
                raw,
                src.buf,
                src.len as _,
                filename.as_ptr(),
                kind.eval_flags(),
            )
        });
        // SAFETY: inspects the tag of a value.
        if unsafe { qjs::JS_IsException(value) } {
            return Some(Err(rquickjs::Error::Exception));
        }
        // The entry is written now, before any of the script has run, so
        // nothing of the page is in it; the functions it runs are noted
        // for the next time the entry is made (see `finish`).
        if build
            && let Some(entry) =
                write(ctx, value).and_then(|code| Entry::new(text, &code, Vec::new(), 0))
        {
            self.cache.insert(key, entry);
            self.used.borrow_mut()[slot(tag)].how = How::Created;
        }
        Some(Ok(value))
    }

    /// Takes the next slot for a script, and returns its tag.
    fn note(&self, used: Used) -> u64 {
        let mut all = self.used.borrow_mut();
        all.push(used);
        all.len() as u64
    }

    /// Notes what the page learnt about the scripts it used, and returns
    /// the entries to make again with the functions it ran compiled in.
    /// Scripts it uses after this are noted afresh.
    pub(crate) fn finish(&self) -> ScriptCacheWork {
        let mut builds = Vec::new();
        for mut u in self.used.take() {
            u.ran.sort_unstable();
            u.ran.dedup();
            match u.how {
                How::Compiled => {}
                // Made from this page's compile: the functions it ran are
                // compiled in when a later page uses the entry, which
                // scripts seen once never are.
                How::Created => {
                    if !u.ran.is_empty() {
                        self.cache.want(&u.key, &u.ran);
                    }
                }
                How::Read(entry) => {
                    self.cache.state().stats.compiled_late += u.ran.len() as u64;
                    let pending = merge(&entry.wanted, &u.ran);
                    if pending.is_empty() {
                        continue;
                    }
                    let enough = pending.len() >= (entry.hints.len() / REBUILD_SHARE).max(1);
                    if entry.rebuilds < MAX_REBUILDS && enough {
                        builds.push(Build {
                            key: u.key,
                            kind: u.kind,
                            hints: merge(&entry.hints, &pending),
                            text: entry.text,
                            rebuilds: entry.rebuilds + 1,
                        });
                    } else if !u.ran.is_empty() {
                        self.cache.want(&u.key, &u.ran);
                    }
                }
            }
        }
        ScriptCacheWork {
            cache: (!builds.is_empty()).then(|| self.cache.clone()),
            lazy: self.lazy,
            builds,
        }
    }
}

/// Entries a page left to be made again, with the functions it ran
/// compiled in (see [`JsRuntime::take_script_cache_work`]).
///
/// [`JsRuntime::take_script_cache_work`]: crate::JsRuntime::take_script_cache_work
#[must_use = "the entries are only made by `run`"]
pub struct ScriptCacheWork {
    cache: Option<ScriptCache>,
    lazy: bool,
    builds: Vec<Build>,
}

struct Build {
    key: Key,
    kind: Kind,
    text: Arc<Text>,
    hints: Vec<u32>,
    rebuilds: u8,
}

impl fmt::Debug for ScriptCacheWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptCacheWork")
            .field("entries", &self.builds.len())
            .finish()
    }
}

/// Nothing to do.
impl Default for ScriptCacheWork {
    fn default() -> Self {
        Self {
            cache: None,
            lazy: true,
            builds: Vec::new(),
        }
    }
}

impl ScriptCacheWork {
    /// Whether there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.builds.is_empty()
    }

    /// Compiles the scripts again, each in a runtime of its own, and puts
    /// the entries in the cache.
    pub fn run(self) {
        let Some(cache) = self.cache else {
            return;
        };
        let Some(compiler) = Compiler::new(self.lazy) else {
            return;
        };
        for b in self.builds {
            if let Some(entry) = compiler
                .compile(b.kind, &b.text, &b.hints)
                .and_then(|code| Entry::new(b.text, &code, b.hints, b.rebuilds))
            {
                cache.insert(b.key, entry);
            }
        }
    }
}

/// The index of a script's slot from its tag.
fn slot(tag: u64) -> usize {
    usize::try_from(tag - 1).unwrap_or(0)
}

/// Two ascending lists as one, without repeats.
fn merge(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut all = Vec::with_capacity(a.len() + b.len());
    all.extend_from_slice(a);
    all.extend_from_slice(b);
    all.sort_unstable();
    all.dedup();
    all
}

/// Writes a compiled script out for the cache, functions not compiled yet
/// as they are.
fn write(ctx: &Ctx<'_>, value: qjs::JSValue) -> Option<Vec<u8>> {
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `raw` is live and locked, and `value` a compiled script it
    // owns. The buffer is the context's and is freed once, below.
    unsafe {
        let mut len = 0;
        let buf = qjs::JS_WriteObject(
            raw,
            &mut len,
            value,
            (qjs::JS_WRITE_OBJ_BYTECODE as i32) | JS_WRITE_OBJ_SCRIPT,
        );
        if buf.is_null() {
            drop(ctx.catch());
            return None;
        }
        let code = std::slice::from_raw_parts(buf, len as usize).to_vec();
        qjs::js_free(raw, buf.cast());
        Some(code)
    }
}

/// Runs a compiled global script, consuming it.
fn run(ctx: &Ctx<'_>, function: qjs::JSValue) -> rquickjs::Result<()> {
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `function` is a function bytecode value owned here, which
    // `JS_EvalFunction` takes; its result is freed once.
    unsafe {
        let value = qjs::JS_EvalFunction(raw, function);
        if qjs::JS_IsException(value) {
            return Err(rquickjs::Error::Exception);
        }
        qjs::JS_FreeValue(raw, value);
    }
    Ok(())
}

unsafe extern "C" fn on_lazy_compile(opaque: *mut c_void, tag: u64, start: u32) {
    // SAFETY: `opaque` is the `PageScripts` that `attach` set, alive until
    // `detach`.
    let scripts = unsafe { &*opaque.cast::<PageScripts>().cast_const() };
    let Ok(mut used) = scripts.used.try_borrow_mut() else {
        return;
    };
    let index = usize::try_from(tag).unwrap_or(usize::MAX).wrapping_sub(1);
    if let Some(u) = used.get_mut(index) {
        u.ran.push(start);
    }
}

/// A runtime of its own for making entries: nothing a page made is in it.
struct Compiler {
    context: rquickjs::Context,
    _runtime: rquickjs::Runtime,
    count: std::cell::Cell<u32>,
}

impl Compiler {
    fn new(lazy: bool) -> Option<Self> {
        let (runtime, _) = crate::heap::new_runtime(None).ok()?;
        let context = rquickjs::Context::full(&runtime).ok()?;
        context.with(|ctx| {
            // SAFETY: the context is live and locked; these set flags.
            unsafe {
                let rt = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
                JS_SetLazyFunctions(rt, lazy);
                qjs::JS_SetModuleLoaderFunc(rt, None, Some(empty_module), std::ptr::null_mut());
            }
        });
        Some(Self {
            context,
            _runtime: runtime,
            count: std::cell::Cell::new(0),
        })
    }

    /// Compiles `text` with the functions at `hints` compiled with it, and
    /// writes it out for the cache.
    fn compile(&self, kind: Kind, text: &Arc<Text>, hints: &[u32]) -> Option<Vec<u8>> {
        // Modules of one name would find each other when they import.
        let count = self.count.get() + 1;
        self.count.set(count);
        let name = CString::new(format!("dasher:script-cache/{count}")).ok()?;
        self.context.with(|ctx| {
            let raw = ctx.as_raw().as_ptr();
            // SAFETY: `raw` is live and locked. `src` describes `text`,
            // which outlives the calls; the compiled function and the
            // written buffer are freed once, below.
            unsafe {
                let rt = qjs::JS_GetRuntime(raw);
                qjs::JS_UpdateStackTop(rt);
                let src = JSScriptSource::new(text, std::ptr::null(), 0, hints);
                let function = with_source(rt, &src, || {
                    qjs::JS_Eval(raw, src.buf, src.len as _, name.as_ptr(), kind.eval_flags())
                });
                if qjs::JS_IsException(function) {
                    drop(ctx.catch());
                    return None;
                }
                let code = write(&ctx, function);
                qjs::JS_FreeValue(raw, function);
                code
            }
        })
    }
}

/// Compiling a module loads what it imports: here every import is a
/// module with no exports, since nothing is linked or run.
unsafe extern "C" fn empty_module(
    ctx: *mut qjs::JSContext,
    name: *const c_char,
    _opaque: *mut c_void,
) -> *mut qjs::JSModuleDef {
    unsafe extern "C" fn init(_: *mut qjs::JSContext, _: *mut qjs::JSModuleDef) -> std::ffi::c_int {
        0
    }
    // SAFETY: called by the engine with a live context and a C string.
    unsafe { qjs::JS_NewCModule(ctx, name, Some(init)) }
}

// ---------------------------------------------------------------------
// Files.

/// Entries kept as files in a directory, one per key.
///
/// A file is the magic, the engine build, the key, the lengths, the text,
/// the code, the hints and a SHA-256 of all that. It is written to a
/// temporary name and renamed, so a reader sees all of it or nothing.
struct Disk {
    dir: PathBuf,
    limit: u64,
    /// Bytes in the directory, counted on first use; `None` until then.
    bytes: Mutex<Option<u64>>,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Disk {
    fn new(dir: PathBuf, limit: u64) -> Self {
        Self {
            dir,
            limit,
            bytes: Mutex::new(None),
        }
    }

    fn path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!("{}.jsc", key.hex()))
    }

    fn load(&self, key: &Key) -> Option<Entry> {
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        let entry = decode(key, &bytes);
        if entry.is_none() {
            // Damaged, or from another build: make room for a good one.
            let _ = fs::remove_file(&path);
        } else if let Ok(file) = fs::File::options().write(true).open(&path) {
            // Recently used files are the last to go.
            let _ = file.set_modified(SystemTime::now());
        }
        entry
    }

    fn store(&self, key: &Key, entry: &Entry) {
        let bytes = encode(key, entry);
        let _ = fs::create_dir_all(&self.dir);
        let temp = self.dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::File::create(&temp)
            .and_then(|mut f| f.write_all(&bytes))
            .and_then(|()| fs::rename(&temp, self.path(key)));
        if written.is_err() {
            let _ = fs::remove_file(&temp);
            return;
        }
        let mut total = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = match *total {
            Some(n) => n + bytes.len() as u64,
            None => dir_size(&self.dir),
        };
        *total = Some(if now > self.limit { self.trim() } else { now });
    }

    /// Deletes the least recently used files until the directory holds
    /// nine tenths of the limit, and returns what it then holds.
    fn trim(&self) -> u64 {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(&self.dir)
            .map(|dir| {
                dir.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jsc"))
                    .filter_map(|e| {
                        let meta = e.metadata().ok()?;
                        Some((meta.modified().ok()?, meta.len(), e.path()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        for (_, len, path) in files {
            if total <= self.limit / 10 * 9 {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total -= len;
            }
        }
        total
    }
}

fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn encode(key: &Key, entry: &Entry) -> Vec<u8> {
    let text = entry.text.as_bytes();
    let mut out = Vec::with_capacity(text.len() + entry.code.len() + entry.hints.len() * 4 + 160);
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(ENGINE_ID.as_bytes());
    out.extend_from_slice(&key.0);
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(&(entry.code.len() as u64).to_le_bytes());
    out.extend_from_slice(&(entry.code_len as u64).to_le_bytes());
    out.extend_from_slice(&(entry.hints.len() as u64).to_le_bytes());
    out.extend_from_slice(&(entry.wanted.len() as u64).to_le_bytes());
    out.push(entry.rebuilds);
    out.extend_from_slice(text);
    out.extend_from_slice(&entry.code);
    for offset in entry.hints.iter().chain(entry.wanted.iter()) {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    let sum = digest(&SHA256, &out);
    out.extend_from_slice(sum.as_ref());
    out
}

fn decode(key: &Key, bytes: &[u8]) -> Option<Entry> {
    let (body, sum) = bytes.split_at_checked(bytes.len().checked_sub(32)?)?;
    if digest(&SHA256, body).as_ref() != sum {
        return None;
    }
    let mut rest = body;
    let mut take = |n: usize| -> Option<&[u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        rest = tail;
        Some(head)
    };
    let u64_at = |b: &[u8]| usize::try_from(u64::from_le_bytes(b.try_into().ok()?)).ok();
    if take(8)? != FILE_MAGIC
        || take(ENGINE_ID.len())? != ENGINE_ID.as_bytes()
        || take(32)? != key.0
    {
        return None;
    }
    let text_len = u64_at(take(8)?)?;
    let code_size = u64_at(take(8)?)?;
    let code_len = u64_at(take(8)?)?;
    let hint_count = u64_at(take(8)?)?;
    let wanted_count = u64_at(take(8)?)?;
    let rebuilds = take(1)?[0];
    let text = Text::new(take(text_len)?);
    let code: Arc<[u8]> = take(code_size)?.into();
    let mut offsets = |n: usize| -> Option<Arc<[u32]>> {
        Some(
            take(n.checked_mul(4)?)?
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    };
    let hints = offsets(hint_count)?;
    let wanted = offsets(wanted_count)?;
    if !rest.is_empty() {
        return None;
    }
    Some(Entry {
        text,
        code,
        code_len,
        hints,
        wanted,
        rebuilds,
        used: 0,
    })
}
