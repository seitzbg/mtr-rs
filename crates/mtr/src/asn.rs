//! Origin-AS ("ipinfo") lookups: key construction, TXT parsing and column formatting from
//! ui/asn.c (mtr 0.96, commit 7b01773). The DNS transport lives in resolver.rs. GPL-2.0-only.

use std::net::IpAddr;

/// `iiwidth[]` (asn.c:74): ASN, Route, Country, Registry, Allocated.
pub const IIWIDTH: [usize; 5] = [12, 19, 4, 8, 11];
/// `UNKN`.
pub const UNKN: &str = "???";

/// The `|`-separated fields of one origin TXT record, plus the AS name from the second lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsnInfo {
    pub fields: Vec<String>,
    /// Last field of `AS<n>.asn.cymru.com` (spec §7.2); `None` until that query answers.
    pub name: Option<String>,
}

impl AsnInfo {
    /// What C caches after a failed `res_query()`.
    pub fn unknown() -> Self {
        AsnInfo {
            fields: vec![UNKN.to_string()],
            name: None,
        }
    }

    /// `split_txtrec()`: a missing field falls back to field 0.
    pub fn field(&self, i: usize) -> &str {
        self.fields
            .get(i)
            .or_else(|| self.fields.first())
            .map(String::as_str)
            .unwrap_or(UNKN)
    }
}

/// asn.c:394-439: reversed dotted quad, or the top 64 bits nibble-reversed, plus the zone;
/// `64:ff9b::/32` (NAT64) folds to the embedded IPv4 address (asn.c:329-332, 409-419).
pub fn query_name(ip: IpAddr, provider4: &str, provider6: &str) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.{}", o[3], o[2], o[1], o[0], provider4)
        }
        IpAddr::V6(v6) => {
            let b = v6.octets();
            if b[0] == 0x00 && b[1] == 0x64 && b[2] == 0xff && b[3] == 0x9b {
                return format!("{}.{}.{}.{}.{}", b[15], b[14], b[13], b[12], provider4);
            }
            let mut key = String::with_capacity(32);
            for byte in b[..8].iter().rev() {
                key.push_str(&format!("{:x}.{:x}.", byte & 0xf, byte >> 4));
            }
            key.pop();
            format!("{key}.{provider6}")
        }
    }
}

/// `split_txtrec()` (asn.c:267-309): split on `|`, trim whitespace and `|`, at most 16 fields.
pub fn parse_txt(txt: &str) -> AsnInfo {
    AsnInfo {
        fields: txt
            .splitn(16, '|')
            .map(|f| {
                f.trim_matches(|c: char| c.is_ascii_whitespace() || c == '|')
                    .to_string()
            })
            .collect(),
        name: None,
    }
}

/// The AS-name record is `ASN | CC | registry | allocated | <name>`: take the last `|` field.
pub fn parse_as_name(txt: &str) -> Option<String> {
    let name = txt.rsplit('|').next()?.trim();
    (txt.contains('|') && !name.is_empty()).then(|| name.to_string())
}

/// The zone the `AS<n>` records live in; the origin providers are `origin[6].<zone>`.
pub fn name_zone(provider: &str) -> &str {
    provider
        .strip_prefix("origin.")
        .or_else(|| provider.strip_prefix("origin6."))
        .unwrap_or(provider)
}

/// TXT name for the AS-name lookup, or `None` when the origin record holds no single AS number
/// (`???`, or a multi-origin prefix such as `64500 64501`).
pub fn as_name_query(info: &AsnInfo, zone: &str) -> Option<String> {
    let n = info.field(0);
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| format!("AS{n}.{zone}"))
}

/// `fmt_ipinfo_field()` (asn.c:517-530): `AS%-12s` for field 0, `%-{w}s` otherwise, `???` when unknown.
pub fn format_field(info: Option<&AsnInfo>, field: usize) -> String {
    let value = info.map(|i| i.field(field)).unwrap_or(UNKN);
    let width = IIWIDTH[field % IIWIDTH.len()];
    if field == 0 {
        format!("AS{value:<width$}")
    } else {
        format!("{value:<width$}")
    }
}

