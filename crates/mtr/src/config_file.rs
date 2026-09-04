//! The user configuration file, `$XDG_CONFIG_HOME/mtr-rs/config.toml` (default
//! `~/.config/mtr-rs/config.toml`). It sits between the built-in defaults and `$MTR_OPTIONS`:
//!
//!   built-in defaults  <  config file  <  `$MTR_OPTIONS`  <  command line
//!
//! `build_argv` prepends the words of `$MTR_OPTIONS` to `argv`, so clap reports both of the two
//! upper layers as [`ValueSource::CommandLine`]; the file only fills the keys clap left at their
//! default. This file has no counterpart in C mtr. GPL-2.0-only.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::Args;
use crate::tui::palette::RttThresholds;

/// The header of the written file: what the layout is and how the file ranks against the other
/// sources. Kept verbatim in `docs/config.example.toml`.
const HEADER: &str = r##"# mtr-rs configuration file — ~/.config/mtr-rs/config.toml
# ($XDG_CONFIG_HOME/mtr-rs/config.toml when XDG_CONFIG_HOME is set; --config PATH overrides both.)
#
# Precedence, lowest to highest:
#   built-in defaults  <  this file  <  $MTR_OPTIONS  <  the command line
#
# Every key is optional and is shown below commented out, with its built-in default.
# Uncomment a key to change it.

[display]
"##;

/// `color = "auto" | "always" | "never"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    /// Colour unless `NO_COLOR` is set (the built-in behaviour).
    #[default]
    Auto,
    /// Colour even when `NO_COLOR` is set.
    Always,
    /// Never colour.
    Never,
}

