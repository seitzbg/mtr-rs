//! Command line: `long_options[]`, defaults and validation of ui/mtr.c:520-920 and the
//! `host:port` splitting of ui/mtr.c:188-234 — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::net::IpAddr;
use std::time::Duration;

use clap::{
    ArgAction, ArgMatches, CommandFactory as _, FromArgMatches as _, Parser, parser::ValueSource,
};
use mtr_core::{Config, MAX_PACKET, MIN_PACKET, fields};
use mtr_proto::Protocol;

use crate::options::split_mtr_options;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Tui,
    Report,
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamily {
    Unspec,
    V4,
    V6,
}

/// A probe target as given on the command line; `port` 0 = not specified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub port: u16,
}

/// `strtol(s, &end, 0)` requiring full consumption (utils.c:73-129): decimal, `0x` hex,
/// leading-`0` octal, optional sign.
pub fn parse_c_long(s: &str) -> Result<i64, String> {
    let err = || format!("invalid argument: '{s}'");
    let t = s.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (radix, digits) =
        if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            (16, h)
        } else if body.len() > 1 && body.starts_with('0') {
            (8, &body[1..])
        } else {
            (10, body)
        };
    if digits.is_empty() {
        return Err(err());
    }
    let v = i64::from_str_radix(digits, radix).map_err(|_| err())?;
    Ok(if neg { -v } else { v })
}

fn parse_port(s: &str) -> Result<u16, String> {
    let n = parse_c_long(s)?;
    if !(1..=65535).contains(&n) {
        return Err(format!("Illegal port number: {n}"));
    }
    Ok(n as u16)
}

/// `split_target_port()` (ui/mtr.c:200-234). Callers skip it for ICMP.
pub fn split_target_port(name: &str) -> Result<Target, String> {
    if let Some(rest) = name.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            if let Some(port) = rest[close + 1..].strip_prefix(':') {
                if !port.is_empty() {
                    return Ok(Target {
                        name: rest[..close].to_string(),
                        port: parse_port(port)?,
                    });
                }
            }
        }
        return Ok(Target {
            name: name.to_string(),
            port: 0,
        }); // brackets without a port stay literal
    }
    if name.matches(':').count() == 1 {
        let (host, port) = name.split_once(':').expect("one colon");
        if !port.is_empty() {
            return Ok(Target {
                name: host.to_string(),
                port: parse_port(port)?,
            });
        }
    }
    Ok(Target {
        name: name.to_string(),
        port: 0,
    })
}

