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
//! Functions are compiled on their first call (see
//! `vendor/rquickjs-sys/PATCHES.md`), so compiling a script compiles only
//! its top level and the functions it is likely to call at once. The rest
//! are kept as records of where they are in the text. An entry is the
//! script's text, written once, and its compiled code in QuickJS's bytecode
//! format (`JS_WRITE_OBJ_SCRIPT`), with the functions that are not
//! compiled written as those records.
//!
//! Each page that uses an entry notes which of its functions it compiled
//! on their first call. When the page is dropped, the script is compiled
//! again with those functions compiled with it ("hints"), and the entry is
//! replaced. The next page then gets exactly the functions pages have been
//! running, compiled, and the rest still waiting for their first call. The
//! hints grow as pages run functions the entry lacks, at most
//! [`MAX_REBUILDS`] times per entry, so a script settles after a few pages.
//!
//! Every function read back shares one copy of the script's text, and that
//! copy is the cache's own: pages point into it rather than copying it into
//! their heaps (`JSScriptSource`), and it is freed when the last page using
//! it and the cache have both let it go.
//!
//! # What pages can see
//!
//! Nothing a page does reaches another page. An entry is made from the
//! script's text alone, in a runtime of its own with nothing in it, never
//! from the objects of a page that ran the script; the hints only say which
//! functions to compile now rather than later, which a script cannot tell
//! apart. A page reads the entry into objects of its own. The key is the
//! SHA-256 of the text, so a page only finds the entry of a script whose
//! text it fetched itself.
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
const FILE_MAGIC: &[u8; 8] = b"dshrjsc1";

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
    /// Entries made or remade after a page.
    pub builds: u64,
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
    fn new(lazy: bool, text: &[u8]) -> Self {
        let mut input = Vec::with_capacity(ENGINE_ID.len() + 8 + text.len());
        input.extend_from_slice(ENGINE_ID.as_bytes());
        // The kind: a classic script, and whether functions wait for
        // their first call (`DASHER_EAGER_JS` turns that off).
        input.extend_from_slice(if lazy { b"\0classic" } else { b"\0eager\0\0" });
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
    code: Arc<[u8]>,
    /// Ascending offsets of the functions compiled with the script.
    hints: Arc<[u32]>,
    rebuilds: u8,
    used: u64,
}

impl Entry {
    fn size(&self) -> usize {
        // the text, the code, the hints and the map's bookkeeping
        self.text.bytes.len() + self.code.len() + self.hints.len() * 4 + 128
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
    fn JS_SetLazyCompileHook(rt: *mut qjs::JSRuntime, hook: Option<LazyCompileHook>, opaque: *mut c_void);
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
    text: Arc<Text>,
    /// The entry's hints, when the script came from the cache.
    entry: Option<(Arc<[u32]>, u8)>,
    /// Whether to make an entry for a script compiled from its text.
    build: bool,
    /// Offsets of the functions compiled on their first call.
    ran: Vec<u32>,
}

/// Classic scripts run through the cache.
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
            JS_SetLazyCompileHook(rt, Some(on_lazy_compile), Rc::as_ptr(self).cast_mut().cast());
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
        if source.len() < MIN_SCRIPT_LEN || source.len() > u32::MAX as usize / 2 {
            return Ok(false);
        }
        let key = Key::new(self.lazy, source.as_bytes());
        let filename = CString::new(name)?;
        let raw = ctx.as_raw().as_ptr();
        // SAFETY: `raw` is the live context `ctx` wraps, locked for this call.
        let rt = unsafe { qjs::JS_GetRuntime(raw) };
        let tag = self.used.borrow().len() as u64 + 1;

        if let Some(entry) = self.cache.lookup(&key, source.len()) {
            let src = JSScriptSource::new(&entry.text, filename.as_ptr(), tag, &[]);
            // SAFETY: the code was written by `build` with this engine
            // build (the disk checks that), for this text, which `src`
            // describes and `entry` keeps alive.
            let function = with_source(rt, &src, || unsafe {
                qjs::JS_ReadObject(
                    raw,
                    entry.code.as_ptr(),
                    entry.code.len() as _,
                    (qjs::JS_READ_OBJ_BYTECODE as i32) | JS_READ_OBJ_SCRIPT,
                )
            });
            // SAFETY: inspects the tag of a value.
            if !unsafe { qjs::JS_IsException(function) } {
                self.used.borrow_mut().push(Used {
                    key,
                    text: Arc::clone(&entry.text),
                    entry: Some((Arc::clone(&entry.hints), entry.rebuilds)),
                    build: false,
                    ran: Vec::new(),
                });
                return run(ctx, function).map(|()| true);
            }
            // Out of memory, most likely: compile it instead.
            drop(ctx.catch());
        }