impl ColorChoice {
    /// The spelling the file uses, i.e. the `serde(rename_all = "lowercase")` name.
    fn as_str(self) -> &'static str {
        match self {
            ColorChoice::Auto => "auto",
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySection {
    pub rtt_thresholds_ms: Option<Vec<i64>>,
    pub fields: Option<String>,
    pub ascii: Option<bool>,
    pub color: Option<ColorChoice>,
    pub sparkline: Option<bool>,
    pub detail_pane: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSection {
    pub interval: Option<f64>,
    pub gracetime: Option<f64>,
    pub max_ttl: Option<i64>,
    pub max_unknown: Option<i64>,
    pub timeout: Option<i64>,
    pub dns: Option<bool>,
    pub asn: Option<bool>,
}

/// Every key optional, so a partial file leaves the remaining defaults alone.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default)]
    pub display: DisplaySection,
    #[serde(default)]
    pub probe: ProbeSection,
}

/// A [`FileConfig`] that has passed [`FileConfig::validate`], carrying the values that needed
/// parsing to be checked at all — so `apply` never re-parses and never has an error to swallow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedConfig {
    pub file: FileConfig,
    pub rtt_thresholds: Option<RttThresholds>,
}

impl FileConfig {
    /// The same rules the matching command line flags enforce, applied at load time so a bad file
    /// is reported once, with its path, instead of as a confusing CLI error.
    pub fn validate(self) -> Result<LoadedConfig, String> {
        let rtt_thresholds = parse_rtt_thresholds_ms(self.display.rtt_thresholds_ms.as_deref())?;
        if let Some(f) = &self.display.fields {
            mtr_core::fields::validate_fields(f)?;
        }
        if let Some(i) = self.probe.interval {
            crate::cli::validate_seconds("wait time", i)?;
        }
        if let Some(g) = self.probe.gracetime {
            crate::cli::validate_seconds("grace time", g)?;
        }
        if let Some(t) = self.probe.timeout
            && t < 1
        {
            return Err("timeout must be positive".to_string());
        }
        if let Some(m) = self.probe.max_ttl
            && !(1..=255).contains(&m)
        {
            return Err(format!("value out of range (1 - 255): {m}"));
        }
        if let Some(u) = self.probe.max_unknown
            && u < 1
        {
            return Err(format!("max_unknown must be at least 1: {u}"));
        }
        Ok(LoadedConfig {
            file: self,
            rtt_thresholds,
        })
    }
}

fn parse_rtt_thresholds_ms(v: Option<&[i64]>) -> Result<Option<RttThresholds>, String> {
    let Some(v) = v else {
        return Ok(None);
    };
    let ms: Vec<u64> = v
        .iter()
        .map(|&n| crate::cli::rtt_threshold_ms(n))
        .collect::<Result<_, _>>()?;
    RttThresholds::from_millis(&ms).map(Some)
}

/// `$XDG_CONFIG_HOME/mtr-rs/config.toml`, else `$HOME/.config/mtr-rs/config.toml`. `None` when
/// neither variable is usable (an empty or relative `XDG_CONFIG_HOME` is ignored, as the spec
/// requires).
pub fn default_path() -> Option<PathBuf> {
    let abs = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = match abs("XDG_CONFIG_HOME") {
        Some(p) => p,
        None => abs("HOME")?.join(".config"),
    };
    Some(base.join("mtr-rs").join("config.toml"))
}

/// `--config PATH` when given, otherwise [`default_path`].
pub fn resolve_path(explicit: Option<&str>) -> Option<PathBuf> {
    match explicit {
        Some(p) => Some(PathBuf::from(p)),
        None => default_path(),
    }
}

/// Which file to read, guarded by the sudo marker exactly as `-F` is (ui/mtr.c:717-721). Under
/// sudo the process is root but every input here is chosen by the unprivileged caller: an explicit
/// `--config` would name any file for a root-privileged read, and `$HOME`/`$XDG_CONFIG_HOME` come
/// from the same untrusted environment. So refuse the flag and read nothing by default.
pub fn config_source(
    explicit: Option<&str>,
    guard_present: bool,
) -> Result<Option<PathBuf>, String> {
    if guard_present {
        return match explicit {
            Some(_) => Err("--config is disabled under sudo.".to_string()),
            None => Ok(None),
        };
    }
    Ok(resolve_path(explicit))
}

/// An absent file is not an error and yields the all-`None` config; anything else — unreadable,
/// malformed, or failing validation — is fatal. The returned message is already prefixed with the
/// path, so callers print `mtr: config: {msg}`.
pub fn load(path: &Path) -> Result<LoadedConfig, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LoadedConfig::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let cfg: FileConfig = toml::from_str(&text)
        .map_err(|e| format!("{}: {}", path.display(), toml_message(&text, &e)))?;
    cfg.validate()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `toml`'s own `Display` quotes the offending source line. That must never reach the terminal:
/// the file is named by the caller but read by the process, so echoing it would turn a parse error
/// into a file-disclosure primitive. Rebuild a single-line message from the error text plus the
/// location, so nothing from the file itself is printed.
fn toml_message(text: &str, e: &toml::de::Error) -> String {
    let Some(span) = e.span() else {
        return e.message().to_string();
    };
    let before = &text[..span.start.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |l| l.chars().count() + 1);
    format!("{} at line {line} column {column}", e.message())
}

/// The values `--init-config` writes: one field per configuration key, holding the value that is
/// actually in effect — the built-in default, or whatever the existing file, `$MTR_OPTIONS` or
/// the command line put in its place.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveConfig {
    pub rtt_thresholds: RttThresholds,
    pub fields: String,
    pub ascii: bool,
    pub color: ColorChoice,
    pub sparkline: bool,
    pub detail_pane: bool,
    pub interval: f64,
    pub gracetime: f64,
    pub max_ttl: i64,
    pub max_unknown: i64,
    pub timeout: i64,
    pub dns: bool,
    pub asn: bool,
}

/// The built-in defaults, i.e. the clap defaults of the matching flags. A test asserts the two
/// stay in step.
impl Default for EffectiveConfig {
    fn default() -> Self {
        Self {
            rtt_thresholds: RttThresholds::default(),
            fields: "LS NABWV".to_string(),
            ascii: false,
            color: ColorChoice::Auto,
            sparkline: true,
            detail_pane: true,
            interval: 1.0,
            gracetime: 5.0,
            max_ttl: 30,
            max_unknown: 12,
            timeout: 10,
            dns: true,
            asn: false,
        }
    }
}

impl EffectiveConfig {
    /// The rendered file has to load again, so run it through the same checks [`load`] applies —
    /// `-o LSQ` or `-i nan` are only rejected by [`Args::into_options`], which `--init-config`
    /// never reaches.
    fn validate(&self) -> Result<(), String> {
        FileConfig::from(self).validate().map(|_| ())
    }
}

