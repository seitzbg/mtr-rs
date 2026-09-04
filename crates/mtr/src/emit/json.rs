//! `json_close()` (ui/report.c:365-469) written by hand so the output matches jansson exactly:
//! 4-space indent, insertion order, `%.5g` reals with a forced `.0`, `"Loss%"` key kept. GPL-2.0-only.

use std::fmt::Write as _;

use mtr_core::MIN_PACKET;

use crate::asn;
use crate::emit::ReportContext;
use crate::names::hop_name;

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// jansson `JSON_REAL_PRECISION(5)`: `%.5g`, then `.0` appended when the text has neither `.` nor `e`.
pub fn json_real(x: f64) -> String {
    if x == 0.0 {
        return "0.0".to_string();
    }
    let sci = format!("{x:.4e}"); // e.g. "1.2346e5", "5.0000e-1"
    let (mantissa, exponent) = sci
        .split_once('e')
        .expect("scientific notation has an exponent");
    let exponent: i32 = exponent.parse().expect("exponent is an integer");
    let mut s = if !(-4..5).contains(&exponent) {
        format!(
            "{}e{}{:02}",
            strip_zeros(mantissa),
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    } else {
        strip_zeros(&format!("{:.*}", (4 - exponent).max(0) as usize, x))
    };
    if !s.contains('.') && !s.contains('e') {
        s.push_str(".0");
    }
    s
}

/// jansson string escaping: `"`, `\`, control characters; everything else (UTF-8 included) verbatim.
pub fn json_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{08}' => o.push_str("\\b"),
            '\u{0C}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn render(ctx: &ReportContext<'_>) -> String {
    let e = ctx.engine;
    let cfg = e.config();
    let psize = if cfg.packet_size >= 0 {
        cfg.packet_size.to_string()
    } else {
        format!("rand({}-{})", MIN_PACKET, -cfg.packet_size)
    };
    let bitpattern = if cfg.bit_pattern >= 0 {
        format!("0x{:02X}", cfg.bit_pattern as u8)
    } else {
        "rand(0x00-FF)".to_string()
    };
    let asn_on = cfg.ipinfo_fields.contains(&0);

    let mut o = String::from("{\n    \"report\": {\n        \"mtr\": {\n");
    let _ = writeln!(
        o,
        "            \"src\": {},",
        json_string(ctx.local_hostname)
    );
    let _ = writeln!(o, "            \"dst\": {},", json_string(ctx.target_name));
    let _ = writeln!(o, "            \"tos\": {},", cfg.tos);
    let _ = writeln!(o, "            \"tests\": {},", cfg.max_ping);
    let _ = writeln!(o, "            \"psize\": {},", json_string(&psize));
    let _ = writeln!(
        o,
        "            \"bitpattern\": {}",
        json_string(&bitpattern)
    );
    o.push_str("        },\n        \"hubs\": [");

    let hubs: Vec<String> = e
        .display_range()
        .map(|at| {
            let hop = &e.hops()[at];
            let mut h = String::from("            {\n");
            let _ = writeln!(h, "                \"count\": {},", at + 1);
            let _ = write!(
                h,
                "                \"host\": {}",
                json_string(&hop_name(hop, ctx.names, cfg.dns, cfg.show_ips))
            );
            if asn_on {
                let _ = write!(
                    h,
                    ",\n                \"ASN\": {}",
                    json_string(asn::format_field(ctx.names.asn(hop.addr), 0).trim())
                );
            }
            // json_close skips the spacer field (`j <= 0`); keys are the raw titles.
            for f in ctx.fields.iter().filter(|f| f.key != ' ') {
                let v = (f.value)(hop);
                let value = if f.format.is_float() {
                    json_real(f64::from(v) / 1000.0)
                } else {
                    v.to_string()
                };
                let _ = write!(h, ",\n                {}: {}", json_string(f.title), value);
            }
            h.push_str("\n            }");
            h
        })
        .collect();
    if hubs.is_empty() {
        o.push_str("]\n");
    } else {
        o.push('\n');
        o.push_str(&hubs.join(",\n"));
        o.push_str("\n        ]\n");
    }
    o.push_str("    }\n}\n");
    o
}

/// Deviation 30: C prints one `{"report": …}` object per target back to back, which is not a
/// valid JSON document. With more than one target we wrap them in an array (jansson layout:
/// 4-space indent); a single target is byte-identical to C.
pub fn wrap_documents(docs: &[String]) -> String {
    if docs.len() < 2 {
        return docs.first().cloned().unwrap_or_default();
    }
    let indented: Vec<String> = docs
        .iter()
        .map(|d| {
            d.trim_end_matches('\n')
                .lines()
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    format!("[\n{}\n]\n", indented.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::wrap_documents;

    #[test]
    fn several_documents_become_one_array() {
        let a = "{\n    \"x\": 1\n}\n".to_string();
        let b = "{\n    \"x\": 2\n}\n".to_string();
        assert_eq!(wrap_documents(std::slice::from_ref(&a)), a);
        assert_eq!(
            wrap_documents(&[a, b]),
            "[\n    {\n        \"x\": 1\n    },\n    {\n        \"x\": 2\n    }\n]\n"
        );
    }
}
