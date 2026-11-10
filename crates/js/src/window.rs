//! The window environment: the page URL, navigation requests and the
//! configuration behind `navigator`, `screen` and the viewport.

use std::cell::RefCell;
use std::rc::Rc;

use rquickjs::{Ctx, Function, Object};
use url::Url;

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

    pub(crate) fn has_navigation(&self) -> bool {
        self.navigation.borrow().is_some()
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
    native.set("env", env)?;
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
}