impl From<&EffectiveConfig> for FileConfig {
    fn from(c: &EffectiveConfig) -> Self {
        FileConfig {
            display: DisplaySection {
                rtt_thresholds_ms: Some(
                    c.rtt_thresholds
                        .to_millis()
                        .iter()
                        .map(|&m| m as i64)
                        .collect(),
                ),
                fields: Some(c.fields.clone()),
                ascii: Some(c.ascii),
                color: Some(c.color),
                sparkline: Some(c.sparkline),
                detail_pane: Some(c.detail_pane),
            },
            probe: ProbeSection {
                interval: Some(c.interval),
                gracetime: Some(c.gracetime),
                max_ttl: Some(c.max_ttl),
                max_unknown: Some(c.max_unknown),
                timeout: Some(c.timeout),
                dns: Some(c.dns),
                asn: Some(c.asn),
            },
        }
    }
}

/// The options in effect at the point `run()` calls it: [`apply`] has already merged the existing
/// file into `args`, and clap merged `$MTR_OPTIONS` and the command line before that. The colour
/// resolution mirrors [`Args::into_options`], minus the `NO_COLOR` lookup — that is a property of
/// the terminal the file is written on, not a setting to save.
pub fn effective_from_args(args: &Args) -> EffectiveConfig {
    EffectiveConfig {
        rtt_thresholds: args.rtt_thresholds.unwrap_or_default(),
        fields: args
            .order
            .clone()
            .unwrap_or_else(|| EffectiveConfig::default().fields),
        ascii: args.ascii,
        color: match (args.color, args.no_color) {
            (Some(c), _) => c,
            (None, true) => ColorChoice::Never,
            (None, false) => args.color_choice.unwrap_or_default(),
        },
        sparkline: args.sparkline,
        detail_pane: args.detail_pane,
        interval: args.interval,
        gracetime: args.gracetime,
        max_ttl: args.max_ttl,
        max_unknown: args.max_unknown,
        timeout: args.timeout,
        dns: !args.no_dns,
        asn: args.aslookup,
    }
}

/// `1.0`, not `1`: an integer would deserialise into `Option<f64>` as a TOML type error.
fn toml_float(v: f64) -> String {
    format!("{v:?}")
}

/// Basic strings, escaped — the values are validated field specs today, but the rendering must
/// not be the thing that turns an odd one into a malformed file.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A key is written commented out while it holds its built-in default, and uncommented once
/// something changed it — so `--init-config` alone reproduces the documented example, and
/// `--init-config -i 2` saves a file that actually sets the interval.
fn key(out: &mut String, name: &str, value: String, is_default: bool) {
    if is_default {
        out.push('#');
    }
    out.push_str(name);
    out.push_str(" = ");
    out.push_str(&value);
    out.push('\n');
}

/// The file `--init-config` writes; `render(&EffectiveConfig::default())` is, byte for byte (see
/// the test below), `docs/config.example.toml`.
pub fn render(cfg: &EffectiveConfig) -> String {
    let d = EffectiveConfig::default();
    let mut s = String::from(HEADER);
    s.push_str(
        "# The four upper bounds of the RTT colour ramp, in milliseconds: below the first is green, then\n\
         # yellow, magenta and red, and at or above the last, bold red. Same as --rtt-thresholds.\n",
    );
    let ms = cfg.rtt_thresholds.to_millis();
    let list = ms.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
    key(
        &mut s,
        "rtt_thresholds_ms",
        format!("[{list}]"),
        cfg.rtt_thresholds == d.rtt_thresholds,
    );
    s.push_str("# The columns to show, using the field letters of -o.\n");
    key(
        &mut s,
        "fields",
        toml_string(&cfg.fields),
        cfg.fields == d.fields,
    );
    s.push_str("# ASCII glyphs and borders instead of Unicode ones (--ascii).\n");
    key(&mut s, "ascii", cfg.ascii.to_string(), cfg.ascii == d.ascii);
    s.push_str(
        "# \"auto\" colours unless NO_COLOR is set, \"always\" colours even then, \"never\" is --no-color.\n",
    );
    key(
        &mut s,
        "color",
        toml_string(cfg.color.as_str()),
        cfg.color == d.color,
    );
    s.push_str("# Show the Recent sparkline column when the TUI starts (toggled with d).\n");
    key(
        &mut s,
        "sparkline",
        cfg.sparkline.to_string(),
        cfg.sparkline == d.sparkline,
    );
    s.push_str("# Open the detail pane when the TUI starts (toggled with Enter).\n");
    key(
        &mut s,
        "detail_pane",
        cfg.detail_pane.to_string(),
        cfg.detail_pane == d.detail_pane,
    );
    s.push_str("\n[probe]\n");
    s.push_str("# Seconds between probe cycles (-i). Values below 1.0 need root.\n");
    key(
        &mut s,
        "interval",
        toml_float(cfg.interval),
        cfg.interval == d.interval,
    );
    s.push_str("# Seconds to wait for late replies after the last cycle (-G).\n");
    key(
        &mut s,
        "gracetime",
        toml_float(cfg.gracetime),
        cfg.gracetime == d.gracetime,
    );
    s.push_str("# The highest TTL probed (-m).\n");
    key(
        &mut s,
        "max_ttl",
        cfg.max_ttl.to_string(),
        cfg.max_ttl == d.max_ttl,
    );
    s.push_str("# Consecutive unanswered hops before the scan stops (-U).\n");
    key(
        &mut s,
        "max_unknown",
        cfg.max_unknown.to_string(),
        cfg.max_unknown == d.max_unknown,
    );
    s.push_str("# Seconds a probe may stay outstanding before it counts as lost (-Z).\n");
    key(
        &mut s,
        "timeout",
        cfg.timeout.to_string(),
        cfg.timeout == d.timeout,
    );
    s.push_str("# Resolve host names; false is -n.\n");
    key(&mut s, "dns", cfg.dns.to_string(), cfg.dns == d.dns);
    s.push_str("# Look up origin AS numbers; true is -z.\n");
    key(&mut s, "asn", cfg.asn.to_string(), cfg.asn == d.asn);
    s
}