/// `parse_ipinfo_fields()` (ui/asn.c:344-392).
pub fn parse_ipinfo_fields(spec: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for item in spec.split(',') {
        if item.is_empty() {
            return Err(format!("empty ipinfo field in '{spec}'"));
        }
        let v =
            parse_c_long(item).map_err(|_| format!("invalid ipinfo field '{item}' in '{spec}'"))?;
        if !(0..=4).contains(&v) {
            return Err(format!("ipinfo value {v} out of range (0 - 4)"));
        }
        if out.len() >= 5 {
            return Err(format!("too many ipinfo fields in '{spec}'"));
        }
        out.push(v as u8);
    }
    if out.is_empty() {
        return Err(format!("empty ipinfo field in '{spec}'"));
    }
    Ok(out)
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "mtr",
    about = "mtr - a network diagnostic tool (Rust port)",
    disable_version_flag = true,
    args_override_self = true
)]
pub struct Args {
    /// Print version (twice for the feature list)
    #[arg(short = 'v', long = "version", action = ArgAction::Count)]
    pub version: u8,
    /// Use IPv4 only
    #[arg(short = '4', long = "inet")]
    pub inet4: bool,
    /// Use IPv6 only
    #[arg(short = '6', long = "inet6")]
    pub inet6: bool,
    /// Read hostnames from FILE (`-` for stdin)
    #[arg(short = 'F', long = "filename", value_name = "FILE")]
    pub filename: Option<String>,
    /// Output using report mode
    #[arg(short = 'r', long = "report")]
    pub report: bool,
    /// Output wide report
    #[arg(short = 'w', long = "report-wide")]
    pub report_wide: bool,
    /// Print the report after the interactive session ends
    #[arg(long = "report-on-exit")]
    pub report_on_exit: bool,
    /// Interactive terminal UI (the default)
    #[arg(short = 't', long = "curses")]
    pub curses: bool,
    /// Output comma separated values
    #[arg(short = 'C', long = "csv")]
    pub csv: bool,
    /// Output JSON
    #[arg(short = 'j', long = "json")]
    pub json: bool,
    /// Do not resolve host names
    #[arg(short = 'n', long = "no-dns")]
    pub no_dns: bool,
    /// Show IP numbers and host names
    #[arg(short = 'b', long = "show-ips")]
    pub show_ips: bool,
    /// Select output fields (e.g. "LS NABWV")
    #[arg(short = 'o', long = "order", value_name = "FIELDS")]
    pub order: Option<String>,
    /// Select IP information fields (comma separated numbers 0-4)
    #[arg(short = 'y', long = "ipinfo", value_name = "FIELDS")]
    pub ipinfo: Option<String>,
    /// Display AS number
    #[arg(short = 'z', long = "aslookup")]
    pub aslookup: bool,
    /// DNS zone answering IPv4 origin-AS TXT queries
    #[arg(
        long = "ipinfo_provider4",
        value_name = "ZONE",
        default_value = "origin.asn.cymru.com"
    )]
    pub ipinfo_provider4: String,
    /// DNS zone answering IPv6 origin-AS TXT queries
    #[arg(
        long = "ipinfo_provider6",
        value_name = "ZONE",
        default_value = "origin6.asn.cymru.com"
    )]
    pub ipinfo_provider6: String,
    /// Probe interval in seconds
    #[arg(
        short = 'i',
        long = "interval",
        value_name = "SECONDS",
        default_value_t = 1.0
    )]
    pub interval: f64,
    /// Number of probe cycles
    #[arg(short = 'c', long = "report-cycles", value_name = "COUNT", value_parser = parse_c_long)]
    pub report_cycles: Option<i64>,
    /// Packet size (negative: random up to the absolute value)
    #[arg(
        short = 's',
        long = "psize",
        value_name = "PACKETSIZE",
        value_parser = parse_c_long,
        default_value = "64",
        allow_hyphen_values = true
    )]
    pub psize: i64,
    /// Payload byte (-1: random)
    #[arg(
        short = 'B',
        long = "bitpattern",
        value_name = "NUM",
        value_parser = parse_c_long,
        default_value = "0",
        allow_hyphen_values = true
    )]
    pub bitpattern: i64,
    /// Type of service field
    #[arg(
        short = 'Q',
        long = "tos",
        value_name = "NUM",
        value_parser = parse_c_long,
        default_value = "0",
        allow_hyphen_values = true
    )]
    pub tos: i64,
    /// Display MPLS labels from ICMP extensions
    #[arg(short = 'e', long = "mpls")]
    pub mpls: bool,
    /// Use the named network interface
    #[arg(short = 'I', long = "interface", value_name = "NAME")]
    pub interface: Option<String>,
    /// Bind the outgoing socket to ADDRESS
    #[arg(short = 'a', long = "address", value_name = "ADDRESS")]
    pub address: Option<String>,
    /// First TTL to probe
    #[arg(short = 'f', long = "first-ttl", value_name = "NUM", value_parser = parse_c_long, default_value = "1")]
    pub first_ttl: i64,
    /// Maximum number of hops
    #[arg(short = 'm', long = "max-ttl", value_name = "NUM", value_parser = parse_c_long, default_value = "30")]
    pub max_ttl: i64,
    /// TTL that must be reached before a cycle ends
    #[arg(short = 'D', long = "due-ttl", value_name = "NUM", value_parser = parse_c_long)]
    pub due_ttl: Option<i64>,
    /// Maximum unknown hops
    #[arg(short = 'U', long = "max-unknown", value_name = "NUM", value_parser = parse_c_long, default_value = "12")]
    pub max_unknown: i64,
    /// Maximum ECMP paths shown per hop
    #[arg(short = 'E', long = "max-display-path", value_name = "NUM", value_parser = parse_c_long, default_value = "8")]
    pub max_display_path: i64,
    /// Use UDP instead of ICMP echo
    #[arg(short = 'u', long = "udp")]
    pub udp: bool,
    /// Use TCP instead of ICMP echo
    #[arg(short = 'T', long = "tcp")]
    pub tcp: bool,
    /// Use SCTP instead of ICMP echo
    #[arg(short = 'S', long = "sctp")]
    pub sctp: bool,
    /// Target port for TCP, SCTP or UDP
    #[arg(short = 'P', long = "port", value_name = "PORT", value_parser = parse_c_long)]
    pub port: Option<i64>,
    /// Source port for UDP
    #[arg(short = 'L', long = "localport", value_name = "PORT", value_parser = parse_c_long)]
    pub localport: Option<i64>,
    /// Seconds to keep probe sockets open
    #[arg(short = 'Z', long = "timeout", value_name = "SECONDS", value_parser = parse_c_long, default_value = "10")]
    pub timeout: i64,
    /// Seconds to wait for late replies after the last cycle
    #[arg(
        short = 'G',
        long = "gracetime",
        value_name = "SECONDS",
        default_value_t = 5.0
    )]
    pub gracetime: f64,
    /// Skip hops that answered within SECONDS
    #[arg(long = "cache", value_name = "SECONDS", value_parser = parse_c_long)]
    pub cache: Option<i64>,
    /// Mark each sent packet (SO_MARK)
    #[arg(short = 'M', long = "mark", value_name = "MARK", value_parser = parse_c_long)]
    pub mark: Option<i64>,
    /// Use ASCII glyphs and borders in the TUI
    #[arg(long = "ascii")]
    pub ascii: bool,
    /// Disable colour in the TUI (NO_COLOR is honoured too)
    #[arg(long = "no-color")]
    pub no_color: bool,
    /// Target hosts (HOSTNAME[:PORT] with -u/-T/-S)
    #[arg(value_name = "HOSTNAME")]
    pub hosts: Vec<String>,
    /// Output mode by last-flag-wins order (mtr.c:624-660); set by [`Args::parse_argv`].
    #[arg(skip)]
    pub mode: Option<OutputMode>,
}