        let build = self.cache.note_miss(&key, source.len(), origin == Origin::Inline);
        let text = Text::new(source.as_bytes());
        let src = JSScriptSource::new(&text, filename.as_ptr(), tag, &[]);
        // SAFETY: `src` describes `text`, which outlives the call; the
        // input is exactly `src.buf`, as the engine checks.
        let function = with_source(rt, &src, || unsafe {
            qjs::JS_Eval(
                raw,
                src.buf,
                src.len as _,
                filename.as_ptr(),
                (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
            )
        });
        // SAFETY: inspects the tag of a value.
        if unsafe { qjs::JS_IsException(function) } {
            return Err(rquickjs::Error::Exception);
        }
        self.used.borrow_mut().push(Used {
            key,
            text,
            entry: None,
            build,
            ran: Vec::new(),
        });
        run(ctx, function).map(|()| true)
    }

    /// Makes or remakes the entries this page has reason to: scripts it
    /// compiled from their text, and scripts whose entry lacked functions
    /// it ran. Call it once the page's own runtime is gone, so that its
    /// memory and the compiler's are not needed at once.
    pub(crate) fn finish(&self) {
        let used = self.used.take();
        let builds: Vec<_> = used
            .into_iter()
            .filter_map(|mut u| {
                u.ran.sort_unstable();
                u.ran.dedup();
                match &u.entry {
                    None if u.build => Some((u.key, u.text, u.ran, 0)),
                    None => None,
                    Some((hints, rebuilds)) => {
                        let wanted = (hints.len() / REBUILD_SHARE).max(1);
                        if *rebuilds >= MAX_REBUILDS || u.ran.len() < wanted {
                            return None;
                        }
                        let mut all = hints.to_vec();
                        all.extend_from_slice(&u.ran);
                        all.sort_unstable();
                        all.dedup();
                        Some((u.key, u.text, all, rebuilds + 1))
                    }
                }
            })
            .collect();
        if builds.is_empty() {
            return;
        }
        let Some(compiler) = Compiler::new(self.lazy) else {
            return;
        };
        for (key, text, hints, rebuilds) in builds {
            if let Some(code) = compiler.compile(&text, &hints) {
                self.cache.insert(
                    key,
                    Entry {
                        text,
                        code: code.into(),
                        hints: hints.into(),
                        rebuilds,
                        used: 0,
                    },
                );
            }
        }
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
            }
        });
        Some(Self {
            context,
            _runtime: runtime,
        })
    }

    /// Compiles `text` with the functions at `hints` compiled with it, and
    /// writes it out for the cache.
    fn compile(&self, text: &Arc<Text>, hints: &[u32]) -> Option<Vec<u8>> {
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
                    qjs::JS_Eval(
                        raw,
                        src.buf,
                        src.len as _,
                        c"dasher:script-cache".as_ptr(),
                        (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32,
                    )
                });
                if qjs::JS_IsException(function) {
                    drop(ctx.catch());
                    return None;
                }
                let mut len = 0;
                let buf = qjs::JS_WriteObject(
                    raw,
                    &mut len,
                    function,
                    (qjs::JS_WRITE_OBJ_BYTECODE as i32) | JS_WRITE_OBJ_SCRIPT,
                );
                qjs::JS_FreeValue(raw, function);
                if buf.is_null() {
                    drop(ctx.catch());
                    return None;
                }
                let code = std::slice::from_raw_parts(buf, len as usize).to_vec();
                qjs::js_free(raw, buf.cast());
                Some(code)
            }
        })
    }
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
        let mut total = self.bytes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        .map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
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
    out.extend_from_slice(&(entry.hints.len() as u64).to_le_bytes());
    out.push(entry.rebuilds);
    out.extend_from_slice(text);
    out.extend_from_slice(&entry.code);
    for hint in entry.hints.iter() {
        out.extend_from_slice(&hint.to_le_bytes());
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
    if take(8)? != FILE_MAGIC || take(ENGINE_ID.len())? != ENGINE_ID.as_bytes() || take(32)? != key.0 {
        return None;
    }
    let text_len = u64_at(take(8)?)?;
    let code_len = u64_at(take(8)?)?;
    let hint_count = u64_at(take(8)?)?;
    let rebuilds = take(1)?[0];
    let text = Text::new(take(text_len)?);
    let code: Arc<[u8]> = take(code_len)?.into();
    let hints: Arc<[u32]> = take(hint_count.checked_mul(4)?)?
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if !rest.is_empty() {
        return None;
    }
    Some(Entry {
        text,
        code,
        hints,
        rebuilds,
        used: 0,
    })
}
