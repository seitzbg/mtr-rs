//! Resolved names/ASNs and hop naming: `snprint_addr()` / `snprint_hop_name()` (ui/report.c:59-95)
//! — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use mtr_core::Hop;

use crate::asn::AsnInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupResult {
    Ptr { addr: IpAddr, name: Option<String> },
    Asn { addr: IpAddr, info: Option<AsnInfo> },
}

/// Per-run cache of lookups; negative results are cached forever, as in ui/dns.c and ui/asn.c.
#[derive(Debug, Default)]
pub struct NameCache {
    ptr: HashMap<IpAddr, Option<String>>,
    ptr_pending: HashSet<IpAddr>,
    asn: HashMap<IpAddr, Option<AsnInfo>>,
    asn_pending: HashSet<IpAddr>,
}

impl NameCache {
    pub fn name(&self, ip: IpAddr) -> Option<&str> {
        self.ptr.get(&ip).and_then(|o| o.as_deref())
    }

    pub fn asn(&self, ip: Option<IpAddr>) -> Option<&AsnInfo> {
        ip.and_then(|ip| self.asn.get(&ip)).and_then(|o| o.as_ref())
    }

    /// The AS name from the second Cymru lookup, when it is known.
    pub fn asn_name(&self, ip: Option<IpAddr>) -> Option<&str> {
        self.asn(ip).and_then(|i| i.name.as_deref())
    }

    /// True when a PTR lookup should be issued for `ip` (first sighting).
    pub fn request_ptr(&mut self, ip: IpAddr) -> bool {
        !self.ptr.contains_key(&ip) && self.ptr_pending.insert(ip)
    }

    /// True when an ASN lookup should be issued for `ip` (first sighting).
    pub fn request_asn(&mut self, ip: IpAddr) -> bool {
        !self.asn.contains_key(&ip) && self.asn_pending.insert(ip)
    }

    pub fn apply(&mut self, r: LookupResult) {
        match r {
            LookupResult::Ptr { addr, name } => {
                self.ptr_pending.remove(&addr);
                self.ptr.insert(addr, name);
            }
            LookupResult::Asn { addr, info } => {
                self.asn_pending.remove(&addr);
                self.asn.insert(addr, info);
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.ptr_pending.len() + self.asn_pending.len()
    }

    pub fn insert_name(&mut self, ip: IpAddr, name: &str) {
        self.ptr_pending.remove(&ip);
        self.ptr.insert(ip, Some(name.to_string()));
    }

    pub fn insert_asn(&mut self, ip: IpAddr, info: AsnInfo) {
        self.asn_pending.remove(&ip);
        self.asn.insert(ip, Some(info));
    }
}

/// `is_useful_hostname()` (ui/utils.h:43-49).
pub fn is_useful_hostname(s: &str) -> bool {
    !s.is_empty() && s != "."
}

/// `snprint_addr()`: `???`, the IP, the name, or `name (ip)` with `-b`.
pub fn addr_name(addr: Option<IpAddr>, names: &NameCache, dns: bool, show_ips: bool) -> String {
    let Some(ip) = addr else {
        return "???".to_string();
    };
    match names.name(ip).filter(|n| dns && is_useful_hostname(n)) {
        Some(n) if show_ips => format!("{n} ({ip})"),
        Some(n) => n.to_string(),
        None => ip.to_string(),
    }
}

/// `snprint_hop_name()`: an ICMP error replaces the address.
pub fn hop_name(hop: &Hop, names: &NameCache, dns: bool, show_ips: bool) -> String {
    match hop.err {
        Some(e) => format!("({})", e.as_str()),
        None => addr_name(hop.addr, names, dns, show_ips),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_core::{Hop, HopError};
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn requests_are_issued_once_and_results_apply() {
        let mut c = NameCache::default();
        assert!(c.request_ptr(ip("10.0.0.1")));
        assert!(!c.request_ptr(ip("10.0.0.1")), "already pending");
        assert_eq!(c.pending(), 1);
        c.apply(LookupResult::Ptr {
            addr: ip("10.0.0.1"),
            name: Some("gw.example".into()),
        });
        assert_eq!(
            (c.pending(), c.name(ip("10.0.0.1"))),
            (0, Some("gw.example"))
        );
        assert!(!c.request_ptr(ip("10.0.0.1")), "already resolved");
        c.apply(LookupResult::Ptr {
            addr: ip("10.0.0.2"),
            name: None,
        });
        assert_eq!(c.name(ip("10.0.0.2")), None);
        assert!(
            !c.request_ptr(ip("10.0.0.2")),
            "negative results are cached too"
        );
        assert!(c.request_asn(ip("10.0.0.1")));
        c.apply(LookupResult::Asn {
            addr: ip("10.0.0.1"),
            info: Some(crate::asn::parse_txt("64500 | x | y | z | w")),
        });
        assert_eq!(c.asn(Some(ip("10.0.0.1"))).unwrap().field(0), "64500");
        assert_eq!(c.asn(None), None);
    }

    #[test]
    fn the_cache_hands_out_the_as_name() {
        let mut c = NameCache::default();
        assert_eq!(c.asn_name(Some(ip("10.0.0.1"))), None);
        let mut info = crate::asn::parse_txt("64500 | 192.0.2.0/24 | US | arin | 2000-01-01");
        info.name = Some("EXAMPLE-AS, US".into());
        c.insert_asn(ip("10.0.0.1"), info);
        assert_eq!(c.asn_name(Some(ip("10.0.0.1"))), Some("EXAMPLE-AS, US"));
        assert_eq!(c.asn_name(None), None);
    }

    #[test]
    fn addr_name_follows_snprint_addr() {
        let mut c = NameCache::default();
        c.insert_name(ip("10.0.0.1"), "gw.example");
        assert_eq!(addr_name(None, &c, true, false), "???");
        assert_eq!(addr_name(Some(ip("10.0.0.2")), &c, true, false), "10.0.0.2");
        assert_eq!(
            addr_name(Some(ip("10.0.0.1")), &c, true, false),
            "gw.example"
        );
        assert_eq!(
            addr_name(Some(ip("10.0.0.1")), &c, true, true),
            "gw.example (10.0.0.1)"
        );
        assert_eq!(
            addr_name(Some(ip("10.0.0.1")), &c, false, true),
            "10.0.0.1",
            "-n wins over -b"
        );
        assert!(is_useful_hostname("a") && !is_useful_hostname("") && !is_useful_hostname("."));
        // report.c:70: a useless cached name (empty or ".") falls back to the address
        c.insert_name(ip("10.0.0.3"), ".");
        assert_eq!(addr_name(Some(ip("10.0.0.3")), &c, true, false), "10.0.0.3");
        assert!(c.request_ptr(ip("10.0.0.4")));
        c.insert_name(ip("10.0.0.4"), "x.example");
        assert_eq!(c.pending(), 0, "insert_name clears the pending mark");
    }

    #[test]
    fn hop_name_prefers_icmp_errors() {
        let c = NameCache::default();
        let mut h = Hop::new(4);
        h.addr = Some(ip("10.0.0.5"));
        assert_eq!(hop_name(&h, &c, true, false), "10.0.0.5");
        h.err = Some(HopError::NoRouteHost);
        assert_eq!(hop_name(&h, &c, true, false), "(no route to host)");
    }
}
