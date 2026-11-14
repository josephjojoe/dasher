//! `dasher serve` and `dasher lambda`: dasher as a service.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use dasher::{ScriptCache, ScriptCacheOptions};

use dasher_serve::{Config, LambdaConfig, RUNTIME_API_ENV, Server};

/// Set to `true` to let pages reach private addresses.
const ALLOW_PRIVATE_ADDRESSES_ENV: &str = "DASHER_ALLOW_PRIVATE_ADDRESSES";

pub const USAGE: &str = "\
usage: dasher serve [options]
       dasher lambda

serve runs an HTTP service: POST a JSON job to /scrape and get JSON
back; GET /health reports on it. SIGTERM stops it gracefully.

lambda runs as an AWS Lambda custom runtime, taking the same jobs as
invocation events, or as HTTP requests from a function URL or API
Gateway. It is also what a binary named `bootstrap` does.

A job is a JSON object; only url is required:
  {\"url\": \"https://example.com/\", \"wait_for\": \".item\",
   \"extract\": {\"items\": {\"selector\": \".item\", \"all\": true}}}

Options for serve (environment variables in brackets):
      --host <addr>              address to listen on [default: 0.0.0.0]
      --port <n>                 port to listen on [$PORT; default: 8080]
      --max-concurrency <n>      jobs run at once [$DASHER_MAX_CONCURRENCY;
                                 default: 4]
      --max-queue <n>            jobs that may wait for a free slot; more
                                 get 503 [$DASHER_MAX_QUEUE; default: 32]
      --max-timeout <secs>       cap on a job's timeout_ms
                                 [$DASHER_MAX_TIMEOUT; default: 300]
      --shutdown-timeout <secs>  how long running jobs get to finish after
                                 SIGTERM [default: 8]
      --auth-token <token>       require 'Authorization: Bearer <token>'
                                 [$DASHER_AUTH_TOKEN]
      --allow-private-addresses  let pages connect to loopback, link-local
                                 and private addresses, which are refused
                                 by default; for testing against local
                                 servers [$DASHER_ALLOW_PRIVATE_ADDRESSES]
      --script-cache <MiB>       memory for compiled scripts that later
                                 jobs reuse; 0 turns the cache off
                                 [$DASHER_SCRIPT_CACHE_MB; default: 32]
      --script-cache-dir <dir>   keep compiled scripts in <dir> as well, so
                                 a new process starts with them
                                 [$DASHER_SCRIPT_CACHE_DIR]
  -h, --help                     print this help

lambda reads $DASHER_ALLOW_PRIVATE_ADDRESSES, $DASHER_SCRIPT_CACHE_MB and
$DASHER_SCRIPT_CACHE_DIR too.";

/// Options for `dasher serve`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServeOptions {
    pub host: IpAddr,
    pub port: u16,
    pub max_concurrency: usize,
    pub max_queue: usize,
    pub max_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub auth_token: Option<String>,
    pub allow_private_addresses: bool,
    pub script_cache: ScriptCacheSettings,
}

/// The script cache's size and directory, from options or the
/// environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptCacheSettings {
    /// In MiB; 0 turns the cache off.
    pub mib: usize,
    pub dir: Option<PathBuf>,
}

impl Default for ScriptCacheSettings {
    fn default() -> Self {
        Self {
            mib: dasher::DEFAULT_SCRIPT_CACHE_BYTES / (1024 * 1024),
            dir: None,
        }
    }
}

impl ScriptCacheSettings {
    fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut settings = Self::default();
        let from_env = |var: &str| env(var).filter(|v| !v.trim().is_empty());
        if let Some(mib) = from_env("DASHER_SCRIPT_CACHE_MB") {
            settings.mib = number("$DASHER_SCRIPT_CACHE_MB", &mib)?;
        }
        settings.dir = from_env("DASHER_SCRIPT_CACHE_DIR").map(PathBuf::from);
        Ok(settings)
    }

    /// The cache these describe; none with neither memory nor a directory.
    pub fn cache(&self) -> Option<ScriptCache> {
        if self.mib == 0 && self.dir.is_none() {
            return None;
        }
        Some(ScriptCache::new(ScriptCacheOptions {
            memory_bytes: self.mib.saturating_mul(1024 * 1024),
            dir: self.dir.clone(),
            ..ScriptCacheOptions::default()
        }))
    }
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            host: IpAddr::from([0, 0, 0, 0]),
            port: 8080,
            max_concurrency: dasher_serve::DEFAULT_MAX_CONCURRENCY,
            max_queue: dasher_serve::DEFAULT_MAX_QUEUE,
            max_timeout: dasher_serve::DEFAULT_MAX_TIMEOUT,
            shutdown_timeout: dasher_serve::DEFAULT_SHUTDOWN_TIMEOUT,
            auth_token: None,
            allow_private_addresses: false,
            script_cache: ScriptCacheSettings::default(),
        }
    }
}

