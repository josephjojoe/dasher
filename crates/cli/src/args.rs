//! Command-line parsing.

use std::time::Duration;

use dasher::Visibility;

use crate::service::{self, ServeOptions};

pub const USAGE: &str = "\
usage: dasher [options] <url>
       dasher serve [options]   run the HTTP service
       dasher lambda            run as an AWS Lambda custom runtime

Loads <url> in a headless browser, runs its JavaScript until the page
settles, and prints the resulting HTML. See `dasher serve --help` for
the service modes.

Waiting:
      --wait <strategy>     when the page counts as ready [default: settled]
                              load           the load event has fired
                              network-idle   no requests for the quiet period
                              network-idle2  at most 2 requests for it
                              dom-quiet      no DOM changes for it
                              settled        network-idle and dom-quiet
      --quiet <ms>          quiet period for the strategies above [default: 500]
      --wait-for <css>      also wait until an element matches the selector
      --wait-until <js>     also wait until the JS expression is truthy
  -t, --timeout <secs>      time limit for the whole run [default: 30]
      --scroll              then scroll to load infinite feeds and lazy content
      --max-scrolls <n>     most scrolls with --scroll [default: 20]

Browser:
  -A, --user-agent <ua>     User-Agent header and navigator.userAgent
      --viewport <WxH>      window size in CSS pixels [default: 1920x1080]
  -H, --header <h: v>       send a header with every request (repeatable)
      --cookie <cookie>     set a cookie for <url> first, in Set-Cookie
                            syntax: 'name=value; Path=/' (repeatable)
      --proxy <url>         send requests through an http:// proxy
      --block <pattern>     refuse requests to matching URLs; * is a
                            wildcard: '*://*.doubleclick.net/*' (repeatable)
      --realtime            run timers in real time instead of skipping
                            ahead when the page is only waiting on them
      --visibility <policy> what counts as in view, which decides how much
                            lazy content loads [default: loaded]
                              loaded    the page as it stood at load is in
                                        view; what it adds later is below
                                        the fold until --scroll reveals it
                              all       everything, always: the most lazy
                                        content, but a page that keeps
                                        loading more can run to the timeout
                              viewport  only what is scrolled to: the
                                        least work and the least content
      --memory-limit <MiB>  cap the page's JS heap; 0 for no cap [default: 256]
      --max-dom-nodes <n>   cap the nodes in the page's DOM; 0 for no cap
                            [default: 500000]. A page over either limit is
                            stopped and the run fails
      --script-cache-dir <dir>
                            keep the page's compiled scripts in <dir>, and
                            use those earlier runs kept there, so a later
                            run skips compiling them

Output:
  -e, --eval <js>           print the value of a JS expression or function
                            instead of the HTML. Promises are awaited;
                            strings print raw, other values as JSON
      --json                print a JSON object with the url, status, html
                            (or eval result), console messages and errors
  -v, --verbose             print console messages and page errors to stderr
  -h, --help                print this help
  -V, --version             print the version

Exit status: 0 on success, 1 if the page could not be loaded or --eval
failed, 2 for invalid arguments.";

/// When the page counts as ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Load,
    NetworkIdle,
    NetworkIdle2,
    DomQuiet,
    Settled,
}

/// What to do, from the command line.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Run(Box<Options>),
    /// Help for `serve` and `lambda`.
    ServiceHelp,
    Serve(ServeOptions),
    Lambda,
}

/// Options for loading a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub url: String,
    pub wait: Wait,
    pub quiet: Duration,
    pub wait_for: Option<String>,
    pub wait_until: Option<String>,
    pub timeout: Duration,
    pub scroll: bool,
    pub max_scrolls: Option<usize>,
    pub user_agent: Option<String>,
    pub viewport: Option<(u32, u32)>,
    pub headers: Vec<(String, String)>,
    pub cookies: Vec<String>,
    pub proxy: Option<String>,
    pub block: Vec<String>,
    pub realtime: bool,
    pub visibility: Option<Visibility>,
    /// In MiB; `Some(0)` lifts the limit, `None` keeps the default.
    pub memory_limit: Option<usize>,
    /// `Some(0)` lifts the limit, `None` keeps the default.
    pub max_dom_nodes: Option<usize>,
    pub script_cache_dir: Option<std::path::PathBuf>,
    pub eval: Option<String>,
    pub json: bool,
    pub verbose: bool,
}

impl Options {
    fn new(url: String) -> Self {
        Self {
            url,
            wait: Wait::Settled,
            quiet: Duration::from_millis(500),
            wait_for: None,
            wait_until: None,
            timeout: Duration::from_secs(30),
            scroll: false,
            max_scrolls: None,
            user_agent: None,
            viewport: None,
            headers: Vec::new(),
            cookies: Vec::new(),
            proxy: None,
            block: Vec::new(),
            realtime: false,
            visibility: None,
            memory_limit: None,
            max_dom_nodes: None,
            script_cache_dir: None,
            eval: None,
            json: false,
            verbose: false,
        }
    }
}