/// The mode flags in `argv` order; the one that appears last wins, as each `case` in
/// ui/mtr.c:624-660 overwrites `ctl->DisplayMode`. Returns `None` when no mode flag was given.
pub fn output_mode(m: &ArgMatches) -> Option<OutputMode> {
    [
        ("report", OutputMode::Report),
        ("report_wide", OutputMode::Report),
        ("curses", OutputMode::Tui),
        ("csv", OutputMode::Csv),
        ("json", OutputMode::Json),
    ]
    .into_iter()
    .filter(|(id, _)| m.value_source(id) == Some(ValueSource::CommandLine))
    .filter_map(|(id, mode)| m.index_of(id).map(|i| (i, mode)))
    .max_by_key(|(i, _)| *i)
    .map(|(_, mode)| mode)
}

#[derive(Debug, Clone)]
pub struct Options {
    pub mode: OutputMode,
    pub report_wide: bool,
    pub report_on_exit: bool,
    pub af: AddressFamily,
    pub targets: Vec<Target>,
    pub source_address: Option<IpAddr>,
    pub ipinfo_provider4: String,
    pub ipinfo_provider6: String,
    pub ascii: bool,
    pub color: bool,
    pub config: Config,
}

impl Args {
    /// Parse `argv` (including `argv[0]`) and record the last-wins output mode.
    pub fn parse_argv(argv: Vec<String>) -> Result<Args, clap::Error> {
        let matches = Args::command().try_get_matches_from(argv)?;
        let mut args = Args::from_arg_matches(&matches)?;
        args.mode = output_mode(&matches);
        Ok(args)
    }