/// `fmt_ipinfo()` (asn.c:532-560): selected fields joined by one space.
pub fn format_selected(info: Option<&AsnInfo>, fields: &[u8]) -> String {
    fields
        .iter()
        .map(|f| format_field(info, usize::from(*f)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `get_iiwidth_selected()` (asn.c:486-501).
pub fn selected_width(fields: &[u8]) -> usize {
    if fields.is_empty() {
        return 0;
    }
    let widths: usize = fields
        .iter()
        .map(|f| IIWIDTH[usize::from(*f) % IIWIDTH.len()])
        .sum();
    widths + (fields.len() - 1) + 2 * fields.iter().filter(|f| **f == 0).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    const P4: &str = "origin.asn.cymru.com";
    const P6: &str = "origin6.asn.cymru.com";

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn query_names_match_asn_c() {
        assert_eq!(
            query_name(ip("192.0.2.1"), P4, P6),
            "1.2.0.192.origin.asn.cymru.com"
        );
        assert_eq!(
            query_name(ip("2001:db8:1234:5678:9abc::1"), P4, P6),
            "8.7.6.5.4.3.2.1.8.b.d.0.1.0.0.2.origin6.asn.cymru.com"
        );
        assert_eq!(
            query_name(ip("64:ff9b::c000:201"), P4, P6),
            "1.2.0.192.origin.asn.cymru.com",
            "NAT64 folds to IPv4"
        );
    }

    #[test]
    fn txt_records_split_on_pipes_and_trim() {
        let i = parse_txt("64500 | 192.0.2.0/24 | US | arin | 2000-01-01");
        assert_eq!(
            i.fields,
            ["64500", "192.0.2.0/24", "US", "arin", "2000-01-01"]
        );
        assert_eq!(i.field(4), "2000-01-01");
        assert_eq!(i.field(9), "64500", "missing field falls back to the ASN");
        assert_eq!(parse_txt("64500").fields, ["64500"]);
    }

    #[test]
    fn formats_like_fmt_ipinfo() {
        let i = parse_txt("64500 | 192.0.2.0/24 | US | arin | 2000-01-01");
        assert_eq!(format_field(Some(&i), 0), "AS64500       ");
        assert_eq!(format_field(None, 0), "AS???         ");
        assert_eq!(format_field(Some(&i), 2), "US  ");
        assert_eq!(format_selected(Some(&i), &[0, 2]), "AS64500        US  ");
        assert_eq!(selected_width(&[0, 2]), 19);
        assert_eq!(selected_width(&[0]), 14);
        assert_eq!(selected_width(&[]), 0);
        assert_eq!(format_field(Some(&AsnInfo::unknown()), 0).trim(), "AS???");
    }

    #[test]
    fn as_name_records_yield_the_last_field() {
        // dig +short TXT AS64500.asn.cymru.com → "64500 | US | arin | 2000-01-01 | EXAMPLE-AS, US"
        assert_eq!(
            parse_as_name("64500 | US | arin | 2000-01-01 | EXAMPLE-AS, US"),
            Some("EXAMPLE-AS, US".to_string())
        );
        assert_eq!(
            parse_as_name("64500 | US | arin | 2000-01-01 | "),
            None,
            "empty name"
        );
        assert_eq!(parse_as_name(""), None);
        assert_eq!(
            parse_txt("64500 | 192.0.2.0/24 | US | arin | 2000-01-01").name,
            None
        );
        assert_eq!(AsnInfo::unknown().name, None);
    }

    #[test]
    fn as_name_queries_target_the_asn_zone() {
        assert_eq!(name_zone(P4), "asn.cymru.com");
        assert_eq!(name_zone(P6), "asn.cymru.com");
        assert_eq!(name_zone("example.net"), "example.net");
        let info = parse_txt("64500 | 192.0.2.0/24 | US | arin | 2000-01-01");
        assert_eq!(
            as_name_query(&info, name_zone(P4)).as_deref(),
            Some("AS64500.asn.cymru.com")
        );
        assert_eq!(
            as_name_query(&AsnInfo::unknown(), "asn.cymru.com"),
            None,
            "??? has no AS number"
        );
        // a multi-origin prefix ("64500 64501 | …") names no single AS: no second query
        assert_eq!(
            as_name_query(&parse_txt("64500 64501 | 192.0.2.0/24"), "asn.cymru.com"),
            None
        );
    }
}
