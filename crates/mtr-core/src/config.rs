//! The probing/presentation subset of `struct mtr_ctl` (ui/mtr.h) with the defaults set in
//! ui/mtr.c:1144-1172 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::time::Duration;

use mtr_proto::Protocol;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub protocol: Protocol,
    /// `WaitTime`, seconds between cycles.
    pub interval: f64,
    /// `MaxPing`: cycles before the grace period (report modes; interactive only with `-c`).
    pub max_ping: u32,
    /// `Interactive`: never stop on `max_ping` unless `force_max_ping`.
    pub interactive: bool,
    /// `ForceMaxPing`: `-c` was given.
    pub force_max_ping: bool,
    /// `cpacketsize`; negative = random size in `[MIN_PACKET, -packet_size]` per batch.
    pub packet_size: i32,
    /// `bitpattern`; `-1` = random byte per batch.
    pub bit_pattern: i32,
    pub tos: u8,
    /// `SO_MARK` value, 0 = unset.
    pub mark: u32,
    /// `fstTTL` (>= 1).
    pub first_ttl: u8,
    /// `maxTTL` (1..=255).
    pub max_ttl: u8,
    /// `dueTTL`, 0 = unset.
    pub due_ttl: u8,
    /// `maxUnknown`.
    pub max_unknown: u32,
    /// `maxDisplayPath`.
    pub max_display_path: usize,
    /// `probe_timeout`; sent to the helper in whole seconds.
    pub probe_timeout: Duration,
    /// `GraceTime`, seconds.
    pub grace_time: f64,
    /// `--cache N`: skip hops that answered within `N`.
    pub cache_timeout: Option<Duration>,
    /// `remoteport`, 0 = unset.
    pub remote_port: u16,
    /// `localport`, 0 = unset.
    pub local_port: u16,
    /// `-I`: forwarded as `local-device`.
    pub interface: Option<String>,
    /// `fld_active`, e.g. `"LS NABWV"`.
    pub fields: String,
    pub dns: bool,
    pub show_ips: bool,
    pub mpls: bool,
    /// `-y`/`-z` field indices (0 = ASN); empty = ipinfo off.
    pub ipinfo_fields: Vec<u8>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            protocol: Protocol::Icmp,
            interval: 1.0,
            max_ping: 10,
            interactive: true,
            force_max_ping: false,
            packet_size: 64,
            bit_pattern: 0,
            tos: 0,
            mark: 0,
            first_ttl: 1,
            max_ttl: 30,
            due_ttl: 0,
            max_unknown: 12,
            max_display_path: 8,
            probe_timeout: Duration::from_secs(10),
            grace_time: 5.0,
            cache_timeout: None,
            remote_port: 0,
            local_port: 0,
            interface: None,
            fields: "LS NABWV".to_string(),
            dns: true,
            show_ips: false,
            mpls: false,
            ipinfo_fields: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn defaults_match_ui_mtr_c() {
        let c = Config::default();
        assert_eq!(c.protocol, mtr_proto::Protocol::Icmp);
        assert_eq!(c.interval, 1.0);
        assert_eq!(
            (c.max_ping, c.interactive, c.force_max_ping),
            (10, true, false)
        );
        assert_eq!((c.packet_size, c.bit_pattern, c.tos, c.mark), (64, 0, 0, 0));
        assert_eq!(
            (
                c.first_ttl,
                c.max_ttl,
                c.due_ttl,
                c.max_unknown,
                c.max_display_path
            ),
            (1, 30, 0, 12, 8)
        );
        assert_eq!(c.probe_timeout, Duration::from_secs(10));
        assert_eq!(c.grace_time, 5.0);
        assert_eq!(c.cache_timeout, None);
        assert_eq!((c.remote_port, c.local_port), (0, 0));
        assert_eq!(c.interface, None);
        assert_eq!(c.fields, "LS NABWV");
        assert_eq!((c.dns, c.show_ips, c.mpls), (true, false, false));
        assert!(c.ipinfo_fields.is_empty());
    }
}
