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

/// What `--init-config` writes, and — byte for byte, see the test below — the content of
/// `docs/config.example.toml`.
pub const TEMPLATE: &str = r##"# mtr-rs configuration file — ~/.config/mtr-rs/config.toml
# ($XDG_CONFIG_HOME/mtr-rs/config.toml when XDG_CONFIG_HOME is set; --config PATH overrides both.)
#
# Precedence, lowest to highest:
#   built-in defaults  <  this file  <  $MTR_OPTIONS  <  the command line
#
# Every key is optional and is shown below commented out, with its built-in default.
# Uncomment a key to change it.

[display]
# The four upper bounds of the RTT colour ramp, in milliseconds: below the first is green, then
# yellow, magenta and red, and at or above the last, bold red. Same as --rtt-thresholds.
#rtt_thresholds_ms = [30, 100, 200, 500]
# The columns to show, using the field letters of -o.
#fields = "LS NABWV"
# ASCII glyphs and borders instead of Unicode ones (--ascii).
#ascii = false
# "auto" colours unless NO_COLOR is set, "always" colours even then, "never" is --no-color.
#color = "auto"
# Show the Recent sparkline column when the TUI starts (toggled with d).
#sparkline = true
# Open the detail pane when the TUI starts (toggled with Enter).
#detail_pane = true

[probe]
# Seconds between probe cycles (-i). Values below 1.0 need root.
#interval = 1.0
# The highest TTL probed (-m).
#max_ttl = 30
# Consecutive unanswered hops before the scan stops (-U).
#max_unknown = 12
# Seconds a probe may stay outstanding before it counts as lost (-Z).
#timeout = 10
# Resolve host names; false is -n.
#dns = true
# Look up origin AS numbers; true is -z.
#asn = false
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
        if let Some(i) = self.probe.interval
            && i <= 0.0
        {
            return Err("wait time must be positive".to_string());
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
    if v.iter().any(|&n| n < 0) {
        return Err("rtt thresholds must be positive".to_string());
    }
    let ms: Vec<u64> = v.iter().map(|&n| n as u64).collect();
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

/// `--init-config`: create the parent directories and write [`TEMPLATE`], refusing to touch an
/// existing file. The message is path-prefixed like [`load`]'s.
pub fn init(path: &Path) -> Result<(), String> {
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
    f.write_all(TEMPLATE.as_bytes())
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

    #[test]
    fn the_template_matches_the_documented_example_byte_for_byte() {
        let doc = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/config.example.toml"
        );
        assert_eq!(
            std::fs::read_to_string(doc).unwrap(),
            TEMPLATE,
            "docs/config.example.toml and config_file::TEMPLATE have drifted apart"
        );
    }

    #[test]
    fn the_template_parses_to_the_built_in_defaults() {
        // Every key is commented out, so the written file is valid TOML that changes nothing …
        assert_eq!(parse(TEMPLATE).unwrap(), LoadedConfig::default());
        // … and uncommenting every key yields exactly the documented defaults.
        let all: String = TEMPLATE
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
    fn init_writes_the_template_once_and_refuses_to_overwrite() {
        let dir = std::env::temp_dir().join(format!("mtr-rs-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("config.toml");
        init(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        assert_eq!(
            init(&path).unwrap_err(),
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
