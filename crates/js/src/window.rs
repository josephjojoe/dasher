//! The window environment: the page URL, navigation requests and the
//! configuration behind `navigator`, `screen` and the viewport.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use rquickjs::{Ctx, Function, Object};
use url::Url;

use crate::media;
use crate::runtime::{JsRuntime, Shared};
use crate::storage::{self, StorageStore};

/// The `navigator.userAgent` used when none is configured: a current desktop
/// Chrome on Linux, matching dasher-net's default `User-Agent` header.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";

/// The window and screen size pages see. Nothing is laid out; these numbers
/// feed `innerWidth`, `screen`, `matchMedia` and similar.
#[derive(Clone, Debug, PartialEq)]
pub struct Viewport {
    /// `innerWidth` / `outerWidth`, in CSS pixels.
    pub width: u32,
    /// `innerHeight` / `outerHeight`, in CSS pixels.
    pub height: u32,
    /// `devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// `screen.width`.
    pub screen_width: u32,
    /// `screen.height`.
    pub screen_height: u32,
    /// Whether the device has a touch screen and no mouse: changes
    /// `navigator.maxTouchPoints` and the `pointer` / `hover` media features.
    pub touch: bool,
}

impl Default for Viewport {
    /// A desktop 1920×1080 window filling a 1920×1080 screen.
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            device_pixel_ratio: 1.0,
            screen_width: 1920,
            screen_height: 1080,
            touch: false,
        }
    }
}

/// How the synthetic layout places elements, which decides what counts as
/// in view.
///
/// Nothing is really laid out. Either way, every rendered element has a
/// non-zero box, as wide as the viewport, and hidden ones (`display: none`
/// inline, `hidden`, never-rendered elements) have an empty one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayoutMode {
    /// Everything is in view. Every rendered element is a 20px-tall box at
    /// the top of the viewport, wherever the page is scrolled, and
    /// `IntersectionObserver` reports it intersecting as soon as it is
    /// observed. The page can be scrolled without limit.
    ///
    /// One concession to infinite scroll: a target that content inserted
    /// before it pushes further down the document counts as having left
    /// the viewport, until the page next scrolls. That lets a "load more"
    /// sentinel fire again on every scroll, as it would in a browser.
    #[default]
    InView,
    /// Elements are only in view once scrolled to. The elements under
    /// `<body>` are stacked in document order, 20px per element, each
    /// spanning its descendants, and the document is as tall as the stack
    /// (at least the viewport). Boxes move as the page scrolls, scrolling
    /// stops at the bottom, and `IntersectionObserver` reports what overlaps
    /// the viewport.
    ///
    /// Content below the fold only loads once scrolled to, as in a browser,
    /// and a page whose content does not fill the viewport may stop loading
    /// more, as it would in a browser with a tall window.
    Scrolled,
}

/// How a page's window environment is set up.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    /// The document URL: `location`, the origin for storage and
    /// `postMessage`, and the base for relative URLs. Must be an absolute
    /// URL.
    pub url: String,
    /// `navigator.userAgent`. Keep it in step with the `User-Agent` header
    /// the network layer sends.
    pub user_agent: String,
    /// `navigator.platform`.
    pub platform: String,
    /// `navigator.languages`, most preferred first. The first is
    /// `navigator.language`.
    pub languages: Vec<String>,
    /// `navigator.hardwareConcurrency`.
    pub hardware_concurrency: u32,
    /// `navigator.deviceMemory`, in GiB.
    pub device_memory: u32,
    /// Window and screen dimensions.
    pub viewport: Viewport,
    /// How elements are placed, and so what is in view.
    pub layout: LayoutMode,
    /// Backs `localStorage`. Pass the same store to several pages to share
    /// items between them, as a browser profile would.
    pub local_storage: StorageStore,
    /// Backs `sessionStorage`. Pages of one browsing session (a tab) share
    /// it in browsers; by default each page gets its own.
    pub session_storage: StorageStore,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            url: String::from("about:blank"),
            user_agent: String::from(DEFAULT_USER_AGENT),
            platform: String::from("Linux x86_64"),
            languages: vec![String::from("en-US"), String::from("en")],
            hardware_concurrency: 8,
            device_memory: 8,
            viewport: Viewport::default(),
            layout: LayoutMode::default(),
            local_storage: StorageStore::new(),
            session_storage: StorageStore::new(),
        }
    }
}

/// What kind of navigation a page asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavigationKind {
    /// A new history entry: `location.href = …`, `location.assign()` and
    /// the other `location` setters.
    Push,
    /// Replaces the current entry: `location.replace()`.
    Replace,
    /// `location.reload()` or `history.go(0)`.
    Reload,
}

/// A request from the page to load a different document.
///
/// dasher does not navigate by itself: the request is recorded for the page
/// layer, which decides whether to follow it. Same-document changes
/// (`pushState`, `replaceState`, fragment changes) are not requests; they
/// just update [`JsRuntime::url`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationRequest {
    /// The absolute URL to load.
    pub url: String,
    /// How it was requested.
    pub kind: NavigationKind,
}

