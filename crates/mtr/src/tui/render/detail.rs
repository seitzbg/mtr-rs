//! Detail pane for the selected hop: RTT chart, Addresses, Log (spec §8 item 3). GPL-2.0-only.

use std::time::Instant;

use mtr_core::{Hop, Sample};
use mtr_proto::ProbeResult;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Tabs, Widget};

use crate::asn;
use crate::names::addr_name;
use crate::tui::render::{View, ms};
use crate::tui::state::DetailTab;
use crate::width::{display_width, pad_right, truncate_with_ellipsis};

pub fn nice_ceiling(max_ms: f64) -> f64 {
    if max_ms.is_nan() || max_ms <= 1.0 {
        return 1.0;
    }
    let exp = max_ms.log10().floor();
    let base = 10f64.powf(exp);
    for m in [1.0, 2.0, 5.0, 10.0] {
        if max_ms <= m * base {
            return m * base;
        }
    }
    10.0 * base
}

pub struct ChartData {
    pub points: Vec<(f64, f64)>,
    pub lost_columns: Vec<bool>,
    pub span_s: f64,
    pub ceiling: f64,
}

pub fn chart_points(hop: &Hop, columns: usize, now: Instant, interval_s: f64) -> ChartData {
    let n = columns * 2;
    let entries: Vec<_> = hop.history.entries().collect();
    let skip = entries.len().saturating_sub(n);
    let window = &entries[skip..];
    let offset = n - window.len();
    let mut points = Vec::new();
    let mut lost_columns = vec![false; columns];
    let mut max_ms = 0.0f64;
    for (i, e) in window.iter().enumerate() {
        let x = offset + i;
        match e.sample {
            Sample::Rtt(us) => {
                let y = f64::from(us) / 1000.0;
                max_ms = max_ms.max(y);
                points.push((x as f64, y));
            }
            Sample::Lost => lost_columns[x / 2] = true,
            Sample::Pending { .. } => {}
        }
    }
    // The window is right-anchored (newest sample in the last column); when history is shorter
    // than the window, `offset` empty slots sit to its left. The left label should describe the
    // whole window's age, not just the oldest sample's, so add back the time those empty slots
    // represent.
    let span_s = window
        .first()
        .map_or(0.0, |e| now.saturating_duration_since(e.sent).as_secs_f64())
        + (offset as f64) * interval_s;
    ChartData {
        points,
        lost_columns,
        span_s,
        ceiling: nice_ceiling(max_ms),
    }
}

pub fn title_line(view: &View, at: usize) -> String {
    let hop = &view.engine.hops()[at];
    let cfg = view.engine.config();
    let mut s = format!("Hop {}", at + 1);
    match hop.addr {
        None => s.push_str("  ???"),
        Some(ip) => {
            let name = addr_name(Some(ip), view.names, cfg.dns, false);
            if name != ip.to_string() {
                s.push_str(&format!("  {name}"));
            }
            s.push_str(&format!("  {ip}"));
            if let Some(info) = view.names.asn(Some(ip)) {
                // asn::format_field pads to iiwidth; the header wants the bare "AS<n>"
                s.push_str(&format!(
                    "  {}",
                    asn::format_field(Some(info), 0).trim_end()
                ));
            }
            if let Some(name) = view.names.asn_name(Some(ip)) {
                s.push_str(&format!("  {name}"));
            }
        }
    }
    s
}

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    let g = view.glyphs;
    let pal = view.palette;
    let range = view.engine.display_range();
    let title = if range.contains(&view.ui.selected) {
        title_line(view, view.ui.selected)
    } else {
        "Hop -".to_string()
    };
    let block = Block::bordered()
        .border_set(g.border)
        .title(Line::from(Span::styled(format!(" {title} "), pal.bold())));
    let inner = block.inner(area);
    block.render(area, buf);
    // tabs in the top border, right-aligned
    let tabs_w = 26u16.min(area.width.saturating_sub(4));
    let tabs_area = Rect::new(area.x + area.width - tabs_w - 1, area.y, tabs_w, 1);
    Tabs::new(
        [DetailTab::Rtt, DetailTab::Addresses, DetailTab::Log]
            .iter()
            .map(|t| t.title()),
    )
    .select(match view.ui.tab {
        DetailTab::Rtt => 0,
        DetailTab::Addresses => 1,
        DetailTab::Log => 2,
    })
    .divider(g.divider)
    .style(pal.dim())
    .highlight_style(pal.header())
    .render(tabs_area, buf);
    if !range.contains(&view.ui.selected) {
        let msg = format!("waiting for the first reply{}", g.ellipsis);
        buf.set_string(inner.x, inner.y, &msg, pal.dim());
        return;
    }
    let hop = &view.engine.hops()[view.ui.selected];
    match view.ui.tab {
        DetailTab::Rtt => render_rtt(view, hop, inner, buf),
        DetailTab::Addresses => render_addresses(view, hop, inner, buf),
        DetailTab::Log => render_log(view, hop, inner, buf),
    }
}

