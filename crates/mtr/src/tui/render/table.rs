//! Hop table (spec §8 item 2); ports the row layout of `mtr_curses_hosts()` (ui/curses.c:449-560,
//! mtr 0.96, commit 7b01773) to a scrolling, selectable table with a sparkline column. GPL-2.0-only.

use mtr_core::fields::{FieldFormat, active_fields, format_title, format_value};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::asn;
use crate::names::{addr_name, hop_name};
use crate::tui::render::View;
use crate::tui::render::sparkline::{Scale, cells_for_hop, glyph};
use crate::width::{display_width, pad_right, truncate_to};

pub const HOST_MIN: usize = 20;
pub const SPARK_MIN: usize = 8;
/// Row prefix: 1 cell of selection marker + `"{:>3}."` + one space = 6 cells.
const NUM_WIDTH: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Hop,
    /// Continuation row for `hop.addrs[i]` (an ECMP responder other than `hop.addr`).
    Extra(usize),
    /// `[MPLS: …]` row for label `label` of `hop.addrs[addr]`.
    Mpls {
        addr: usize,
        label: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableRow {
    pub at: usize,
    pub kind: RowKind,
}

pub fn rows(view: &View) -> Vec<TableRow> {
    let e = view.engine;
    let cfg = e.config();
    let mut out = Vec::new();
    for at in e.display_range() {
        let hop = &e.hops()[at];
        out.push(TableRow {
            at,
            kind: RowKind::Hop,
        });
        let primary = hop.addrs.iter().position(|a| Some(a.addr) == hop.addr);
        if cfg.mpls {
            if let Some(pi) = primary {
                for label in 0..hop.addrs[pi].mpls.len() {
                    out.push(TableRow {
                        at,
                        kind: RowKind::Mpls { addr: pi, label },
                    });
                }
            }
        }
        // curses.c:518-530: at most maxDisplayPath - 1 extra addresses, the primary skipped
        let mut shown = 0;
        for (i, a) in hop.addrs.iter().enumerate() {
            if Some(i) == primary {
                continue;
            }
            if shown + 1 >= cfg.max_display_path {
                break;
            }
            shown += 1;
            out.push(TableRow {
                at,
                kind: RowKind::Extra(i),
            });
            if cfg.mpls {
                for label in 0..a.mpls.len() {
                    out.push(TableRow {
                        at,
                        kind: RowKind::Mpls { addr: i, label },
                    });
                }
            }
        }
    }
    out
}

/// Index of `at`'s hop row. `ui.clamp` keeps `ui.scroll` inside `display_range()`, so a miss is a
/// bug in the caller, not a state the table should paper over silently.
pub fn first_row_of(rows: &[TableRow], at: usize) -> usize {
    match rows
        .iter()
        .position(|r| r.at == at && r.kind == RowKind::Hop)
    {
        Some(i) => i,
        None => {
            debug_assert!(rows.is_empty(), "scroll offset {at} is not a displayed hop");
            0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub num: usize,
    pub host: usize,
    pub stats: usize,
    pub spark: usize,
}

pub fn columns(view: &View, width: usize) -> Columns {
    let cfg = view.engine.config();
    let stats: usize = active_fields(&cfg.fields).iter().map(|f| f.length).sum();
    let ipinfo = asn::selected_width(&cfg.ipinfo_fields);
    let mut longest = HOST_MIN;
    for at in view.engine.display_range() {
        let h = &view.engine.hops()[at];
        longest = longest.max(display_width(&hop_name(
            h,
            view.names,
            cfg.dns,
            cfg.show_ips,
        )));
    }
    let host_wanted = longest + ipinfo;
    let fixed = NUM_WIDTH + stats + 1;
    let avail = width.saturating_sub(fixed);
    let mut spark = 0;
    let host = if view.ui.sparkline && avail >= HOST_MIN + ipinfo + SPARK_MIN {
        // give the host column what it wants, the sparkline the rest (min SPARK_MIN)
        let host = host_wanted.min(avail - SPARK_MIN);
        spark = avail - host;
        host
    } else {
        host_wanted.min(avail).max(HOST_MIN.min(avail))
    };
    Columns {
        num: NUM_WIDTH,
        host,
        stats,
        spark,
    }
}

fn stat_spans(view: &View, hop: &mtr_core::Hop) -> Vec<Span<'static>> {
    let cfg = view.engine.config();
    active_fields(&cfg.fields)
        .into_iter()
        .map(|f| {
            let v = (f.value)(hop);
            let style = match f.format {
                FieldFormat::Percent => view.palette.loss(v),
                FieldFormat::Ms5 | FieldFormat::Ms4 if hop.received() > 0 => {
                    view.palette.rtt(v.max(0) as u32)
                }
                _ => Style::new(),
            };
            Span::styled(format_value(f, v), style)
        })
        .collect()
}

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let e = view.engine;
    let cfg = e.config();
    let pal = view.palette;
    let g = view.glyphs;
    let cols = columns(view, usize::from(area.width));
    let range = e.display_range();
    let scale = Scale::from_hops(range.clone().map(|at| &e.hops()[at]));

    // sticky header
    let mut head = vec![Span::styled(
        format!("{:>w$}. ", "#", w = NUM_WIDTH - 2),
        pal.header(),
    )];
    head.push(Span::styled(pad_right("Host", cols.host), pal.header()));
    for f in active_fields(&cfg.fields) {
        head.push(Span::styled(format_title(f), pal.header()));
    }
    if cols.spark > 0 {
        head.push(Span::styled(
            format!(" {:>w$}", "Recent", w = cols.spark),
            pal.header(),
        ));
    }
    buf.set_line(area.x, area.y, &Line::from(head), area.width);

    let all = rows(view);
    let start = first_row_of(&all, view.ui.scroll);
    let ipinfo = !cfg.ipinfo_fields.is_empty();
    for (i, row) in all
        .iter()
        .skip(start)
        .take(usize::from(area.height) - 1)
        .enumerate()
    {
        let y = area.y + 1 + i as u16;
        let hop = &e.hops()[row.at];
        let selected = row.at == view.ui.selected;
        let mut spans: Vec<Span> = Vec::new();
        match row.kind {
            RowKind::Hop => {
                let marker = if selected { g.selected } else { " " };
                spans.push(Span::raw(format!("{marker}{:>3}. ", row.at + 1)));
                let mut name = hop_name(hop, view.names, cfg.dns, cfg.show_ips);
                if ipinfo && hop.err.is_none() && hop.addr.is_some() {
                    name = format!(
                        "{}{name}",
                        asn::format_selected(view.names.asn(hop.addr), &cfg.ipinfo_fields)
                    );
                }
                let name_style = if hop.is_unknown() && hop.err.is_none() {
                    pal.dim()
                } else if !hop.up || hop.addr == Some(e.target()) {
                    pal.bold()
                } else {
                    Style::new()
                };
                spans.push(Span::styled(
                    pad_right(truncate_to(&name, cols.host), cols.host),
                    name_style,
                ));
                spans.extend(stat_spans(view, hop));
                if cols.spark > 0 {
                    spans.push(Span::raw(" "));
                    for c in cells_for_hop(hop, cols.spark, &scale) {
                        let style = match c {
                            crate::tui::render::sparkline::Cell::Rtt(_, us) => pal.rtt(us),
                            crate::tui::render::sparkline::Cell::Lost => pal.lost_sample(),
                            crate::tui::render::sparkline::Cell::Pending => Style::new(),
                        };
                        spans.push(Span::styled(glyph(&c, g), style));
                    }
                }
            }
            RowKind::Extra(i) => {
                let a = &hop.addrs[i];
                let mut name = addr_name(Some(a.addr), view.names, cfg.dns, cfg.show_ips);
                if ipinfo {
                    name = format!(
                        "{}{name}",
                        asn::format_selected(view.names.asn(Some(a.addr)), &cfg.ipinfo_fields)
                    );
                }
                spans.push(Span::styled(
                    format!("{:w$}    {name}", "", w = NUM_WIDTH),
                    pal.dim(),
                ));
            }
            RowKind::Mpls { addr, label } => {
                let l = &hop.addrs[addr].mpls[label];
                spans.push(Span::styled(
                    format!(
                        "{:w$}    [MPLS: Lbl {} TC {} S {} TTL {}]",
                        "",
                        l.label,
                        l.tc,
                        u8::from(l.bottom_of_stack),
                        l.ttl,
                        w = NUM_WIDTH
                    ),
                    pal.dim(),
                ));
            }
        }
        buf.set_line(area.x, y, &Line::from(spans), area.width);
        if selected {
            buf.set_style(Rect::new(area.x, y, area.width, 1), pal.selected());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Answer, Fixture, drive, ip, row_text, view_fixture};
    use mtr_core::{Command, Config, Engine, Event};
    use mtr_proto::{MplsLabel, ProbeResult, ResponseKind};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use std::time::{Duration, Instant};

    /// One hop (max_ttl 1) answered by two addresses, the second with an MPLS label.
    /// A `Fixture` (not a `View`-returning closure: a closure cannot express the higher-ranked
    /// lifetime `|e: &Engine| -> View<'_>` needs).
    fn ecmp_fixture(max_display_path: usize) -> Fixture {
        let t0 = Instant::now();
        let cfg = Config {
            max_ttl: 1,
            max_display_path,
            ..Config::default()
        };
        let mut e = Engine::new(cfg, ip("192.0.2.10"), Some(ip("192.0.2.1")), t0, 1);
        let mut now = t0;
        for (from, lbl) in [
            ("10.0.0.1", vec![]),
            (
                "10.0.0.2",
                vec![MplsLabel {
                    label: 100,
                    tc: 0,
                    bottom_of_stack: true,
                    ttl: 1,
                }],
            ),
        ] {
            let cmds = e.handle(Event::Tick, now);
            let token = cmds
                .iter()
                .find_map(|c| match c {
                    Command::SendProbe { token, .. } => Some(*token),
                    _ => None,
                })
                .unwrap();
            e.handle(
                Event::Probe {
                    token,
                    kind: ResponseKind::Probe {
                        result: ProbeResult::TtlExpired,
                        addr: ip(from),
                        rtt_us: 1000,
                        mpls: lbl,
                    },
                },
                now,
            );
            now = cmds
                .iter()
                .find_map(|c| match c {
                    Command::NextWake(t) => Some(*t),
                    _ => None,
                })
                .unwrap();
        }
        Fixture::around(e, now)
    }

    #[test]
    fn rows_follow_display_range_with_extras_and_mpls() {
        let f = view_fixture();
        let r = rows(&f.view());
        assert_eq!(r.iter().map(|r| r.at).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(r.iter().all(|r| matches!(r.kind, RowKind::Hop)));

        let mut f = ecmp_fixture(8);
        let r = rows(&f.view());
        assert!(
            matches!(
                r[..],
                [
                    TableRow {
                        at: 0,
                        kind: RowKind::Hop
                    },
                    TableRow {
                        at: 0,
                        kind: RowKind::Extra(0)
                    }
                ]
            ),
            "{r:?}"
        );
        // primary addr is the latest responder (10.0.0.2); the extra row is addrs[0] = 10.0.0.1
        f.engine
            .handle(Event::Action(mtr_core::UserAction::ToggleMpls), f.now);
        let r = rows(&f.view());
        assert!(
            matches!(
                r[..],
                [
                    TableRow {
                        at: 0,
                        kind: RowKind::Hop
                    },
                    TableRow {
                        at: 0,
                        kind: RowKind::Mpls { addr: 1, label: 0 }
                    },
                    TableRow {
                        at: 0,
                        kind: RowKind::Extra(0)
                    }
                ]
            ),
            "{r:?}"
        );
        assert_eq!(first_row_of(&r, 0), 0);
    }

    #[test]
    fn max_display_path_limits_extra_rows() {
        // -E 1: curses.c:518 loops `for (i = 1; i < maxDisplayPath; i++)` → no extra rows at all
        let f = ecmp_fixture(1);
        assert!(
            matches!(
                rows(&f.view())[..],
                [TableRow {
                    at: 0,
                    kind: RowKind::Hop
                }]
            ),
            "{:?}",
            rows(&f.view())
        );
        // an engine with no replies yet has an empty display range → no rows
        let fresh = Fixture::around(
            Engine::new(Config::default(), ip("192.0.2.10"), None, Instant::now(), 1),
            Instant::now(),
        );
        assert!(rows(&fresh.view()).is_empty(), "no hops yet");
    }

    #[test]
    fn renders_header_rows_selection_and_sparkline() {
        let mut f = view_fixture();
        f.ui.selected = 2;
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let head = row_text(&buf, 0);
        assert!(head.contains("Host"), "{head:?}");
        assert!(
            head.contains("Loss%") && head.contains("StDev") && head.trim_end().ends_with("Recent"),
            "{head:?}"
        );
        let r1 = row_text(&buf, 1);
        assert!(r1.starts_with("   1. gw.example"), "{r1:?}");
        assert!(
            r1.contains("  0.00%     2    0.5   0.5   0.5   0.5   0.0"),
            "{r1:?}"
        );
        let r2 = row_text(&buf, 2);
        assert!(r2.starts_with("   2. ???"), "{r2:?}");
        assert!(r2.contains("100.00%     2    0.0"), "{r2:?}");
        assert!(
            !r2.contains('×'),
            "a hop that never answered has a blank sparkline, not lost marks: {r2:?}"
        );
        let r3 = row_text(&buf, 3);
        assert!(r3.starts_with("▶  3. 192.0.2.10"), "{r3:?}");
        assert!(
            r3.trim_end().ends_with("██"),
            "target has the slowest RTT: {r3:?}"
        );
        assert!(row_text(&buf, 4).trim().is_empty());
        // selection style spans the row
        assert!(
            buf.cell((10, 3))
                .unwrap()
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        assert!(
            !buf.cell((10, 1))
                .unwrap()
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }

    #[test]
    fn sparkline_column_hides_when_toggled_or_too_narrow() {
        let mut f = view_fixture();
        let c = columns(&f.view(), 80);
        assert!(
            c.spark >= SPARK_MIN && c.num + c.host + c.stats + c.spark <= 80,
            "{c:?}"
        );
        f.ui.sparkline = false;
        assert_eq!(columns(&f.view(), 80).spark, 0);
        f.ui.sparkline = true;
        let c = columns(&f.view(), 60);
        assert_eq!(c.spark, 0, "no room for a sparkline at 60 columns: {c:?}");
        assert!(c.num + c.host + c.stats <= 60, "{c:?}");
    }

    #[test]
    fn scroll_starts_at_the_first_row_of_the_scrolled_hop() {
        let mut f = view_fixture();
        f.ui.scroll = 1;
        f.ui.selected = 1;
        let area = Rect::new(0, 0, 80, 3);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        assert!(
            row_text(&buf, 1).starts_with("▶  2. ???"),
            "{:?}",
            row_text(&buf, 1)
        );
        assert!(row_text(&buf, 2).starts_with("   3. 192.0.2.10"));
    }

    #[test]
    fn lost_sample_on_an_answering_hop_is_a_thin_red_mark_not_the_full_loss_style() {
        let cfg = Config {
            max_ttl: 1,
            max_ping: 2,
            force_max_ping: true,
            grace_time: 0.1,
            ..Config::default()
        };
        // ttl 1 answers on the first cycle, then goes quiet: an answering hop with one drop.
        let (engine, end) = drive(cfg, |_ttl, cycle| match cycle {
            1 => Answer::Reply {
                addr: ip("10.0.0.1"),
                rtt_us: 500,
                mpls: vec![],
            },
            _ => Answer::NoReply,
        });
        let f = Fixture::around(engine, end + Duration::from_secs(1));
        assert!(f.engine.hops()[0].received() > 0, "hop did answer once");

        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let r1 = row_text(&buf, 1);
        let x = r1
            .chars()
            .position(|c| c == '×')
            .unwrap_or_else(|| panic!("no lost mark in {r1:?}")) as u16;
        let cell = buf.cell((x, 1)).unwrap();
        assert_eq!(cell.fg, Color::Red, "{r1:?}");
        assert!(
            !cell.modifier.contains(Modifier::BOLD),
            "a single drop is not the bold 100%-loss style: {r1:?}"
        );
    }
}
