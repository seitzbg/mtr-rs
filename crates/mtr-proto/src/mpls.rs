//! MPLS label list as carried in `mpls …` (packet/probe.c:212-244 formats it,
//! ui/cmdpipe.c:559-617 parses it) — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::fmt::Write as _;

use crate::{MAX_LABELS, ParseError};

/// One MPLS label from an ICMP extension object (RFC 4950).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MplsLabel {
    pub label: u32,
    pub tc: u8,
    pub bottom_of_stack: bool,
    pub ttl: u8,
}

/// Parse the flat comma list `l,tc,s,ttl[,l,tc,s,ttl…]`. Like `parse_mpls_values()`, a
/// trailing partial group is dropped, more than `MAX_LABELS` labels are truncated, and any
/// non-numeric field is an error (the caller then keeps no labels).
pub fn parse_mpls_list(s: &str) -> Result<Vec<MplsLabel>, ParseError> {
    let mut values: Vec<u32> = Vec::new();
    for field in s.split(',') {
        let v: u64 = field
            .parse()
            .map_err(|_| ParseError::MalformedMpls(s.to_string()))?;
        values.push(v as u32); // C stores into unsigned long / uint8_t, truncating
    }
    Ok(values
        .chunks_exact(4)
        .take(MAX_LABELS)
        .map(|g| MplsLabel {
            label: g[0],
            tc: g[1] as u8,
            bottom_of_stack: g[2] != 0,
            ttl: g[3] as u8,
        })
        .collect())
}

/// `format_mpls_string()`: `%u,%u,%u,%u` per label, labels joined by a bare comma.
pub fn format_mpls_list(labels: &[MplsLabel], out: &mut String) {
    for (i, l) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{},{},{},{}",
            l.label,
            l.tc,
            u8::from(l.bottom_of_stack),
            l.ttl
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_comma_list_in_groups_of_four() {
        let v = parse_mpls_list("16001,0,1,1,16002,2,0,255").unwrap();
        assert_eq!(
            v,
            vec![
                MplsLabel {
                    label: 16001,
                    tc: 0,
                    bottom_of_stack: true,
                    ttl: 1
                },
                MplsLabel {
                    label: 16002,
                    tc: 2,
                    bottom_of_stack: false,
                    ttl: 255
                },
            ]
        );
    }

    #[test]
    fn trailing_partial_group_is_dropped_and_labels_capped_at_eight() {
        assert_eq!(parse_mpls_list("1,2,3,4,5,6").unwrap().len(), 1);
        let many = (0..40).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        assert_eq!(parse_mpls_list(&many).unwrap().len(), 8);
    }

    #[test]
    fn non_numeric_field_is_an_error() {
        assert!(parse_mpls_list("1,x,3,4").is_err());
        assert!(parse_mpls_list("").is_err());
    }

    #[test]
    fn formats_like_probe_c() {
        let mut s = String::new();
        format_mpls_list(
            &[
                MplsLabel {
                    label: 16001,
                    tc: 0,
                    bottom_of_stack: true,
                    ttl: 1,
                },
                MplsLabel {
                    label: 7,
                    tc: 3,
                    bottom_of_stack: false,
                    ttl: 9,
                },
            ],
            &mut s,
        );
        assert_eq!(s, "16001,0,1,1,7,3,0,9");
    }
}