/// Parses the arguments after the program name, with `env` for the
/// service's environment variables.
pub fn parse(
    args: impl IntoIterator<Item = String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Command, String> {
    let mut args = args.into_iter().peekable();
    match args.peek().map(String::as_str) {
        Some("serve") => {
            args.next();
            return Ok(match service::parse_serve(args, env)? {
                Some(options) => Command::Serve(options),
                None => Command::ServiceHelp,
            });
        }
        Some("lambda") => {
            args.next();
            return match args.next().as_deref() {
                None => Ok(Command::Lambda),
                Some("-h" | "--help") => Ok(Command::ServiceHelp),
                Some(other) => Err(format!("lambda takes no arguments, not {other:?}")),
            };
        }
        _ => {}
    }
    let mut url = None;
    let mut options = Options::new(String::new());
    let mut only_positional = false;

    while let Some(arg) = args.next() {
        if only_positional || !arg.starts_with('-') || arg == "-" {
            if url.replace(arg).is_some() {
                return Err("only one URL may be given".into());
            }
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        // --name=value
        let (name, mut inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_owned(), Some(value.into())),
            _ => (arg.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .take()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs {what}"))
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--wait" => options.wait = parse_wait(&value("a strategy")?)?,
            "--quiet" => options.quiet = Duration::from_millis(number(&name, &value("ms")?)?),
            "--wait-for" => options.wait_for = Some(value("a selector")?),
            "--wait-until" => options.wait_until = Some(value("an expression")?),
            "-t" | "--timeout" => options.timeout = seconds(&name, &value("seconds")?)?,
            "--scroll" => options.scroll = true,
            "--max-scrolls" => {
                options.max_scrolls = Some(number(&name, &value("a count")?)?);
                options.scroll = true;
            }
            "-A" | "--user-agent" => options.user_agent = Some(value("a user agent")?),
            "--viewport" => options.viewport = Some(viewport(&value("WIDTHxHEIGHT")?)?),
            "-H" | "--header" => options.headers.push(header(&value("a header")?)?),
            "--cookie" => options.cookies.push(value("a cookie")?),
            "--proxy" => options.proxy = Some(value("a proxy URL")?),
            "--block" => options.block.push(value("a URL pattern")?),
            "--realtime" => options.realtime = true,
            "--visibility" => options.visibility = Some(parse_visibility(&value("a policy")?)?),
            "--memory-limit" => options.memory_limit = Some(number(&name, &value("MiB")?)?),
            "--max-dom-nodes" => options.max_dom_nodes = Some(number(&name, &value("a count")?)?),
            "--script-cache-dir" => options.script_cache_dir = Some(value("a directory")?.into()),
            "-e" | "--eval" => options.eval = Some(value("an expression")?),
            "--json" => options.json = true,
            "-v" | "--verbose" => options.verbose = true,
            _ => return Err(format!("unknown option {name}")),
        }
        if inline.is_some() {
            return Err(format!("{name} takes no value"));
        }
    }

    options.url = url.ok_or("no URL given")?;
    Ok(Command::Run(Box::new(options)))
}

fn parse_wait(s: &str) -> Result<Wait, String> {
    Ok(match s {
        "load" => Wait::Load,
        "network-idle" | "networkidle" | "networkidle0" => Wait::NetworkIdle,
        "network-idle2" | "networkidle2" => Wait::NetworkIdle2,
        "dom-quiet" => Wait::DomQuiet,
        "settled" => Wait::Settled,
        _ => return Err(format!("unknown --wait strategy {s:?}")),
    })
}

fn parse_visibility(s: &str) -> Result<Visibility, String> {
    Visibility::from_name(s).ok_or_else(|| {
        format!("unknown --visibility policy {s:?}: expected loaded, all or viewport")
    })
}

fn number<T: std::str::FromStr>(name: &str, s: &str) -> Result<T, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{name} expects a whole number, not {s:?}"))
}

fn seconds(name: &str, s: &str) -> Result<Duration, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| format!("{name} expects a number of seconds, not {s:?}"))
}

fn viewport(s: &str) -> Result<(u32, u32), String> {
    let parsed = s
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        .filter(|&(w, h): &(u32, u32)| w > 0 && h > 0);
    parsed.ok_or_else(|| format!("--viewport expects WIDTHxHEIGHT, such as 390x844, not {s:?}"))
}