/// Where `--init-config` writes: the explicit `--config` path or the XDG default. Refused under
/// the sudo guard (the helper file /etc/mtr.is.run.under.sudo) like `--config` and `-F`, since
/// creating files as root at a caller-chosen or `$HOME`-derived path is exactly what the guard
/// prevents.
pub fn init_config_target(explicit: Option<&str>, guard_present: bool) -> Result<PathBuf, String> {
    if guard_present {
        return Err("--init-config is disabled under sudo.".to_string());
    }
    match explicit {
        Some(p) => Ok(PathBuf::from(p)),
        None => default_path()
            .ok_or_else(|| "no path: set $HOME or $XDG_CONFIG_HOME, or pass --config".to_string()),
    }
}

/// `--init-config`: create the parent directories and write [`render`]'s output, refusing to
/// touch an existing file. The message is path-prefixed like [`load`]'s, except for the
/// validation error, which is about the options and not about the file.
pub fn init(path: &Path, cfg: &EffectiveConfig) -> Result<(), String> {
    cfg.validate()?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!(
                "{}: {} exists, refusing to overwrite it",
                path.display(),
                if path.is_dir() { "directory" } else { "file" }
            ),
            _ => format!("{}: {e}", path.display()),
        })?;
    f.write_all(render(cfg).as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Push the file's values into `args` for every key the command line (or `$MTR_OPTIONS`) left
/// alone. Called between parsing and [`Args::into_options`], so the file's values then go through
/// exactly the same validation and conversion as the flags they stand in for.
pub fn apply(args: &mut Args, cfg: &LoadedConfig) {
    let file = &cfg.file;
    let unset = |id: &str| !args.cli_set.contains(id);
    if unset("rtt_thresholds")
        && let Some(t) = cfg.rtt_thresholds
    {
        args.rtt_thresholds = Some(t);
    }
    if unset("order")
        && let Some(f) = &file.display.fields
    {
        args.order = Some(f.clone());
    }
    if unset("ascii")
        && let Some(a) = file.display.ascii
    {
        args.ascii = a;
    }
    if unset("color")
        && let Some(c) = file.display.color
    {
        args.color_choice = Some(c);
    }
    if let Some(s) = file.display.sparkline {
        args.sparkline = s;
    }
    if let Some(d) = file.display.detail_pane {
        args.detail_pane = d;
    }
    if unset("interval")
        && let Some(i) = file.probe.interval
    {
        args.interval = i;
    }
    if unset("gracetime")
        && let Some(g) = file.probe.gracetime
    {
        args.gracetime = g;
    }
    if unset("max_ttl")
        && let Some(m) = file.probe.max_ttl
    {
        args.max_ttl = m;
    }
    if unset("max_unknown")
        && let Some(u) = file.probe.max_unknown
    {
        args.max_unknown = u;
    }
    if unset("timeout")
        && let Some(t) = file.probe.timeout
    {
        args.timeout = t;
    }
    if unset("no_dns")
        && let Some(d) = file.probe.dns
    {
        args.no_dns = !d;
    }
    if unset("aslookup")
        && let Some(z) = file.probe.asn
    {
        args.aslookup = z;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::cli::{Options, build_argv};

    #[test]
    fn validate_rejects_non_finite_interval_and_gracetime() {
        let mut f = FileConfig::default();
        f.probe.interval = Some(f64::INFINITY);
        assert_eq!(f.validate().unwrap_err(), "wait time must be positive");
        let mut f = FileConfig::default();
        f.probe.gracetime = Some(f64::NAN);
        assert_eq!(f.validate().unwrap_err(), "grace time must be positive");
    }

    /// A `config.toml` in a private temp directory, removed again when `f` returns.
    fn with_file<R>(text: &str, f: impl FnOnce(&Path) -> R) -> R {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mtr-rs-cfg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        let r = f(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        r
    }

    fn parse(text: &str) -> Result<LoadedConfig, String> {
        with_file(text, |path| {
            load(path).map_err(|e| e.replace(&format!("{}: ", path.display()), ""))
        })
    }

    /// The whole chain: file → `$MTR_OPTIONS` → command line → [`Options`].
    fn opts(text: &str, env: Option<&str>, args: &[&str]) -> Options {
        with_file(text, |path| {
            let argv = build_argv(env, args.iter().map(|s| s.to_string())).unwrap();
            let mut a = Args::parse_argv(argv).unwrap();
            apply(&mut a, &load(path).unwrap());
            a.into_options(true).unwrap()
        })
    }

    #[test]
    fn the_command_line_beats_mtr_options_beats_the_file() {
        let file = "[probe]\ninterval = 2.0\n";
        assert_eq!(opts("", None, &["h"]).config.interval, 1.0);
        assert_eq!(opts(file, None, &["h"]).config.interval, 2.0);
        assert_eq!(opts(file, Some("-i 3"), &["h"]).config.interval, 3.0);
        assert_eq!(
            opts(file, Some("-i 3"), &["-i", "4", "h"]).config.interval,
            4.0
        );
        assert_eq!(opts(file, None, &["-i", "4", "h"]).config.interval, 4.0);
    }

    #[test]
    fn every_key_reaches_options() {
        let o = opts(
            r#"
[display]
rtt_thresholds_ms = [5, 10, 20, 40]
fields = "LSNB"
ascii = true
color = "never"
sparkline = false
detail_pane = false
[probe]
interval = 2.5
gracetime = 2.0
max_ttl = 20
max_unknown = 3
timeout = 7
dns = false
asn = true
"#,
            None,
            &["h"],
        );
        assert_eq!(o.rtt_thresholds.to_millis(), [5, 10, 20, 40]);
        assert_eq!(o.config.fields, "LSNB");
        assert!(o.ascii && !o.color && !o.sparkline && !o.detail_pane);
        assert_eq!(o.config.interval, 2.5);
        assert_eq!(o.config.grace_time, 2.0);
        assert_eq!(o.config.max_ttl, 20);
        assert_eq!(o.config.max_unknown, 3);
        assert_eq!(o.config.probe_timeout, std::time::Duration::from_secs(7));
        assert!(!o.config.dns);
        assert_eq!(o.config.ipinfo_fields, vec![0]);
    }

    #[test]
    fn defaults_survive_an_empty_file_and_the_cli_overrides_each_key() {
        let o = opts("", None, &["h"]);
        assert_eq!(o.rtt_thresholds, RttThresholds::default());
        assert_eq!(o.config.fields, "LS NABWV");
        assert!(!o.ascii && o.sparkline && o.detail_pane);
        assert!(o.config.dns && o.config.ipinfo_fields.is_empty());
        assert_eq!((o.config.max_ttl, o.config.max_unknown), (30, 12));

        let file = "[display]\nascii = true\nfields = \"LSNB\"\nrtt_thresholds_ms = [5, 10, 20, 40]\n\
                    [probe]\ndns = false\nasn = true\nmax_ttl = 20\nmax_unknown = 3\ntimeout = 7\n";
        let o = opts(
            file,
            None,
            &[
                "-o",
                "LSNA",
                "-n",
                "-m",
                "9",
                "-U",
                "4",
                "-Z",
                "2",
                "--rtt-thresholds",
                "1,2,3,4",
                "h",
            ],
        );
        assert_eq!(o.config.fields, "LSNA");
        assert_eq!(o.rtt_thresholds.to_millis(), [1, 2, 3, 4]);
        assert_eq!((o.config.max_ttl, o.config.max_unknown), (9, 4));
        assert_eq!(o.config.probe_timeout, std::time::Duration::from_secs(2));
        assert!(!o.config.dns);
        // `--ascii` is a flag: the file can turn it on, the CLI can only add to that
        assert!(opts(file, None, &["h"]).ascii);
        assert!(opts("[display]\nascii = false\n", None, &["--ascii", "h"]).ascii);
        // `dns = true` in the file loses to `-n`
        assert!(!opts("[probe]\ndns = true\n", None, &["-n", "h"]).config.dns);
        assert!(!opts("[probe]\ndns = false\n", None, &["h"]).config.dns);
    }

    #[test]
    fn color_choice_maps_to_the_colour_flag() {
        // `never` is the file saying `--no-color`; `--no-color` on the command line still wins
        assert!(!opts("[display]\ncolor = \"never\"\n", None, &["h"]).color);
        assert!(
            !opts(
                "[display]\ncolor = \"always\"\n",
                None,
                &["--no-color", "h"]
            )
            .color
        );
        assert!(opts("[display]\ncolor = \"always\"\n", None, &["h"]).color);
        // …and `--color` can undo the file in either direction, which `--no-color` alone cannot
        let never = "[display]\ncolor = \"never\"\n";
        let always = "[display]\ncolor = \"always\"\n";
        assert!(opts(never, None, &["--color", "always", "h"]).color);
        assert!(!opts(always, None, &["--color", "never", "h"]).color);
        assert!(opts(never, Some("--color always"), &["h"]).color);
        // an explicit `--color` beats a `--no-color` given alongside it
        assert!(opts("", None, &["--no-color", "--color", "always", "h"]).color);
    }

    #[test]
    fn the_sudo_guard_disables_config_the_way_it_disables_dash_f() {
        // no guard: both the flag and the environment-derived default are honoured
        assert_eq!(
            config_source(Some("/tmp/x.toml"), false).unwrap(),
            Some(PathBuf::from("/tmp/x.toml"))
        );
        assert_eq!(config_source(None, false).unwrap(), default_path());
        // guard present: the flag is refused outright and nothing is read by default — the process
        // is root, but `--config`, `$HOME` and `$XDG_CONFIG_HOME` all come from the caller
        assert_eq!(
            config_source(Some("/etc/shadow"), true).unwrap_err(),
            "--config is disabled under sudo."
        );
        assert_eq!(config_source(None, true).unwrap(), None);
    }

    #[test]
    fn init_config_target_refuses_under_sudo_and_explains_no_path() {
        assert_eq!(
            init_config_target(None, true).unwrap_err(),
            "--init-config is disabled under sudo."
        );
        assert_eq!(
            init_config_target(Some("/tmp/x.toml"), true).unwrap_err(),
            "--init-config is disabled under sudo."
        );
        assert_eq!(
            init_config_target(Some("/tmp/x.toml"), false).unwrap(),
            PathBuf::from("/tmp/x.toml")
        );
    }

    #[test]
    fn parse_errors_never_echo_the_files_own_bytes() {
        let secret = "root:$6$super-secret-hash:19000:0:99999:7:::\n";
        let err = parse(secret).unwrap_err();
        assert!(!err.contains("secret"), "{err}");
        assert!(!err.contains("19000"), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
        assert!(err.contains("at line 1 column "), "{err}");
        // the location still points at the right line of a multi-line file
        let err = parse("[display]\nascii = true\nnope\n").unwrap_err();
        assert!(err.contains("at line 3 column "), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }

    #[test]
    fn a_full_file_parses_every_key() {
        let cfg = parse(
            r#"
[display]
rtt_thresholds_ms = [5, 10, 20, 40]
fields = "LSNB"
ascii = true
color = "never"
sparkline = false
detail_pane = false
[probe]
interval = 2.5
gracetime = 2.0
max_ttl = 20
max_unknown = 3
timeout = 7
dns = false
asn = true
"#,
        )
        .unwrap();
        assert_eq!(cfg.rtt_thresholds.unwrap().to_millis(), [5, 10, 20, 40]);
        let (d, pr) = (&cfg.file.display, &cfg.file.probe);
        assert_eq!(d.rtt_thresholds_ms, Some(vec![5, 10, 20, 40]));
        assert_eq!(d.fields.as_deref(), Some("LSNB"));
        assert_eq!(d.ascii, Some(true));
        assert_eq!(d.color, Some(ColorChoice::Never));
        assert_eq!(d.sparkline, Some(false));
        assert_eq!(d.detail_pane, Some(false));
        assert_eq!(pr.interval, Some(2.5));
        assert_eq!(pr.gracetime, Some(2.0));
        assert_eq!(pr.max_ttl, Some(20));
        assert_eq!(pr.max_unknown, Some(3));
        assert_eq!(pr.timeout, Some(7));
        assert_eq!(pr.dns, Some(false));
        assert_eq!(pr.asn, Some(true));
    }

    #[test]
    fn a_partial_file_leaves_the_other_keys_unset() {
        let cfg = parse("[probe]\ninterval = 2.0\n").unwrap();
        assert_eq!(cfg.file.probe.interval, Some(2.0));
        assert_eq!(cfg.file.probe.max_ttl, None);
        assert_eq!(cfg.file.display, DisplaySection::default());
        assert_eq!(parse("").unwrap(), LoadedConfig::default());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        assert_eq!(
            load(Path::new("/nonexistent/mtr-rs/config.toml")).unwrap(),
            LoadedConfig::default()
        );
    }

    #[test]
    fn malformed_and_invalid_files_are_errors() {
        assert!(
            parse("[display\nascii = true\n")
                .unwrap_err()
                .contains("at line 1")
        );
        assert_eq!(
            parse("[display]\nfields = \"LSQ\"\n").unwrap_err(),
            "Unknown field identifier: Q"
        );
        assert_eq!(
            parse("[display]\nrtt_thresholds_ms = [1, 2, 3]\n").unwrap_err(),
            "rtt thresholds need exactly 4 values, got 3"
        );
        assert_eq!(
            parse("[display]\nrtt_thresholds_ms = [40, 30, 20, 10]\n").unwrap_err(),
            "rtt thresholds must be ascending: 30 is not greater than 40"
        );
        assert_eq!(
            parse("[probe]\ninterval = 0.0\n").unwrap_err(),
            "wait time must be positive"
        );
        assert_eq!(
            parse("[probe]\ntimeout = 0\n").unwrap_err(),
            "timeout must be positive"
        );
        assert_eq!(
            parse("[probe]\nmax_ttl = 0\n").unwrap_err(),
            "value out of range (1 - 255): 0"
        );
        assert!(
            parse("[probe]\nnope = 1\n")
                .unwrap_err()
                .contains("unknown field")
        );
        assert!(
            parse("[display]\ncolor = \"pink\"\n")
                .unwrap_err()
                .contains("unknown variant")
        );
    }

    fn args(argv: &[&str]) -> Args {
        let mut v = vec!["mtr-rs".to_string()];
        v.extend(argv.iter().map(|s| s.to_string()));
        Args::parse_argv(v).unwrap()
    }

    #[test]
    fn the_default_effective_config_is_what_a_bare_command_line_produces() {
        assert_eq!(
            effective_from_args(&args(&["h"])),
            EffectiveConfig::default(),
            "EffectiveConfig::default() has drifted from the clap defaults"
        );
    }

    #[test]
    fn render_writes_the_changed_keys_uncommented_and_leaves_the_defaults_alone() {
        let cfg = effective_from_args(&args(&[
            "-i",
            "2",
            "-n",
            "--rtt-thresholds",
            "20,50,100,300",
            "h",
        ]));
        let text = render(&cfg);
        assert!(text.contains("\ninterval = 2.0\n"), "{text}");
        assert!(text.contains("\ndns = false\n"), "{text}");
        assert!(
            text.contains("\nrtt_thresholds_ms = [20, 50, 100, 300]\n"),
            "{text}"
        );
        // untouched keys stay commented out, with their built-in defaults
        assert!(text.contains("\n#gracetime = 5.0\n"), "{text}");
        assert!(text.contains("\n#fields = \"LS NABWV\"\n"), "{text}");
        assert!(text.contains("\n#asn = false\n"), "{text}");
        // and the result is a valid file that loads back to exactly those values
        let cfg2 = parse(&text).unwrap();
        assert_eq!(cfg2.file.probe.interval, Some(2.0));
        assert_eq!(cfg2.file.probe.dns, Some(false));
        assert_eq!(cfg2.rtt_thresholds.unwrap().to_millis(), [20, 50, 100, 300]);
        assert_eq!(cfg2.file.probe.gracetime, None);
    }

    #[test]
    fn render_covers_every_key() {
        let cfg = EffectiveConfig {
            rtt_thresholds: RttThresholds::from_millis(&[5, 10, 20, 40]).unwrap(),
            fields: "LSNB".to_string(),
            ascii: true,
            color: ColorChoice::Never,
            sparkline: false,
            detail_pane: false,
            interval: 2.5,
            gracetime: 2.0,
            max_ttl: 20,
            max_unknown: 3,
            timeout: 7,
            dns: false,
            asn: true,
        };
        let text = render(&cfg);
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with('#') && l.contains(" = ")),
            "no key should be commented out: {text}"
        );
        let mut a = args(&["h"]);
        apply(&mut a, &parse(&text).unwrap());
        assert_eq!(effective_from_args(&a), cfg);
    }

    #[test]
    fn the_template_matches_the_documented_example_byte_for_byte() {
        let doc = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/config.example.toml"
        );
        assert_eq!(
            std::fs::read_to_string(doc).unwrap(),
            render(&EffectiveConfig::default()),
            "docs/config.example.toml and the default rendering have drifted apart"
        );
    }

    #[test]
    fn the_template_parses_to_the_built_in_defaults() {
        // Every key is commented out, so the written file is valid TOML that changes nothing …
        let template = render(&EffectiveConfig::default());
        assert_eq!(parse(&template).unwrap(), LoadedConfig::default());
        // … and uncommenting every key yields exactly the documented defaults.
        let all: String = template
            .lines()
            .map(|l| {
                l.strip_prefix('#')
                    .filter(|r| r.contains(" = "))
                    .unwrap_or(l)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = parse(&all).unwrap();
        assert_eq!(cfg.rtt_thresholds, Some(RttThresholds::default()));
        assert_eq!(cfg.file.display.fields.as_deref(), Some("LS NABWV"));
        assert_eq!(cfg.file.display.color, Some(ColorChoice::Auto));
        assert_eq!(cfg.file.probe.interval, Some(1.0));
        let o = with_file(&all, |path| {
            let mut a = Args::parse_argv(vec!["mtr".to_string(), "h".to_string()]).unwrap();
            apply(&mut a, &load(path).unwrap());
            a.into_options(false).unwrap()
        });
        let d = opts("", None, &["h"]);
        assert_eq!(o.rtt_thresholds, d.rtt_thresholds);
        assert_eq!(o.config, d.config);
        assert_eq!(
            (o.ascii, o.color, o.sparkline, o.detail_pane),
            (d.ascii, d.color, d.sparkline, d.detail_pane)
        );
    }

    #[test]
    fn init_refuses_values_that_would_not_load_again() {
        let cfg = EffectiveConfig {
            fields: "LSQ".to_string(),
            ..EffectiveConfig::default()
        };
        assert_eq!(
            init(Path::new("/nonexistent/x.toml"), &cfg).unwrap_err(),
            "Unknown field identifier: Q"
        );
    }

    #[test]
    fn init_writes_the_template_once_and_refuses_to_overwrite() {
        let dir = std::env::temp_dir().join(format!("mtr-rs-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("config.toml");
        init(&path, &EffectiveConfig::default()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            render(&EffectiveConfig::default())
        );
        assert_eq!(
            init(&path, &EffectiveConfig::default()).unwrap_err(),
            format!("{}: file exists, refusing to overwrite it", path.display())
        );
        // the round trip: what init wrote loads cleanly
        assert_eq!(load(&path).unwrap(), LoadedConfig::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn default_path_ends_in_the_xdg_location() {
        // `default_path` reads the process environment, so only assert on its shape here; the
        // end-to-end path behaviour is covered by the `--init-config` process test.
        let p = default_path();
        assert!(p.is_none_or(|p| p.ends_with("mtr-rs/config.toml")));
    }
}