    /// Validation in the order of ui/mtr.c, producing its error texts (exit code 1 in `main`).
    pub fn into_options(self, is_root: bool) -> Result<Options, String> {
        let mode = self.mode.unwrap_or(if self.json {
            OutputMode::Json
        } else if self.csv {
            OutputMode::Csv
        } else if self.report || self.report_wide {
            OutputMode::Report
        } else {
            OutputMode::Tui
        });
        let protocol = match (self.udp, self.tcp, self.sctp) {
            (false, false, false) => Protocol::Icmp,
            (true, false, false) => Protocol::Udp,
            (false, true, false) => Protocol::Tcp,
            (false, false, true) => Protocol::Sctp,
            _ => return Err("-u , -T and -S are mutually exclusive".to_string()),
        };
        let af = match (self.inet4, self.inet6) {
            (true, true) => return Err("-4 and -6 are mutually exclusive".to_string()),
            (true, false) => AddressFamily::V4,
            (false, true) => AddressFamily::V6,
            (false, false) => AddressFamily::Unspec,
        };
        if self.psize.abs() < i64::from(MIN_PACKET) || self.psize.abs() > i64::from(MAX_PACKET) {
            return Err(format!("value out of range ({MIN_PACKET} - {MAX_PACKET})"));
        }
        if !(-1..=255).contains(&self.bitpattern) {
            return Err(format!(
                "value out of range (-1 - 255): {}",
                self.bitpattern
            ));
        }
        if !(0..=255).contains(&self.tos) {
            return Err(format!("value out of range (0 - 255): {}", self.tos));
        }
        if self.interval <= 0.0 {
            return Err("wait time must be positive".to_string());
        }
        if !is_root && self.interval < 1.0 {
            return Err("non-root users cannot request an interval < 1.0 seconds".to_string());
        }
        if self.gracetime <= 0.0 {
            return Err("grace time must be positive".to_string());
        }
        if self.cache.is_some_and(|c| c <= 0) {
            return Err("cache timeout must be positive".to_string());
        }
        if self.timeout < 1 {
            return Err("timeout must be positive".to_string());
        }
        let first_ttl = self.first_ttl.max(1);
        let max_ttl = self.max_ttl.clamp(1, 255);
        let due_ttl = match self.due_ttl {
            None => 0,
            Some(d) if d <= 0 => return Err("due TTL must be greater than 0".to_string()),
            Some(d) => d.min(255),
        };
        if first_ttl > max_ttl {
            return Err(format!(
                "firstTTL({first_ttl}) cannot be larger than maxTTL({max_ttl})."
            ));
        }
        if due_ttl > 0 && due_ttl < first_ttl {
            return Err(format!(
                "dueTTL({due_ttl}) cannot be less than firstTTL({first_ttl})."
            ));
        }
        if due_ttl > max_ttl {
            return Err(format!(
                "dueTTL({due_ttl}) cannot be larger than maxTTL({max_ttl})."
            ));
        }
        let max_unknown = self.max_unknown.max(1);
        let max_display_path = self.max_display_path.clamp(0, 128);
        let mut remote_port = match self.port {
            None => 0,
            Some(p) if (1..=65535).contains(&p) => p as u16,
            Some(p) => return Err(format!("Illegal port number: {p}")),
        };
        let local_port = match self.localport {
            None => 0,
            Some(p) if (1024..=65535).contains(&p) => p as u16,
            Some(p) => return Err(format!("Illegal port number: {p}")),
        };
        if matches!(protocol, Protocol::Tcp | Protocol::Sctp) && remote_port == 0 {
            remote_port = 80;
        }
        if protocol == Protocol::Icmp && remote_port != 0 {
            return Err(
                "port number specified (-P) but protocol is ICMP; use -T (TCP) or -u (UDP)"
                    .to_string(),
            );
        }
        let fields_spec = self.order.clone().unwrap_or_else(|| "LS NABWV".to_string());
        fields::validate_fields(&fields_spec)?;
        let ipinfo_fields = match &self.ipinfo {
            Some(spec) => parse_ipinfo_fields(spec)?,
            None if self.aslookup => vec![0],
            None => Vec::new(),
        };
        let source_address = match &self.address {
            None => None,
            Some(a) => Some(
                a.parse::<IpAddr>()
                    .map_err(|_| "invalid local address".to_string())?,
            ),
        };
        let mut targets = Vec::new();
        for name in &self.hosts {
            targets.push(if protocol == Protocol::Icmp {
                Target {
                    name: name.clone(),
                    port: 0,
                }
            } else {
                split_target_port(name)?
            });
        }
        if targets.is_empty() {
            targets.push(Target {
                name: "localhost".to_string(),
                port: 0,
            });
        }
        let config = Config {
            protocol,
            interval: self.interval,
            max_ping: self.report_cycles.map(|c| c.max(0) as u32).unwrap_or(10),
            interactive: mode == OutputMode::Tui,
            force_max_ping: self.report_cycles.is_some(),
            packet_size: self.psize as i32,
            bit_pattern: self.bitpattern as i32,
            tos: self.tos as u8,
            mark: self.mark.map(|m| m as u32).unwrap_or(0),
            first_ttl: first_ttl as u8,
            max_ttl: max_ttl as u8,
            due_ttl: due_ttl as u8,
            max_unknown: max_unknown as u32,
            max_display_path: max_display_path as usize,
            probe_timeout: Duration::from_secs(self.timeout as u64),
            grace_time: self.gracetime,
            cache_timeout: self.cache.map(|c| Duration::from_secs(c as u64)),
            remote_port,
            local_port,
            interface: self.interface.clone(),
            fields: fields_spec,
            dns: !self.no_dns,
            show_ips: self.show_ips,
            mpls: self.mpls,
            ipinfo_fields,
        };
        Ok(Options {
            mode,
            report_wide: self.report_wide,
            report_on_exit: self.report_on_exit,
            af,
            targets,
            source_address,
            ipinfo_provider4: self.ipinfo_provider4.clone(),
            ipinfo_provider6: self.ipinfo_provider6.clone(),
            ascii: self.ascii,
            color: !self.no_color && std::env::var_os("NO_COLOR").is_none(),
            config,
        })
    }
}