fn header(s: &str) -> Result<(String, String), String> {
    match s.split_once(':') {
        Some((name, value)) if !name.trim().is_empty() => {
            Ok((name.trim().to_owned(), value.trim().to_owned()))
        }
        _ => Err(format!("--header expects 'Name: value', not {s:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        super::parse(args.iter().map(|s| s.to_string()), &|_| None)
    }

    fn run(args: &[&str]) -> Result<Options, String> {
        match parse(args)? {
            Command::Run(options) => Ok(*options),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_url_alone_uses_the_defaults() {
        let options = run(&["https://example.com/"]).unwrap();
        assert_eq!(options, Options::new("https://example.com/".into()));
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert_eq!(parse(&["https://x/", "--help"]), Ok(Command::Help));
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn every_option_is_read() {
        let options = run(&[
            "--wait=network-idle2",
            "--quiet",
            "250",
            "--wait-for",
            ".item",
            "--wait-until=window.done",
            "-t",
            "1.5",
            "--max-scrolls",
            "7",
            "-A",
            "bot/1",
            "--viewport",
            "390x844",
            "-H",
            "X-A: 1",
            "--header=X-B:two words ",
            "--cookie",
            "a=1",
            "--proxy",
            "http://proxy:3128",
            "--block",
            "*ads*",
            "--block",
            "*track*",
            "--realtime",
            "--visibility",
            "viewport",
            "--memory-limit",
            "64",
            "--max-dom-nodes=0",
            "--script-cache-dir",
            "/tmp/s",
            "-e",
            "document.title",
            "--json",
            "-v",
            "--",
            "-weird-url",
        ])
        .unwrap();
        assert_eq!(options.url, "-weird-url");
        assert_eq!(options.wait, Wait::NetworkIdle2);
        assert_eq!(options.quiet, Duration::from_millis(250));
        assert_eq!(options.wait_for.as_deref(), Some(".item"));
        assert_eq!(options.wait_until.as_deref(), Some("window.done"));
        assert_eq!(options.timeout, Duration::from_millis(1500));
        assert!(options.scroll);
        assert_eq!(options.max_scrolls, Some(7));
        assert_eq!(options.user_agent.as_deref(), Some("bot/1"));
        assert_eq!(options.viewport, Some((390, 844)));
        assert_eq!(
            options.headers,
            [
                ("X-A".to_owned(), "1".to_owned()),
                ("X-B".to_owned(), "two words".to_owned())
            ]
        );
        assert_eq!(options.cookies, ["a=1"]);
        assert_eq!(options.proxy.as_deref(), Some("http://proxy:3128"));
        assert_eq!(options.block, ["*ads*", "*track*"]);
        assert!(options.realtime && options.json && options.verbose);
        assert_eq!(options.visibility, Some(Visibility::Viewport));
        assert_eq!(options.memory_limit, Some(64));
        assert_eq!(options.max_dom_nodes, Some(0));
        assert_eq!(options.script_cache_dir.as_deref(), Some("/tmp/s".as_ref()));
        assert_eq!(options.eval.as_deref(), Some("document.title"));
    }

    #[test]
    fn service_modes_are_subcommands() {
        assert!(matches!(parse(&["serve"]), Ok(Command::Serve(_))));
        assert_eq!(parse(&["serve", "--help"]), Ok(Command::ServiceHelp));
        assert_eq!(parse(&["lambda"]), Ok(Command::Lambda));
        assert_eq!(parse(&["lambda", "-h"]), Ok(Command::ServiceHelp));
        assert!(
            parse(&["lambda", "x"])
                .unwrap_err()
                .contains("no arguments")
        );
        // Anywhere else, they are URLs.
        assert_eq!(run(&["--json", "serve"]).unwrap().url, "serve");
    }

    #[test]
    fn mistakes_are_explained() {
        let err = |args: &[&str]| run(args).unwrap_err();
        assert_eq!(err(&[]), "no URL given");
        assert_eq!(err(&["a", "b"]), "only one URL may be given");
        assert!(err(&["--visibility", "some", "x"]).contains("expected loaded, all or viewport"));
        assert_eq!(err(&["--bogus", "u"]), "unknown option --bogus");
        assert_eq!(err(&["u", "--eval"]), "--eval needs an expression");
        assert_eq!(err(&["u", "--json=yes"]), "--json takes no value");
        assert!(err(&["u", "--wait", "soon"]).contains("unknown --wait strategy"));
        assert!(err(&["u", "--timeout", "0"]).contains("seconds"));
        assert!(err(&["u", "--timeout", "-1"]).contains("seconds"));
        assert!(err(&["u", "--quiet", "1.5"]).contains("whole number"));
        assert!(err(&["u", "--viewport", "100"]).contains("WIDTHxHEIGHT"));
        assert!(err(&["u", "--viewport", "0x10"]).contains("WIDTHxHEIGHT"));
        assert!(err(&["u", "-H", "no colon"]).contains("Name: value"));
    }
}