/// Host-side window state, shared with the native functions.
pub(crate) struct WindowState {
    url: RefCell<Url>,
    navigation: RefCell<Option<NavigationRequest>>,
}

impl WindowState {
    pub(crate) fn new(url: Url) -> Self {
        Self {
            url: RefCell::new(url),
            navigation: RefCell::new(None),
        }
    }

    /// The document's current URL.
    pub(crate) fn url(&self) -> Url {
        self.url.borrow().clone()
    }

    pub(crate) fn has_navigation(&self) -> bool {
        self.navigation.borrow().is_some()
    }

    /// Requests the navigation a declarative refresh asks for when it
    /// comes due, unless the page has already asked to go elsewhere.
    pub(crate) fn request_refresh(&self, url: String) {
        let mut navigation = self.navigation.borrow_mut();
        if navigation.is_none() {
            *navigation = Some(NavigationRequest {
                url,
                kind: NavigationKind::Replace,
            });
        }
    }
}

/// Parses the configured document URL.
pub(crate) fn parse_url(url: &str) -> Result<Url, String> {
    Url::parse(url).map_err(|e| format!("invalid page URL {url:?}: {e}"))
}

/// Adds the window's configuration and host functions to the prelude's
/// `native` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
    options: &WindowOptions,
) -> rquickjs::Result<()> {
    let env = Object::new(ctx.clone())?;
    env.set("url", shared.window.url.borrow().as_str())?;
    env.set("userAgent", options.user_agent.as_str())?;
    env.set("platform", options.platform.as_str())?;
    env.set("languages", options.languages.clone())?;
    env.set("hardwareConcurrency", options.hardware_concurrency)?;
    env.set("deviceMemory", options.device_memory)?;
    let viewport = &options.viewport;
    env.set("width", viewport.width)?;
    env.set("height", viewport.height)?;
    env.set("devicePixelRatio", viewport.device_pixel_ratio)?;
    env.set("screenWidth", viewport.screen_width)?;
    env.set("screenHeight", viewport.screen_height)?;
    env.set("touch", viewport.touch)?;
    env.set(
        "layout",
        match options.layout {
            LayoutMode::InView => "inView",
            LayoutMode::Scrolled => "scrolled",
        },
    )?;
    native.set("env", env)?;
    media::install(ctx, native, &options.viewport)?;
    storage::install(
        ctx,
        native,
        &shared.window.url.borrow(),
        &options.local_storage,
        &options.session_storage,
    )?;

    let s = Rc::clone(shared);
    native.set(
        "setUrl",
        Function::new(ctx.clone(), move |href: String| {
            if let Ok(url) = Url::parse(&href) {
                *s.window.url.borrow_mut() = url;
            }
        })?,
    )?;
    let s = Rc::clone(shared);
    native.set(
        "navigate",
        Function::new(ctx.clone(), move |url: String, kind: String| {
            let kind = match kind.as_str() {
                "replace" => NavigationKind::Replace,
                "reload" => NavigationKind::Reload,
                _ => NavigationKind::Push,
            };
            *s.window.navigation.borrow_mut() = Some(NavigationRequest { url, kind });
        })?,
    )?;
    Ok(())
}

impl JsRuntime {
    /// The document's current URL. Starts as [`WindowOptions::url`] and
    /// follows `pushState`, `replaceState`, history traversal and fragment
    /// navigation.
    pub fn url(&self) -> String {
        self.shared().window.url.borrow().to_string()
    }

    /// The most recent request to load another document, if the page made
    /// one. Later requests replace earlier ones, as in browsers.
    pub fn navigation_request(&self) -> Option<NavigationRequest> {
        self.shared().window.navigation.borrow().clone()
    }

    /// Removes and returns the pending navigation request.
    pub fn take_navigation_request(&self) -> Option<NavigationRequest> {
        self.shared().window.navigation.borrow_mut().take()
    }

    /// Schedules a declarative refresh, as `<meta http-equiv="refresh">`
    /// or a `Refresh` header asks for: once `delay` of page time has
    /// passed, the page requests a navigation to `url` (with
    /// [`NavigationKind::Replace`]), unless it has already requested one.
    ///
    /// Until then the refresh counts as a timer: [`run`](Self::run) waits
    /// for it, or skips ahead to it under the virtual clock, and the page
    /// is not [idle](Self::is_idle). Replaces any refresh scheduled
    /// before. The URL is not checked; the host decides whether to follow
    /// the request.
    pub fn schedule_refresh(&self, url: &str, delay: Duration) {
        let due = self.now().saturating_add(delay);
        *self.event_loop().refresh.borrow_mut() = Some((due, url.to_owned()));
    }

    /// When the scheduled [refresh](Self::schedule_refresh) comes due, in
    /// page time, if one is scheduled and has not come due yet.
    pub fn refresh_due(&self) -> Option<Duration> {
        self.event_loop()
            .refresh
            .borrow()
            .as_ref()
            .map(|&(due, _)| due)
    }
}