/// Width of the y-axis label column (`"100 ms"` = 6) + 1 for the axis line.
fn y_label_width(ceiling: f64) -> u16 {
    format!("{ceiling} ms").len() as u16 + 1
}

fn render_rtt(view: &View, hop: &Hop, inner: Rect, buf: &mut Buffer) {
    let [chart_area, loss_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    // The label column width depends on the ceiling, the ceiling on the window, the window on the
    // column count: start from the hop's worst RTT, then settle on the window's own ceiling so the
    // loss row below is offset by exactly the labels the axis prints.
    let interval_s = view.engine.config().interval;
    let mut label_w = y_label_width(nice_ceiling(f64::from(hop.worst()) / 1000.0));
    let mut columns = usize::from(chart_area.width.saturating_sub(label_w)).max(1);
    let mut data = chart_points(hop, columns, view.now, interval_s);
    if y_label_width(data.ceiling) != label_w {
        label_w = y_label_width(data.ceiling);
        columns = usize::from(chart_area.width.saturating_sub(label_w)).max(1);
        data = chart_points(hop, columns, view.now, interval_s);
    }
    let label_w = y_label_width(data.ceiling);
    let pal = view.palette;
    if view.glyphs.ascii {
        // deviation 23: ratatui's Chart draws its axes with `─│└` and its points with the marker
        // set, none of which is ASCII, so `--ascii` gets its own scatter over the same data.
        render_rtt_ascii(view, &data, label_w, columns, chart_area, buf);
    } else {
        render_rtt_chart(view, &data, columns, chart_area, buf);
    }
    // loss row under the plot, aligned with the plotting columns
    let x0 = loss_area.x + label_w;
    for (c, lost) in data.lost_columns.iter().enumerate() {
        if *lost {
            let x = x0 + c as u16;
            if x < loss_area.x + loss_area.width {
                buf.set_string(x, loss_area.y, view.glyphs.lost_mark, pal.lost_sample());
            }
        }
    }
}

/// `*` points on `|`/`-`/`+` axes: the same picture the ratatui `Chart` draws, in ASCII only.
fn render_rtt_ascii(
    view: &View,
    data: &ChartData,
    label_w: u16,
    columns: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let pal = view.palette;
    // two rows go to the axis line and the x labels, one column of the label block to the y axis
    if area.height < 3 || area.width <= label_w || label_w == 0 {
        return;
    }
    let plot_h = area.height - 2;
    let axis_x = area.x + label_w - 1;
    let plot_x = area.x + label_w;
    let axis_y = area.y + plot_h;
    let cols = columns.min(usize::from(area.width - label_w));
    let lab_w = usize::from(label_w - 1);
    let mut y_label = |row: u16, text: String| {
        buf.set_string(area.x, row, format!("{text:>lab_w$}"), pal.dim());
    };
    // rows run top (ceiling) to bottom (0); the half label goes on the row a half-ceiling point
    // would be plotted on, so label and plot agree
    let row_of = |frac: f64| area.y + plot_h - 1 - (frac * f64::from(plot_h - 1)).round() as u16;
    y_label(area.y, format!("{} ms", data.ceiling));
    if plot_h >= 2 {
        y_label(area.y + plot_h - 1, "0".to_string());
    }
    let half = row_of(0.5);
    if half != area.y && half != area.y + plot_h - 1 {
        y_label(half, format!("{}", data.ceiling / 2.0));
    }
    for r in 0..plot_h {
        buf.set_string(axis_x, area.y + r, "|", pal.dim());
    }
    buf.set_string(axis_x, axis_y, "+", pal.dim());
    buf.set_string(plot_x, axis_y, "-".repeat(cols), pal.dim());
    for &(x, y) in &data.points {
        let c = x as usize / 2;
        if c >= cols {
            continue;
        }
        let frac = (y / data.ceiling).clamp(0.0, 1.0);
        buf.set_string(plot_x + c as u16, row_of(frac), "*", pal.accent());
    }
    let left = format!("-{:.0}s", data.span_s);
    buf.set_string(plot_x, axis_y + 1, &left, pal.dim());
    let now_w = 3u16;
    if cols as u16 > left.len() as u16 + now_w {
        buf.set_string(plot_x + cols as u16 - now_w, axis_y + 1, "now", pal.dim());
    }
}

fn render_rtt_chart(view: &View, data: &ChartData, columns: usize, area: Rect, buf: &mut Buffer) {
    let pal = view.palette;
    let ds = Dataset::default()
        .data(&data.points)
        .marker(view.glyphs.marker)
        .graph_type(GraphType::Line)
        .style(pal.accent());
    let x_axis = Axis::default()
        .bounds([0.0, (columns * 2) as f64])
        .labels(vec![
            Span::styled(format!("-{:.0}s", data.span_s), pal.dim()),
            Span::styled("now", pal.dim()),
        ])
        .style(pal.dim());
    let y_axis = Axis::default()
        .bounds([0.0, data.ceiling])
        .labels(vec![
            Span::styled("0", pal.dim()),
            Span::styled(format!("{}", data.ceiling / 2.0), pal.dim()),
            Span::styled(format!("{} ms", data.ceiling), pal.dim()),
        ])
        .labels_alignment(Alignment::Right)
        .style(pal.dim());
    Chart::new(vec![ds])
        .x_axis(x_axis)
        .y_axis(y_axis)
        .legend_position(None)
        .render(area, buf);
}

/// Fixed Addresses columns: ASN, Count, Last, First seen and the five two-cell separators.
const ASN_W: usize = 8;
const COUNT_W: usize = 5;
const LAST_W: usize = 6;
const FIRST_W: usize = 10;
const SEPARATORS: usize = 10;
/// Neither text column ever shrinks below this.
const MIN_TEXT_W: usize = 4;

/// Split what the fixed columns leave over between IP and Name: the address column takes the width
/// it needs, but never less than 60 % of the remainder, and both keep `MIN_TEXT_W` columns.
fn address_columns(width: usize, widest_ip: usize) -> (usize, usize) {
    let rem = width
        .saturating_sub(ASN_W + COUNT_W + LAST_W + FIRST_W + SEPARATORS)
        .max(2 * MIN_TEXT_W);
    let hi = rem - MIN_TEXT_W;
    let lo = ((rem * 3).div_ceil(5)).min(hi);
    let ip = widest_ip.clamp(lo, hi);
    (ip, rem - ip)
}

fn render_addresses(view: &View, hop: &Hop, inner: Rect, buf: &mut Buffer) {
    let cfg = view.engine.config();
    let pal = view.palette;
    let ell = view.glyphs.ellipsis;
    let widest = hop
        .addrs
        .iter()
        .map(|a| display_width(&a.addr.to_string()))
        .max()
        .unwrap_or(0);
    let (ip_w, name_w) = address_columns(usize::from(inner.width), widest);
    let head = format!(
        "{}  {}  {}  {:>COUNT_W$}  {:>LAST_W$}  {}",
        pad_right("IP", ip_w),
        pad_right("Name", name_w),
        pad_right("ASN", ASN_W),
        "Count",
        "Last",
        "First seen"
    );
    buf.set_string(inner.x, inner.y, &head, pal.header());
    for (i, a) in hop
        .addrs
        .iter()
        .take(usize::from(inner.height.saturating_sub(1)))
        .enumerate()
    {
        let name = view.names.name(a.addr).filter(|_| cfg.dns).unwrap_or("-");
        let asn = view
            .names
            .asn(Some(a.addr))
            .map(|i| asn::format_field(Some(i), 0).trim_end().to_string())
            .unwrap_or_else(|| "-".into());
        let first = view.now.saturating_duration_since(a.first_seen).as_secs();
        let row = format!(
            "{}  {}  {}  {:>COUNT_W$}  {:>LAST_W$}  -{first}s",
            pad_right(
                &truncate_with_ellipsis(&a.addr.to_string(), ip_w, ell),
                ip_w
            ),
            pad_right(&truncate_with_ellipsis(name, name_w, ell), name_w),
            pad_right(&truncate_with_ellipsis(&asn, ASN_W, ell), ASN_W),
            a.count,
            ms(a.last_rtt as i32)
        );
        let style = if Some(a.addr) == hop.addr {
            Style::new()
        } else {
            pal.dim()
        };
        buf.set_string(inner.x, inner.y + 1 + i as u16, &row, style);
    }
}

fn render_log(view: &View, hop: &Hop, inner: Rect, buf: &mut Buffer) {
    let pal = view.palette;
    for (i, e) in hop
        .history
        .entries()
        .rev()
        .take(usize::from(inner.height))
        .enumerate()
    {
        let age = view.now.saturating_duration_since(e.sent).as_secs_f64();
        let (result, rtt, style) = match (e.sample, e.result) {
            (Sample::Rtt(us), Some(ProbeResult::Reply)) => ("reply", ms(us as i32), Style::new()),
            (Sample::Rtt(us), Some(ProbeResult::TtlExpired)) => {
                ("ttl-expired", ms(us as i32), Style::new())
            }
            (Sample::Rtt(us), Some(ProbeResult::NoRouteHost)) => {
                ("no-route-host", ms(us as i32), pal.alert())
            }
            (Sample::Rtt(us), None) => ("reply", ms(us as i32), Style::new()),
            (Sample::Lost, _) => ("no-reply", "-".to_string(), pal.loss(100_000)),
            (Sample::Pending { .. }, _) => ("pending", "-".to_string(), pal.dim()),
        };
        let mpls: Vec<String> = e
            .mpls
            .iter()
            .map(|l| {
                format!(
                    "Lbl {}/{}/{}/{}",
                    l.label,
                    l.tc,
                    u8::from(l.bottom_of_stack),
                    l.ttl
                )
            })
            .collect();
        let row = format!(
            "#{:<5} {:>7.1}s  {:<13} {:>7}  {}",
            e.seq,
            -age,
            result,
            rtt,
            mpls.join(", ")
        );
        buf.set_string(inner.x, inner.y + i as u16, &row, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{row_text, view_fixture};
    use crate::tui::state::DetailTab;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::time::Duration;

    #[test]
    fn nice_ceilings() {
        assert_eq!(nice_ceiling(0.0), 1.0);
        assert_eq!(nice_ceiling(0.7), 1.0);
        assert_eq!(nice_ceiling(1.0), 1.0);
        assert_eq!(nice_ceiling(1.1), 2.0);
        assert_eq!(nice_ceiling(3.0), 5.0);
        assert_eq!(nice_ceiling(7.5), 10.0);
        assert_eq!(nice_ceiling(42.0), 50.0);
        assert_eq!(nice_ceiling(120.0), 200.0);
    }

    #[test]
    fn chart_points_take_two_samples_per_column_and_flag_lost_columns() {
        let f = view_fixture();
        let interval_s = f.engine.config().interval;
        let hop = &f.engine.hops()[1]; // all lost
        let d = chart_points(hop, 10, f.now, interval_s);
        assert!(d.points.is_empty());
        assert_eq!(d.lost_columns.len(), 10);
        assert!(
            d.lost_columns[9],
            "the two lost samples (x = 18, 19) share the last column"
        );
        assert!(!d.lost_columns[8] && !d.lost_columns[0]);
        let hop = &f.engine.hops()[0]; // two replies at 0.5 ms
        let d = chart_points(hop, 10, f.now, interval_s);
        assert_eq!(d.points, vec![(18.0, 0.5), (19.0, 0.5)]);
        assert_eq!(d.ceiling, 1.0);
        assert!(d.lost_columns.iter().all(|l| !l));
    }

    /// With only 3 hops' worth of samples on a wide pane, most of the window is padding: the
    /// left label must describe the whole window's age, not the oldest sample's, so it comes out
    /// far larger than `span_s` would be if it only covered the real samples.
    #[test]
    fn chart_span_covers_the_whole_window_not_just_the_oldest_sample() {
        let f = view_fixture();
        let interval_s = f.engine.config().interval;
        let hop = &f.engine.hops()[0]; // two replies at 0.5 ms, nothing else
        let columns = 100; // window = 200 slots, only 2 filled
        let d = chart_points(hop, columns, f.now, interval_s);
        let oldest_age = hop
            .history
            .entries()
            .next()
            .map(|e| f.now.saturating_duration_since(e.sent).as_secs_f64())
            .unwrap();
        let padding = (columns * 2 - hop.history.entries().count()) as f64;
        assert_eq!(d.span_s, oldest_age + padding * interval_s);
        assert!(
            d.span_s > oldest_age * 10.0,
            "span_s ({}) should be much larger than the oldest sample's age ({oldest_age})",
            d.span_s
        );
    }

    #[test]
    fn title_and_tabs() {
        let mut f = view_fixture();
        let mut info = crate::asn::parse_txt("64500 | 192.0.2.0/24 | EX | ripe | 2020-01-01");
        info.name = Some("EXAMPLE-AS".into());
        f.names.insert_asn("192.0.2.10".parse().unwrap(), info);
        f.ui.selected = 2;
        assert_eq!(
            title_line(&f.view(), 2),
            "Hop 3  192.0.2.10  AS64500  EXAMPLE-AS"
        );
        assert_eq!(title_line(&f.view(), 0), "Hop 1  gw.example  10.0.0.1");
        let area = Rect::new(0, 0, 80, 9);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let top = row_text(&buf, 0);
        assert!(
            top.contains("Hop 3  192.0.2.10  AS64500  EXAMPLE-AS"),
            "{top:?}"
        );
        assert!(
            top.contains("RTT") && top.contains("Addresses") && top.contains("Log"),
            "{top:?}"
        );
        // the y axis has exactly one " ms" label and ratatui prints it on the chart's first row
        // (inner row 0 = buffer row 1); rows 2..6 are plot, row 7 is the loss row
        assert!(
            row_text(&buf, 1).contains("5 ms"),
            "top y label: {:?}",
            row_text(&buf, 1)
        );
        assert!(
            (2..8).all(|y| !row_text(&buf, y).contains(" ms")),
            "only one y label carries the unit"
        );
    }

    /// One hop answering from a long IPv6 address, for the truncation cases.
    fn ipv6_fixture() -> crate::testing::Fixture {
        use crate::testing::{Answer, drive, ip};
        let cfg = mtr_core::Config {
            max_ping: 1,
            force_max_ping: true,
            grace_time: 0.1,
            ..mtr_core::Config::default()
        };
        let (engine, end) = drive(cfg, |ttl, _| Answer::Reply {
            addr: if ttl == 1 {
                ip("2001:db8:85a3::8a2e:370:7334")
            } else {
                ip("192.0.2.10")
            },
            rtt_us: 500,
            mpls: vec![],
        });
        let mut f = crate::testing::Fixture::around(engine, end + Duration::from_secs(1));
        f.ui.tab = DetailTab::Addresses;
        f
    }

    #[test]
    fn ascii_rtt_pane_is_pure_ascii_and_plots_stars() {
        let mut f = view_fixture();
        f.ui.selected = 0;
        f.ui.tab = DetailTab::Rtt;
        f.glyphs = crate::tui::glyphs::Glyphs::select(true);
        let area = Rect::new(0, 0, 80, 9);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let screen: String = (0..area.height).map(|y| row_text(&buf, y)).collect();
        assert!(
            screen.is_ascii(),
            "non-ASCII in the --ascii RTT pane:\n{screen}"
        );
        assert!(screen.contains('*'), "no plotted point:\n{screen}");
        assert!(screen.contains('+') && screen.contains('|'), "{screen}");
        // the axis row is drawn with '-' and the y labels keep their unit
        assert!(screen.contains("---"), "{screen}");
        assert!(screen.contains(" ms") && screen.contains("now"), "{screen}");
    }

    #[test]
    fn addresses_fit_the_narrowest_pane_and_mark_truncation() {
        let mut f = view_fixture();
        f.ui.selected = 0;
        f.ui.tab = DetailTab::Addresses;
        let area = Rect::new(0, 0, crate::tui::render::MIN_WIDTH, 9);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let head = row_text(&buf, 1);
        for c in ["IP", "Name", "ASN", "Count", "Last", "First seen"] {
            assert!(head.contains(c), "{c} missing from {head:?}");
        }
        let r = row_text(&buf, 2);
        assert!(
            r.contains("10.0.0.1")
                && r.contains(" 2 ")
                && r.contains("0.5")
                && r.trim_end_matches(['│', ' ']).ends_with('s'),
            "{r:?}"
        );
        // an address wider than its column keeps a marker to say so
        let mut f = ipv6_fixture();
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let r = row_text(&buf, 2);
        assert!(r.contains("2001:db8") && r.contains('…'), "{r:?}");
        assert!(row_text(&buf, 1).contains("First seen"), "narrow header");
        f.glyphs = crate::tui::glyphs::Glyphs::select(true);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let r = row_text(&buf, 2);
        assert!(
            r.contains("2001:db8") && r.contains('~') && r.is_ascii(),
            "{r:?}"
        );
    }

    #[test]
    fn addresses_and_log_tabs_list_rows() {
        let mut f = view_fixture();
        f.ui.selected = 0;
        f.ui.tab = DetailTab::Addresses;
        let area = Rect::new(0, 0, 80, 9);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        assert!(
            row_text(&buf, 1).contains("IP") && row_text(&buf, 1).contains("Count"),
            "{:?}",
            row_text(&buf, 1)
        );
        let r = row_text(&buf, 2);
        assert!(
            r.contains("10.0.0.1")
                && r.contains("gw.example")
                && r.contains(" 2 ")
                && r.contains("0.5"),
            "{r:?}"
        );
        f.ui.tab = DetailTab::Log;
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        let r = row_text(&buf, 1);
        assert!(
            r.contains("#2") && r.contains("ttl-expired") && r.contains("0.5"),
            "newest first: {r:?}"
        );
        assert!(row_text(&buf, 2).contains("#1"));
        f.ui.selected = 1;
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        // the fixture answers ttl 2 with `no-reply`, and the engine is finished: nothing is pending
        assert!(
            row_text(&buf, 1).contains("no-reply"),
            "{:?}",
            row_text(&buf, 1)
        );
    }
}
