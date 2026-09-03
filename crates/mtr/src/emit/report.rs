//! `report_open()` / `report_close()` (ui/report.c:52-350). GPL-2.0-only.

use std::fmt::Write as _;

use mtr_core::HopAddr;
use mtr_core::fields::{format_title, format_value};
use mtr_proto::MplsLabel;

use crate::asn;
use crate::emit::ReportContext;
use crate::names::{addr_name, hop_name};
use crate::width::{display_width, pad_right};

/// `Start: <iso_time()>` — printed once before probing in `-r`/`-w` mode (report_open, utils.c:193-202).
pub fn start_line(now: &jiff::Zoned) -> String {
    format!("Start: {}", now.strftime("%Y-%m-%dT%H:%M:%S%z"))
}

/// `snprintf(buf + len, …)`: overwrite `buf` from display-width offset `at` (padding if shorter).
fn place(buf: &mut String, at: usize, s: &str) {
    // truncate to display width `at`, then pad to `at`
    let cut = crate::width::truncate_to(buf, at).len();
    buf.truncate(cut);
    for _ in display_width(buf)..at {
        buf.push(' ');
    }
    buf.push_str(s);
}

/// `print_mpls()` (report.c:151-159), 7 leading spaces.
fn mpls_lines(out: &mut String, labels: &[MplsLabel]) {
    for l in labels {
        let _ = writeln!(
            out,
            "       [MPLS: Lbl {} TC {} S {} TTL {}]",
            l.label,
            l.tc,
            u8::from(l.bottom_of_stack),
            l.ttl
        );
    }
}

pub fn render(ctx: &ReportContext<'_>) -> String {
    let e = ctx.engine;
    let cfg = e.config();
    let range = e.display_range();
    let ipinfo = !cfg.ipinfo_fields.is_empty();
    let names: Vec<String> = range
        .clone()
        .map(|at| hop_name(&e.hops()[at], ctx.names, cfg.dns, cfg.show_ips))
        .collect();

    // Widths (report.c:173-205).
    let mut len_hosts: usize = 33;
    let mut stat_start: usize = 33;
    if ctx.wide {
        len_hosts = display_width(ctx.local_hostname);
        for n in &names {
            len_hosts = len_hosts.max(display_width(n));
        }
    }
    let mut len_tmp = len_hosts;
    if ipinfo {
        len_tmp += asn::selected_width(&cfg.ipinfo_fields);
        stat_start = len_tmp;
        if ctx.wide {
            len_hosts += 1;
        }
    }

    let mut out = String::new();
    // Header (report.c:206-217): titles are written at `stat_start`, overwriting the padding.
    let mut buf = format!("HOST: {}", pad_right(ctx.local_hostname, len_tmp));
    let mut len = if ctx.wide {
        display_width(&buf)
    } else {
        stat_start
    };
    for f in &ctx.fields {
        place(&mut buf, len, &format_title(f));
        len += f.length;
    }
    out.push_str(&buf);
    out.push('\n');

    // Hop rows (report.c:219-348).
    for (i, at) in range.enumerate() {
        let hop = &e.hops()[at];
        let name = &names[i];
        let mut buf = if ipinfo {
            format!(
                " {:>2}. {}{}",
                at + 1,
                asn::format_selected(ctx.names.asn(hop.addr), &cfg.ipinfo_fields),
                pad_right(name, len_hosts)
            )
        } else {
            format!(" {:>2}.|-- {}", at + 1, pad_right(name, len_hosts))
        };
        let mut len = if ctx.wide {
            display_width(&buf)
        } else {
            stat_start
        };
        for f in &ctx.fields {
            place(&mut buf, len, &format_value(f, (f.value)(hop)));
            len += f.length;
        }
        out.push_str(&buf);
        out.push('\n');
        if cfg.mpls {
            mpls_lines(&mut out, &hop.mpls);
        }
        // Extra ECMP addresses (deviation 6: HAVE_IPINFO layout, each printed once).
        for a in hop.addrs.iter().take(cfg.max_display_path) {
            if Some(a.addr) == hop.addr {
                continue;
            }
            extra_row(&mut out, ctx, a, len_hosts, ipinfo);
        }
    }
    out
}

fn extra_row(
    out: &mut String,
    ctx: &ReportContext<'_>,
    a: &HopAddr,
    len_hosts: usize,
    ipinfo: bool,
) {
    let cfg = ctx.engine.config();
    let name = addr_name(Some(a.addr), ctx.names, cfg.dns, cfg.show_ips);
    if ipinfo {
        let _ = writeln!(
            out,
            "     {}{}",
            asn::format_selected(ctx.names.asn(Some(a.addr)), &cfg.ipinfo_fields),
            name
        );
    } else {
        let _ = writeln!(out, "        {}", pad_right(&name, len_hosts));
    }
    if cfg.mpls {
        mpls_lines(out, &a.mpls);
    }
}
