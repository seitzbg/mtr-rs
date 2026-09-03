//! `csv_close()` (ui/report.c:557-680). GPL-2.0-only.

use std::fmt::Write as _;

use mtr_core::Hop;

use crate::asn;
use crate::emit::ReportContext;
use crate::names::{addr_name, hop_name};

pub fn render(ctx: &ReportContext<'_>, now_epoch: u64) -> String {
    let e = ctx.engine;
    let cfg = e.config();
    let asn_on = cfg.ipinfo_fields.contains(&0);
    let mut o = String::from("Mtr_Version,Start_Time,Status,Host,Hop,Ip");
    if asn_on {
        o.push_str(",Asn");
    }
    for f in &ctx.fields {
        o.push(',');
        if f.key != ' ' {
            o.push_str(f.title);
        }
    }
    o.push('\n');
    for at in e.display_range() {
        let hop = &e.hops()[at];
        let asn = asn_on.then(|| {
            asn::format_field(ctx.names.asn(hop.addr), 0)
                .trim()
                .to_string()
        });
        row(
            &mut o,
            ctx,
            now_epoch,
            at + 1,
            &hop_name(hop, ctx.names, cfg.dns, cfg.show_ips),
            asn.as_deref(),
            hop,
        );
        if ctx.wide {
            // Extra ECMP rows only with -w (report.c:620-678); statistics are the primary address's.
            for a in hop.addrs.iter().take(cfg.max_display_path) {
                if Some(a.addr) == hop.addr {
                    continue;
                }
                let asn = asn_on.then(|| {
                    asn::format_field(ctx.names.asn(Some(a.addr)), 0)
                        .trim()
                        .to_string()
                });
                row(
                    &mut o,
                    ctx,
                    now_epoch,
                    at + 1,
                    &addr_name(Some(a.addr), ctx.names, cfg.dns, cfg.show_ips),
                    asn.as_deref(),
                    hop,
                );
            }
        }
    }
    o
}

fn row(
    o: &mut String,
    ctx: &ReportContext<'_>,
    now: u64,
    hop_no: usize,
    name: &str,
    asn: Option<&str>,
    hop: &Hop,
) {
    let _ = write!(
        o,
        "MTR.{},{},OK,{},{},{}",
        env!("CARGO_PKG_VERSION"),
        now,
        ctx.target_name,
        hop_no,
        name
    );
    if let Some(a) = asn {
        let _ = write!(o, ",{a}");
    }
    for f in &ctx.fields {
        o.push(',');
        if f.key == ' ' {
            continue;
        }
        let v = (f.value)(hop);
        if f.format.is_float() {
            let _ = write!(o, "{:.2}", f64::from(v) / 1000.0);
        } else {
            let _ = write!(o, "{v}");
        }
    }
    o.push('\n');
}