/// What `dasher serve …` asks for: `None` for help.
pub fn parse_serve(
    args: impl IntoIterator<Item = String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<ServeOptions>, String> {
    let mut options = ServeOptions::default();
    // The environment first, so options override it.
    let from_env = |var: &str| env(var).filter(|v| !v.trim().is_empty());
    if let Some(port) = from_env("PORT") {
        options.port = number("$PORT", &port)?;
    }
    if let Some(n) = from_env("DASHER_MAX_CONCURRENCY") {
        options.max_concurrency = positive("$DASHER_MAX_CONCURRENCY", &n)?;
    }
    if let Some(n) = from_env("DASHER_MAX_QUEUE") {
        options.max_queue = number("$DASHER_MAX_QUEUE", &n)?;
    }
    if let Some(secs) = from_env("DASHER_MAX_TIMEOUT") {
        options.max_timeout = seconds("$DASHER_MAX_TIMEOUT", &secs)?;
    }
    options.auth_token = from_env("DASHER_AUTH_TOKEN");
    options.allow_private_addresses = allow_private_addresses(env)?;
    options.script_cache = ScriptCacheSettings::from_env(env)?;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
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
            "-h" | "--help" => return Ok(None),
            "--host" => {
                let host = value("an address")?;
                options.host = host
                    .parse()
                    .map_err(|_| format!("--host expects an IP address, not {host:?}"))?;
            }
            "--port" => options.port = number(&name, &value("a port")?)?,
            "--max-concurrency" => options.max_concurrency = positive(&name, &value("a count")?)?,
            "--max-queue" => options.max_queue = number(&name, &value("a count")?)?,
            "--max-timeout" => options.max_timeout = seconds(&name, &value("seconds")?)?,
            "--shutdown-timeout" => options.shutdown_timeout = seconds(&name, &value("seconds")?)?,
            "--auth-token" => options.auth_token = Some(value("a token")?),
            "--allow-private-addresses" => options.allow_private_addresses = true,
            "--script-cache" => options.script_cache.mib = number(&name, &value("MiB")?)?,
            "--script-cache-dir" => options.script_cache.dir = Some(value("a directory")?.into()),
            _ => return Err(format!("unknown serve option {name}")),
        }
        if inline.is_some() {
            return Err(format!("{name} takes no value"));
        }
    }
    Ok(Some(options))
}

/// Whether `$DASHER_ALLOW_PRIVATE_ADDRESSES` turns off the private
/// address guard.
fn allow_private_addresses(env: &dyn Fn(&str) -> Option<String>) -> Result<bool, String> {
    let Some(value) = env(ALLOW_PRIVATE_ADDRESSES_ENV) else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "no" | "off" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        _ => Err(format!(
            "${ALLOW_PRIVATE_ADDRESSES_ENV} expects true or false, not {value:?}"
        )),
    }
}

fn number<T: std::str::FromStr>(name: &str, s: &str) -> Result<T, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{name} expects a whole number, not {s:?}"))
}

fn positive(name: &str, s: &str) -> Result<usize, String> {
    number(name, s)
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("{name} expects a positive whole number, not {s:?}"))
}

fn seconds(name: &str, s: &str) -> Result<Duration, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| format!("{name} expects a number of seconds, not {s:?}"))
}

/// Runs the HTTP service until SIGTERM or Ctrl-C.
pub fn serve(options: &ServeOptions) -> ExitCode {
    let config = Config {
        max_concurrency: options.max_concurrency,
        max_queue: options.max_queue,
        max_timeout: options.max_timeout,
        auth_token: options.auth_token.clone(),
        shutdown_timeout: options.shutdown_timeout,
        block_private_addresses: !options.allow_private_addresses,
        script_cache: options.script_cache.cache(),
        ..Config::default()
    };
    let addr = SocketAddr::new(options.host, options.port);
    block_on(async move {
        let server = Server::bind(addr, config)
            .await
            .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        server
            .run(dasher_serve::shutdown_signal())
            .await
            .map_err(|e| e.to_string())
    })
}

/// Runs as a Lambda custom runtime.
pub fn lambda() -> ExitCode {
    let Some(api) = std::env::var(RUNTIME_API_ENV)
        .ok()
        .filter(|v| !v.is_empty())
    else {
        eprintln!("dasher: ${RUNTIME_API_ENV} is not set; `dasher lambda` runs inside AWS Lambda");
        return ExitCode::from(2);
    };
    let env = |name: &str| std::env::var(name).ok();
    let allow_private = match allow_private_addresses(&env) {
        Ok(allow) => allow,
        Err(message) => {
            eprintln!("dasher: {message}");
            return ExitCode::from(2);
        }
    };
    let script_cache = match ScriptCacheSettings::from_env(&env) {
        Ok(settings) => settings.cache(),
        Err(message) => {
            eprintln!("dasher: {message}");
            return ExitCode::from(2);
        }
    };
    let config = LambdaConfig {
        block_private_addresses: !allow_private,
        script_cache,
        ..LambdaConfig::default()
    };
    block_on(async move {
        dasher_serve::run_lambda(&api, config)
            .await
            .map_err(|e| format!("the Lambda Runtime API failed: {e}"))
    })
}