/// `argv[0]`, then the words of `$MTR_OPTIONS`, then the real arguments — so the command line
/// overrides the environment (C parses the environment first for the same reason).
pub fn build_argv(
    env_options: Option<&str>,
    args: impl Iterator<Item = String>,
) -> Result<Vec<String>, String> {
    let mut v = vec!["mtr".to_string()];
    if let Some(e) = env_options {
        v.extend(split_mtr_options(e)?);
    }
    v.extend(args);
    Ok(v)
}

/// `print_version()` (ui/mtr.c:357-407).
pub fn version_text(verbose: u8) -> String {
    let mut s = format!("mtr {}\n", env!("CARGO_PKG_VERSION"));
    if verbose >= 2 {
        s.push_str("features:\n");
        let features = [
            ("ipv6", true),
            ("curses", false),
            ("cursesw", false),
            ("braille", false),
            ("gtk", false),
            ("json", true),
            ("ipinfo", true),
            ("mark", cfg!(target_os = "linux")),
        ];
        for (name, yes) in features {
            s.push_str(&format!(
                "  {:<8} {}\n",
                name,
                if yes { "yes" } else { "no" }
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("mtr").chain(args.iter().copied())).unwrap()
    }

    fn opts(args: &[&str]) -> Result<Options, String> {
        parse(args).into_options(false)
    }

    #[test]
    fn c_long_semantics() {
        assert_eq!(parse_c_long("10"), Ok(10));
        assert_eq!(parse_c_long("0x10"), Ok(16));
        assert_eq!(parse_c_long("010"), Ok(8));
        assert_eq!(parse_c_long("-5"), Ok(-5));
        assert_eq!(parse_c_long("0"), Ok(0));
        assert_eq!(
            parse_c_long("5x"),
            Err("invalid argument: '5x'".to_string())
        );
        assert!(parse_c_long("").is_err());
    }

    #[test]
    fn defaults_produce_the_c_config() {
        let o = opts(&["-r", "example.org"]).unwrap();
        assert_eq!(o.mode, OutputMode::Report);
        assert_eq!(
            o.config,
            Config {
                interactive: false,
                ..Config::default()
            }
        );
        assert_eq!(
            o.targets,
            vec![Target {
                name: "example.org".into(),
                port: 0
            }]
        );
        assert_eq!(o.af, AddressFamily::Unspec);
        assert!(!o.report_wide && !o.report_on_exit && !o.ascii);
    }

    #[test]
    fn report_cycles_set_force_max_ping() {
        let o = opts(&["-r", "-c", "3", "h"]).unwrap();
        assert_eq!((o.config.max_ping, o.config.force_max_ping), (3, true));
        let o = opts(&["-r", "h"]).unwrap();
        assert_eq!((o.config.max_ping, o.config.force_max_ping), (10, false));
        let o = opts(&["h"]).unwrap();
        assert!(o.config.interactive);
    }

    fn opts_argv(args: &[&str]) -> Options {
        let argv: Vec<String> = std::iter::once("mtr")
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        Args::parse_argv(argv).unwrap().into_options(false).unwrap()
    }

    #[test]
    fn output_modes_and_precedence() {
        assert_eq!(opts_argv(&["h"]).mode, OutputMode::Tui);
        assert_eq!(opts_argv(&["-t", "h"]).mode, OutputMode::Tui);
        assert_eq!(opts_argv(&["-w", "h"]).mode, OutputMode::Report);
        // mtr.c:624-660: the last mode flag on the command line wins (deviation 11)
        assert_eq!(opts_argv(&["-r", "-t", "h"]).mode, OutputMode::Tui);
        assert_eq!(opts_argv(&["-t", "-r", "h"]).mode, OutputMode::Report);
        assert_eq!(opts_argv(&["-j", "-C", "h"]).mode, OutputMode::Csv);
        assert_eq!(opts_argv(&["-C", "-j", "h"]).mode, OutputMode::Json);
        assert_eq!(opts_argv(&["-w", "-C", "h"]).mode, OutputMode::Csv);
        let o = opts_argv(&["-w", "-t", "h"]);
        assert_eq!((o.mode, o.report_wide), (OutputMode::Tui, true));
        assert!(o.config.interactive);
        let o = opts_argv(&["-r", "-c", "3", "h"]);
        assert!(!o.config.interactive && o.config.force_max_ping);
    }

    #[test]
    fn protocol_flags_and_ports() {
        assert_eq!(
            opts(&["-u", "-T", "h"]).unwrap_err(),
            "-u , -T and -S are mutually exclusive"
        );
        let o = opts(&["-T", "h"]).unwrap();
        assert_eq!(
            (o.config.protocol, o.config.remote_port),
            (Protocol::Tcp, 80)
        );
        let o = opts(&["-u", "-P", "53", "h"]).unwrap();
        assert_eq!(
            (o.config.protocol, o.config.remote_port),
            (Protocol::Udp, 53)
        );
        assert_eq!(
            opts(&["-P", "80", "h"]).unwrap_err(),
            "port number specified (-P) but protocol is ICMP; use -T (TCP) or -u (UDP)"
        );
        assert_eq!(
            opts(&["-T", "-P", "0", "h"]).unwrap_err(),
            "Illegal port number: 0"
        );
        assert_eq!(
            opts(&["-u", "-L", "80", "h"]).unwrap_err(),
            "Illegal port number: 80"
        );
        assert_eq!(
            opts(&["-u", "-L", "40000", "h"]).unwrap().config.local_port,
            40000
        );
        let o = opts(&[
            "-T",
            "h:8443",
            "[2001:db8::1]:22",
            "v6only::1",
            "[2001:db8::2]",
        ])
        .unwrap();
        assert_eq!(
            o.targets,
            vec![
                Target {
                    name: "h".into(),
                    port: 8443
                },
                Target {
                    name: "2001:db8::1".into(),
                    port: 22
                },
                Target {
                    name: "v6only::1".into(),
                    port: 0
                },
                Target {
                    name: "[2001:db8::2]".into(),
                    port: 0
                },
            ]
        );
        // ICMP never splits host:port
        assert_eq!(
            opts(&["h:8443"]).unwrap().targets[0],
            Target {
                name: "h:8443".into(),
                port: 0
            }
        );
        assert_eq!(
            split_target_port("h:99999").unwrap_err(),
            "Illegal port number: 99999"
        );
    }

    #[test]
    fn ranges_match_c() {
        assert_eq!(
            opts(&["-s", "10", "h"]).unwrap_err(),
            "value out of range (28 - 65535)"
        );
        assert_eq!(opts(&["-s", "-100", "h"]).unwrap().config.packet_size, -100);
        assert_eq!(
            opts(&["-B", "256", "h"]).unwrap_err(),
            "value out of range (-1 - 255): 256"
        );
        assert_eq!(opts(&["-B", "-1", "h"]).unwrap().config.bit_pattern, -1);
        assert_eq!(
            opts(&["-Q", "-1", "h"]).unwrap_err(),
            "value out of range (0 - 255): -1"
        );
        assert_eq!(
            opts(&["-i", "0", "h"]).unwrap_err(),
            "wait time must be positive"
        );
        assert_eq!(
            opts(&["-i", "0.5", "h"]).unwrap_err(),
            "non-root users cannot request an interval < 1.0 seconds"
        );
        assert_eq!(
            parse(&["-i", "0.5", "h"])
                .into_options(true)
                .unwrap()
                .config
                .interval,
            0.5
        );
        assert_eq!(
            opts(&["-G", "0", "h"]).unwrap_err(),
            "grace time must be positive"
        );
        assert_eq!(
            opts(&["--cache", "0", "h"]).unwrap_err(),
            "cache timeout must be positive"
        );
        assert_eq!(
            opts(&["--cache", "30", "h"]).unwrap().config.cache_timeout,
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            opts(&["-Z", "0", "h"]).unwrap_err(),
            "timeout must be positive"
        );
        assert_eq!(
            opts(&["-D", "0", "h"]).unwrap_err(),
            "due TTL must be greater than 0"
        );
        assert_eq!(
            opts(&["-f", "5", "-m", "3", "h"]).unwrap_err(),
            "firstTTL(5) cannot be larger than maxTTL(3)."
        );
        assert_eq!(
            opts(&["-f", "3", "-D", "2", "h"]).unwrap_err(),
            "dueTTL(2) cannot be less than firstTTL(3)."
        );
        assert_eq!(
            opts(&["-m", "4", "-D", "9", "h"]).unwrap_err(),
            "dueTTL(9) cannot be larger than maxTTL(4)."
        );
        let o = opts(&["-f", "0", "-m", "999", "-U", "0", "-E", "500", "h"]).unwrap();
        assert_eq!(
            (
                o.config.first_ttl,
                o.config.max_ttl,
                o.config.max_unknown,
                o.config.max_display_path
            ),
            (1, 255, 1, 128)
        );
    }

    #[test]
    fn fields_and_ipinfo() {
        assert_eq!(
            opts(&["-o", "LSQ", "h"]).unwrap_err(),
            "Unknown field identifier: Q"
        );
        assert_eq!(
            opts(&["-o", "LS NABWV", "h"]).unwrap().config.fields,
            "LS NABWV"
        );
        assert_eq!(opts(&["-z", "h"]).unwrap().config.ipinfo_fields, vec![0]);
        assert_eq!(
            opts(&["-y", "1,0x2", "h"]).unwrap().config.ipinfo_fields,
            vec![1, 2]
        );
        assert_eq!(
            opts(&["-y", "5", "h"]).unwrap_err(),
            "ipinfo value 5 out of range (0 - 4)"
        );
        assert_eq!(
            opts(&["-y", "1,,2", "h"]).unwrap_err(),
            "empty ipinfo field in '1,,2'"
        );
        assert_eq!(
            opts(&["-y", "1,2,3,4,0,1", "h"]).unwrap_err(),
            "too many ipinfo fields in '1,2,3,4,0,1'"
        );
        assert_eq!(
            opts(&["-y", "x", "h"]).unwrap_err(),
            "invalid ipinfo field 'x' in 'x'"
        );
    }

    #[test]
    fn families_dns_and_misc() {
        assert_eq!(
            opts(&["-4", "-6", "h"]).unwrap_err(),
            "-4 and -6 are mutually exclusive"
        );
        assert_eq!(opts(&["-6", "h"]).unwrap().af, AddressFamily::V6);
        assert_eq!(opts(&["-4", "h"]).unwrap().af, AddressFamily::V4);
        let o = opts(&[
            "-n",
            "-b",
            "-e",
            "-a",
            "192.0.2.1",
            "-I",
            "eth0",
            "-M",
            "7",
            "-Z",
            "3",
            "h",
        ])
        .unwrap();
        assert!(!o.config.dns && o.config.show_ips && o.config.mpls);
        assert_eq!(o.source_address, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(o.config.interface.as_deref(), Some("eth0"));
        assert_eq!(
            (o.config.mark, o.config.probe_timeout),
            (7, Duration::from_secs(3))
        );
        assert_eq!(
            opts(&["-a", "nope", "h"]).unwrap_err(),
            "invalid local address"
        );
        assert_eq!(opts(&[]).unwrap().targets[0].name, "localhost");
        assert_eq!(
            opts(&["--ipinfo_provider4", "x.example", "h"])
                .unwrap()
                .ipinfo_provider4,
            "x.example"
        );
    }

    #[test]
    fn argv_assembly_puts_env_first_so_the_command_line_wins() {
        let v = build_argv(
            Some("-r -c 3"),
            ["-c".to_string(), "5".to_string(), "h".to_string()].into_iter(),
        )
        .unwrap();
        assert_eq!(v, ["mtr", "-r", "-c", "3", "-c", "5", "h"]);
        let o = Args::try_parse_from(v)
            .unwrap()
            .into_options(false)
            .unwrap();
        assert_eq!(o.config.max_ping, 5);
        assert!(build_argv(Some("'"), std::iter::empty::<String>()).is_err());
    }

    #[test]
    fn version_text_lists_features_when_verbose() {
        assert_eq!(
            version_text(1),
            format!("mtr {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(version_text(2).contains("  ipv6     yes\n"));
        assert!(version_text(2).contains("  curses   no\n"));
    }

    #[test]
    fn unknown_options_are_rejected_and_flags_after_the_host_still_count() {
        assert!(Args::try_parse_from(["mtr", "--frobnicate", "host"]).is_err());
        assert!(Args::try_parse_from(["mtr", "-Q9x", "host"]).is_err());
        let a = parse(&["host", "-r"]);
        assert!(a.report);
        assert_eq!(a.hosts, ["host"]);
        let a = parse(&["host1", "host2", "-c", "3"]);
        assert_eq!((a.hosts.len(), a.report_cycles), (2, Some(3)));
    }

    #[test]
    fn option_values_do_not_swallow_flags_but_negative_numbers_parse() {
        assert!(Args::try_parse_from(["mtr", "-o", "-r", "host"]).is_err());
        assert!(Args::try_parse_from(["mtr", "-c", "-r", "host"]).is_err());
        assert_eq!(parse(&["-s", "-100", "host"]).psize, -100);
        assert_eq!(parse(&["-B", "-1", "host"]).bitpattern, -1);
        assert_eq!(
            opts(&["-s", "-100", "-B", "-1", "h"])
                .map(|o| (o.config.packet_size, o.config.bit_pattern)),
            Ok((-100, -1))
        );
    }
}
