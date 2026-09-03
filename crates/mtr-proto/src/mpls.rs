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

/// Parse the flat comma list `l,tc,s,ttl[,l,tc,s,ttl…]`. Like `parse_mpls_values()`,
/// reads fields incrementally and stops consuming as soon as `MAX_LABELS` complete labels
/// are collected, ignoring any trailing garbage after the cap. A trailing partial group
/// (fewer than 4 values after the last complete label) is dropped. Any non-numeric field
/// encountered before the cap is an error (the caller then keeps no labels).
pub fn parse_mpls_list(s: &str) -> Result<Vec<MplsLabel>, ParseError> {
    let mut labels: Vec<MplsLabel> = Vec::new();
    let mut current_group: Vec<u32> = Vec::new();

    for field in s.split(',') {
        // If we've already collected MAX_LABELS, stop consuming.
        if labels.len() >= MAX_LABELS {
            break;
        }

        // cmdpipe.c:570 uses strtol, then stores into unsigned long / uint8_t: negatives wrap.
        let v: i64 = field
            .parse()
            .map_err(|_| ParseError::MalformedMpls(s.to_string()))?;
        current_group.push(v as u32); // C stores into unsigned long / uint8_t, truncating

        // When we have a complete group of 4, form a label.
        if current_group.len() == 4 {
            labels.push(MplsLabel {
                label: current_group[0],
                tc: current_group[1] as u8,
                bottom_of_stack: current_group[2] != 0,
                ttl: current_group[3] as u8,
            });
            current_group.clear();
        }
    }

    Ok(labels)
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

    #[test]
    fn stops_at_eight_labels_ignoring_garbage_after() {
        // 32 valid values (8 labels) then ",x" — C stops reading at 8 labels, so Ok(8 labels).
        let mut s = (0..32).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        s.push_str(",x");
        assert_eq!(parse_mpls_list(&s).unwrap().len(), 8);
    }

    #[test]
    fn drops_trailing_partial_and_stops_at_cap() {
        // 35 valid values (8 labels + 3 stray) — stops at cap, partial dropped.
        let many = (0..35).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        assert_eq!(parse_mpls_list(&many).unwrap().len(), 8);
    }

    #[test]
    fn malformed_before_cap_is_error() {
        // "1,x,3,4" — garbage in first group, error.
        assert!(parse_mpls_list("1,x,3,4").is_err());
        // "1,2,3,4,x" — garbage after a complete label but before cap, error.
        assert!(parse_mpls_list("1,2,3,4,x").is_err());
    }

    #[test]
    fn negative_fields_wrap_like_strtol_into_unsigned() {
        // cmdpipe.c:570 parses with strtol and stores into unsigned fields.
        let v = parse_mpls_list("-1,-1,-1,-1").unwrap();
        assert_eq!(
            v[0],
            MplsLabel {
                label: u32::MAX,
                tc: 255,
                bottom_of_stack: true,
                ttl: 255
            }
        );
    }
}