/// Runs `f` on a current-thread runtime, as the service and pages need.
fn block_on(f: impl Future<Output = Result<(), String>>) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("dasher: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(f) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("dasher: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<Option<ServeOptions>, String> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let lookup = move |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        parse_serve(args.iter().map(|s| s.to_string()), &lookup)
    }

    #[test]
    fn defaults_suit_cloud_run() {
        let options = parse(&[], &[]).unwrap().unwrap();
        assert_eq!(options, ServeOptions::default());
        assert_eq!(
            SocketAddr::new(options.host, options.port).to_string(),
            "0.0.0.0:8080"
        );
    }

    #[test]
    fn options_override_the_environment() {
        let env = [
            ("PORT", "9000"),
            ("DASHER_MAX_CONCURRENCY", "2"),
            ("DASHER_MAX_QUEUE", "0"),
            ("DASHER_MAX_TIMEOUT", "60"),
            ("DASHER_AUTH_TOKEN", "tok"),
            ("DASHER_SCRIPT_CACHE_MB", "8"),
            ("DASHER_SCRIPT_CACHE_DIR", "/tmp/scripts"),
        ];
        let options = parse(&[], &env).unwrap().unwrap();
        assert_eq!(
            options.script_cache,
            ScriptCacheSettings {
                mib: 8,
                dir: Some("/tmp/scripts".into())
            }
        );
        assert!(!options.allow_private_addresses);
        assert_eq!(options.port, 9000);
        assert_eq!(options.max_concurrency, 2);
        assert_eq!(options.max_queue, 0);
        assert_eq!(options.max_timeout, Duration::from_secs(60));
        assert_eq!(options.auth_token.as_deref(), Some("tok"));

        let options = parse(
            &[
                "--host",
                "127.0.0.1",
                "--port=9001",
                "--max-concurrency",
                "8",
                "--max-queue",
                "4",
                "--max-timeout",
                "90",
                "--shutdown-timeout",
                "2.5",
                "--auth-token",
                "other",
                "--script-cache=0",
                "--script-cache-dir",
                "/var/cache/dasher",
            ],
            &env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.host, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(options.port, 9001);
        assert_eq!(options.max_concurrency, 8);
        assert_eq!(options.max_queue, 4);
        assert_eq!(options.max_timeout, Duration::from_secs(90));
        assert_eq!(options.shutdown_timeout, Duration::from_millis(2500));
        assert_eq!(options.auth_token.as_deref(), Some("other"));
        assert!(!options.allow_private_addresses);
        assert_eq!(options.script_cache.mib, 0);
        assert_eq!(options.script_cache.dir.as_deref(), Some("/var/cache/dasher".as_ref()));
        // memory off, but still a directory
        assert_eq!(options.script_cache.cache().unwrap().options().memory_bytes, 0);
        assert!(ScriptCacheSettings { mib: 0, dir: None }.cache().is_none());

        let allowed = |args: &[&str], value: &str| {
            parse(args, &[("DASHER_ALLOW_PRIVATE_ADDRESSES", value)])
                .unwrap()
                .unwrap()
                .allow_private_addresses
        };
        assert!(allowed(&["--allow-private-addresses"], ""));
        assert!(allowed(&[], "true"));
        assert!(allowed(&[], "1"));
        assert!(!allowed(&[], "false"));
        assert!(!allowed(&[], ""));
    }

    #[test]
    fn mistakes_are_explained() {
        let err = |args: &[&str], env: &[(&str, &str)]| parse(args, env).unwrap_err();
        assert_eq!(parse(&["--help"], &[]), Ok(None));
        assert!(err(&["--bogus"], &[]).contains("unknown serve option"));
        assert!(err(&["--port"], &[]).contains("needs a port"));
        assert!(err(&["--port", "99999"], &[]).contains("whole number"));
        assert!(err(&["--host", "localhost"], &[]).contains("IP address"));
        assert!(err(&["--max-concurrency", "0"], &[]).contains("positive"));
        assert!(err(&["--max-timeout", "0"], &[]).contains("seconds"));
        assert!(err(&[], &[("PORT", "http")]).contains("$PORT"));
        assert!(err(&[], &[("DASHER_SCRIPT_CACHE_MB", "lots")]).contains("$DASHER_SCRIPT_CACHE_MB"));
        assert!(err(&["--script-cache", "-1"], &[]).contains("whole number"));
        assert!(
            err(&[], &[("DASHER_ALLOW_PRIVATE_ADDRESSES", "maybe")])
                .contains("$DASHER_ALLOW_PRIVATE_ADDRESSES")
        );
        assert!(err(&["--allow-private-addresses=yes"], &[]).contains("takes no value"));
    }
}
