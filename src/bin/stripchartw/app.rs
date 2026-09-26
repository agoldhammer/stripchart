//! The window: a header with the channel legend, the scrolling chart, and a status line.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, Event, FontId, Key, Painter, Pos2, Rect, RichText,
    Sense, Shape, Stroke, Ui,
};
use stripchart::data::Recorder;
use stripchart::scale;
use stripchart::source::Msg;

const FRAME: Duration = Duration::from_millis(33);
const MIN_SPAN: f64 = 0.1;
const MAX_SPAN: f64 = 7.0 * 86400.0;

/// Width of the y-axis label column.
const GUTTER: f32 = 64.0;
/// Height of the time-axis label row.
const AXIS_H: f32 = 20.0;
const PAD: f32 = 10.0;
const LANE_GAP: f32 = 10.0;
const MIN_LANE_H: f32 = 60.0;
/// Minimum distance between vertical grid lines, in pixels.
const GRID_PX: f32 = 90.0;
/// Scroll-wheel travel per span step.
const WHEEL_STEP: f32 = 40.0;

const BG: Color32 = Color32::from_rgb(0x0e, 0x11, 0x16);
const PAPER: Color32 = Color32::from_rgb(0x13, 0x17, 0x1d);
const GRID: Color32 = Color32::from_rgb(0x2a, 0x31, 0x3b);
const AXIS: Color32 = Color32::from_rgb(0x46, 0x4f, 0x5c);
const DIM: Color32 = Color32::from_rgb(0x8c, 0x96, 0xa3);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xed);
const HIDDEN: Color32 = Color32::from_rgb(0x4a, 0x52, 0x5e);
const REC: Color32 = Color32::from_rgb(0xf0, 0x4a, 0x4a);
const WARN: Color32 = Color32::from_rgb(0xf0, 0xc8, 0x3c);

/// Pen colors, in the same order as the terminal version's.
const PENS: [Color32; 7] = [
    Color32::from_rgb(0xff, 0x5f, 0x5f),
    Color32::from_rgb(0x3f, 0xd0, 0xe0),
    Color32::from_rgb(0x5f, 0xd3, 0x5f),
    Color32::from_rgb(0xf0, 0xc8, 0x3c),
    Color32::from_rgb(0xe0, 0x70, 0xe0),
    Color32::from_rgb(0x6a, 0x96, 0xff),
    Color32::from_rgb(0xe0, 0xe0, 0xe0),
];

fn pen(i: usize) -> Color32 {
    PENS[i % PENS.len()]
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub struct Options {
    pub span: f64,
    pub lanes: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

pub struct App {
    rec: Recorder,
    rx: Receiver<Msg>,
    /// Seconds of history across the chart width.
    span: f64,
    lanes: bool,
    min: Option<f64>,
    max: Option<f64>,
    /// When paused, the time at the right edge.
    frozen: Option<f64>,
    /// How far back from the right edge we have scrolled, in seconds.
    pan: f64,
    /// Scale ranges held fixed with the `a` key, one per lane.
    locked: Option<Vec<(f64, f64)>>,
    last_ranges: Vec<(f64, f64)>,
    input: String,
    /// Scroll-wheel travel not yet turned into a span step.
    wheel: f32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rec: Recorder, rx: Receiver<Msg>, opts: Options) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.global_style_mut(|s| s.interaction.selectable_labels = false);
        App {
            rec,
            rx,
            span: opts.span,
            lanes: opts.lanes,
            min: opts.min,
            max: opts.max,
            frozen: None,
            pan: 0.0,
            locked: None,
            last_ranges: Vec::new(),
            input: "waiting for data".into(),
            wheel: 0.0,
        }
    }

    fn drain(&mut self) {
        // Bounded so a firehose on stdin can't starve the display.
        for _ in 0..50_000 {
            match self.rx.try_recv() {
                Ok(Msg::Line(at, line)) => match self.rec.ingest(at, &line) {
                    Ok(true) if self.input != "live" => self.input = "live".into(),
                    Ok(_) => {}
                    Err(e) => self.input = format!("CSV write error: {e}"),
                },
                Ok(Msg::Eof) => self.input = "input closed".into(),
                Ok(Msg::Error(e)) => self.input = e,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn keys(&mut self, ui: &Ui) {
        let events = ui.input(|i| i.events.clone());
        for ev in events {
            match ev {
                Event::Text(text) => {
                    for ch in text.chars() {
                        self.char_key(ch, ui);
                    }
                }
                Event::Key { key, pressed: true, .. } => self.named_key(key, ui),
                _ => {}
            }
        }
    }

    fn char_key(&mut self, ch: char, ui: &Ui) {
        match ch {
            'q' => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ' ' | 'p' => self.toggle_pause(),
            '+' | '=' => self.zoom_in(),
            '-' | '_' => self.zoom_out(),
            's' => self.toggle_lanes(),
            'a' => self.toggle_hold(),
            'c' => self.rec.clear(),
            d @ '1'..='9' => self.toggle_channel(d as usize - '1' as usize),
            _ => {}
        }
    }

    fn named_key(&mut self, key: Key, ui: &Ui) {
        let now = self.rec.now();
        match key {
            Key::Escape => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            Key::ArrowLeft | Key::PageUp => {
                self.frozen.get_or_insert(now);
                self.pan += if key == Key::ArrowLeft { self.span / 10.0 } else { self.span };
            }
            Key::ArrowRight | Key::PageDown if self.frozen.is_some() => {
                let step = if key == Key::ArrowRight { self.span / 10.0 } else { self.span };
                self.pan = (self.pan - step).max(0.0);
            }
            Key::End => self.go_live(),
            _ => {}
        }
    }

    fn toggle_pause(&mut self) {
        if self.frozen.take().is_none() {
            self.frozen = Some(self.rec.now());
        } else {
            self.pan = 0.0;
        }
    }

    fn go_live(&mut self) {
        self.frozen = None;
        self.pan = 0.0;
    }

    fn zoom_in(&mut self) {
        self.span = scale::TIME_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < self.span * 0.999 && s >= MIN_SPAN)
            .unwrap_or(self.span.min(MIN_SPAN));
    }

    fn zoom_out(&mut self) {
        self.span = scale::TIME_STEPS
            .iter()
            .copied()
            .find(|&s| s > self.span * 1.001)
            .unwrap_or(self.span * 2.0)
            .min(MAX_SPAN);
    }

    fn toggle_lanes(&mut self) {
        self.lanes = !self.lanes;
        self.locked = None;
    }

    fn toggle_hold(&mut self) {
        self.locked = match self.locked {
            Some(_) => None,
            None => Some(self.last_ranges.clone()),
        };
    }

    fn toggle_channel(&mut self, i: usize) {
        if let Some(ch) = self.rec.channels.get_mut(i) {
            ch.visible = !ch.visible;
            if self.lanes {
                self.locked = None;
            }
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        let now = self.rec.now();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if self.frozen.is_some() {
                ui.label(RichText::new("PAUSED").color(WARN).strong());
            } else {
                let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                if (now * 2.0) as u64 % 2 == 0 {
                    ui.painter().circle_filled(r.center(), 5.0, REC);
                }
                ui.label(RichText::new("REC").color(REC).strong());
            }
            ui.label(RichText::new(format!("span {}", scale::fmt_span(self.span))).color(DIM));
            if self.pan > 0.0 {
                ui.label(RichText::new(format!("{} back", scale::fmt_span(self.pan))).color(WARN));
            }
            ui.separator();
            if self.rec.channels.is_empty() {
                ui.label(RichText::new("no channels yet").color(DIM));
            }
            let mut toggle = None;
            for (i, ch) in self.rec.channels.iter().enumerate() {
                let color = if ch.visible { pen(i) } else { HIDDEN };
                let resp = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        if ch.visible {
                            ui.painter().rect_filled(r, CornerRadius::same(2), color);
                        } else {
                            ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, color), egui::StrokeKind::Inside);
                        }
                        ui.label(RichText::new(&ch.name).color(color));
                        let value = RichText::new(scale::fmt_reading(ch.last)).font(mono(13.0));
                        ui.label(if ch.visible { value.color(TEXT).strong() } else { value.color(HIDDEN) });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let hint = if i < 9 { format!(" (key {})", i + 1) } else { String::new() };
                if resp.on_hover_text(format!("Click to show or hide{hint}")).clicked() {
                    toggle = Some(i);
                }
                ui.add_space(8.0);
            }
            if let Some(i) = toggle {
                self.toggle_channel(i);
            }
        });
    }

    fn status(&self, ui: &mut Ui) {
        let rate = match self.rec.rate() {
            Some(r) if r >= 10.0 => format!("{r:.0}/s"),
            Some(r) if r >= 1.0 => format!("{r:.1}/s"),
            Some(r) => format!("{r:.2}/s"),
            None => "—/s".into(),
        };
        let right = format!("{} samples · {rate} · {}", self.rec.total, self.input);
        let keys = [
            ("q", "quit"),
            ("space", if self.frozen.is_some() { "resume" } else { "pause" }),
            ("arrows drag", "scroll"),
            ("+- wheel", "span"),
            ("s", if self.lanes { "overlay" } else { "lanes" }),
            ("a", if self.locked.is_some() { "autoscale" } else { "hold scale" }),
            ("c", "clear"),
            ("1-9", "channels"),
            ("End dbl-click", "live"),
        ];
        ui.horizontal(|ui| {
            let color = if self.input == "live" { DIM } else { WARN };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(right).color(color));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (key, what) in keys {
                        let job_w = ui.available_width();
                        if job_w < 90.0 {
                            break;
                        }
                        ui.label(RichText::new(key).color(TEXT).strong());
                        ui.label(RichText::new(what).color(DIM));
                        ui.add_space(8.0);
                    }
                });
            });
        });
    }

    fn chart(&mut self, ui: &mut Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let full = resp.rect;
        painter.rect_filled(full, 0.0, BG);
        let plot = Rect::from_min_max(
            pos2(full.left() + GUTTER, full.top() + PAD),
            pos2(full.right() - PAD, full.bottom() - AXIS_H),
        );
        if plot.width() < 60.0 || plot.height() < 40.0 {
            painter.text(full.center(), Align2::CENTER_CENTER, "window too small", FontId::proportional(14.0), REC);
            return;
        }
        let pw = plot.width() as f64;

        // Mouse: drag to scroll through time, wheel to change the span.
        let now = self.rec.now();
        if resp.dragged() {
            let dx = resp.drag_delta().x as f64;
            if dx != 0.0 {
                self.frozen.get_or_insert(now);
                self.pan = (self.pan + dx / pw * self.span).max(0.0);
            }
        }
        if resp.double_clicked() {
            self.go_live();
        }
        if resp.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            if d.x != 0.0 {
                self.frozen.get_or_insert(now);
                self.pan = (self.pan + d.x as f64 / pw * self.span).max(0.0);
            }
            self.wheel += d.y;
            while self.wheel >= WHEEL_STEP {
                self.wheel -= WHEEL_STEP;
                self.zoom_in();
            }
            while self.wheel <= -WHEEL_STEP {
                self.wheel += WHEEL_STEP;
                self.zoom_out();
            }
        }

        let t_end = self.frozen.unwrap_or(now) - self.pan;
        let t_start = t_end - self.span;
        let tstep = scale::time_step(self.span * GRID_PX as f64 / pw);
        let grid: Vec<(f32, f64)> = time_grid(t_start, t_end, tstep)
            .into_iter()
            .map(|t| (plot.left() + ((t - t_start) / self.span * pw) as f32, t))
            .collect();
        let win = self.rec.window(t_start, t_end);

        let visible: Vec<usize> = (0..self.rec.channels.len()).filter(|&i| self.rec.channels[i].visible).collect();
        let lanes: Vec<Vec<usize>> = if self.lanes
            && visible.len() > 1
            && plot.height() + LANE_GAP >= visible.len() as f32 * (MIN_LANE_H + LANE_GAP)
        {
            visible.iter().map(|&c| vec![c]).collect()
        } else {
            vec![visible]
        };

        let n = lanes.len();
        let lane_h = (plot.height() - LANE_GAP * (n - 1) as f32) / n as f32;
        let mut drawn = Vec::with_capacity(n);
        for (li, chans) in lanes.into_iter().enumerate() {
            let top = plot.top() + li as f32 * (lane_h + LANE_GAP);
            let lane = Lane {
                rect: Rect::from_min_size(pos2(plot.left(), top), vec2(plot.width(), lane_h)),
                range: self.range(li, &chans, win),
                t_start,
                span: self.span,
            };
            lane.draw(&painter, &self.rec, &chans, win, &grid, n > 1);
            drawn.push((lane, chans));
        }
        self.last_ranges = drawn.iter().map(|(l, _)| l.range).collect();

        // Time axis: elapsed time under each vertical grid line.
        let mut free = plot.left() - GUTTER / 2.0;
        for &(x, t) in &grid {
            let g = painter.layout_no_wrap(scale::fmt_elapsed(t, tstep), mono(12.0), DIM);
            let w = g.size().x;
            if x - w / 2.0 >= free && x + w / 2.0 <= full.right() {
                painter.galley(pos2(x - w / 2.0, plot.bottom() + 4.0), g, DIM);
                free = x + w / 2.0 + 12.0;
            }
        }

        if let Some(pos) = resp.hover_pos().filter(|p| plot.contains(*p) && !resp.dragged()) {
            self.readout(&painter, plot, pos, t_start, &drawn);
        }
    }

    /// Vertical scale for a lane: locked, or fitted to what's on screen.
    fn range(&self, li: usize, chans: &[usize], win: (usize, usize)) -> (f64, f64) {
        if let Some(r) = self.locked.as_ref().and_then(|l| l.get(li)) {
            return *r;
        }
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for smp in self.rec.history.range(win.0..win.1) {
            for &c in chans {
                let v = smp.get(c);
                if v.is_finite() {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
        let (mut lo, mut hi) = if lo <= hi {
            scale::auto_range(lo, hi)
        } else {
            self.last_ranges.get(li).copied().unwrap_or((-1.0, 1.0))
        };
        if let Some(m) = self.min {
            lo = m;
        }
        if let Some(m) = self.max {
            hi = m;
        }
        if !(hi > lo) {
            hi = lo + 1.0;
        }
        (lo, hi)
    }

    /// Cursor line with the time and every visible channel's value at the nearest sample.
    fn readout(&self, painter: &Painter, plot: Rect, pos: Pos2, t_start: f64, lanes: &[(Lane, Vec<usize>)]) {
        let hist = &self.rec.history;
        let px = self.span / plot.width() as f64;
        let t = t_start + (pos.x - plot.left()) as f64 * px;
        let i = hist.partition_point(|s| s.t < t);
        let nearest = [i.checked_sub(1), (i < hist.len()).then_some(i)]
            .into_iter()
            .flatten()
            .min_by(|&a, &b| (hist[a].t - t).abs().total_cmp(&(hist[b].t - t).abs()));
        // Only snap to a sample that is close to the cursor.
        let Some(smp) = nearest.map(|i| &hist[i]).filter(|s| (s.t - t).abs() <= px * 24.0) else { return };

        let x = plot.left() + ((smp.t - t_start) / px) as f32;
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, AXIS));

        let mut rows = vec![(format!("t {}", scale::fmt_elapsed(smp.t, px)), TEXT)];
        for (lane, chans) in lanes {
            for &c in chans {
                let v = smp.get(c);
                if v.is_finite() {
                    painter.circle_filled(pos2(x, lane.y(v)), 3.5, pen(c));
                }
                rows.push((format!("{} {}", self.rec.channels[c].name, scale::fmt_reading(v)), pen(c)));
            }
        }

        let galleys: Vec<_> = rows.into_iter().map(|(s, c)| painter.layout_no_wrap(s, mono(12.0), c)).collect();
        let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 12.0;
        let h = galleys.iter().map(|g| g.size().y).sum::<f32>() + 10.0;
        let left = if x + 12.0 + w <= plot.right() { x + 12.0 } else { x - 12.0 - w };
        let top = (pos.y - h / 2.0).clamp(plot.top(), (plot.bottom() - h).max(plot.top()));
        let bx = Rect::from_min_size(pos2(left, top), vec2(w, h));
        painter.rect_filled(bx, CornerRadius::same(4), Color32::from_black_alpha(215));
        painter.rect_stroke(bx, CornerRadius::same(4), Stroke::new(1.0, AXIS), egui::StrokeKind::Inside);
        let mut y = top + 5.0;
        for g in galleys {
            let gh = g.size().y;
            painter.galley(pos2(left + 6.0, y), g, TEXT);
            y += gh;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        ctx.request_repaint_after(FRAME);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.keys(ui);
        let bar = egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::top("header").frame(bar).show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status").frame(bar).show(ui, |ui| self.status(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG)).show(ui, |ui| self.chart(ui));
    }
}

/// Times of vertical grid lines. Lines are anchored to absolute time, so they
/// scroll along with the trace like printed divisions on chart paper.
fn time_grid(t_start: f64, t_end: f64, step: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut k = (t_start.max(0.0) / step).ceil() as i64;
    while (k as f64) * step <= t_end && out.len() < 1000 {
        out.push(k as f64 * step);
        k += 1;
    }
    out
}

struct Lane {
    rect: Rect,
    range: (f64, f64),
    t_start: f64,
    span: f64,
}

impl Lane {
    /// Screen y for a value; values off the scale are drawn at the edge.
    fn y(&self, v: f64) -> f32 {
        let (lo, hi) = self.range;
        let f = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        self.rect.bottom() - f * self.rect.height()
    }

    fn x(&self, t: f64) -> f32 {
        self.rect.left() + ((t - self.t_start) / self.span) as f32 * self.rect.width()
    }

    fn draw(
        &self,
        painter: &Painter,
        rec: &Recorder,
        chans: &[usize],
        win: (usize, usize),
        grid: &[(f32, f64)],
        label: bool,
    ) {
        let r = self.rect;
        painter.rect_filled(r, 0.0, PAPER);

        let (lo, hi) = self.range;
        let (ticks, step) = scale::ticks(lo, hi, ((r.height() / 45.0) as usize).clamp(2, 10));
        let grid_stroke = Stroke::new(1.0, GRID);
        for &(x, _) in grid {
            painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], grid_stroke);
        }
        let mut last_label = f32::INFINITY;
        for &t in &ticks {
            let y = self.y(t);
            painter.line_segment([pos2(r.left(), y), pos2(r.right(), y)], grid_stroke);
            painter.line_segment([pos2(r.left() - 5.0, y), pos2(r.left(), y)], Stroke::new(1.0, AXIS));
            // Ticks run bottom to top; skip a label that would overlap the one below.
            if last_label - y >= 14.0 {
                let text = scale::fmt_value(t, step);
                painter.text(pos2(r.left() - 8.0, y), Align2::RIGHT_CENTER, text, mono(12.0), DIM);
                last_label = y;
            }
        }
        painter.line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0, AXIS));

        // Include one sample either side so the trace runs off the edges.
        let a = win.0.saturating_sub(1);
        let b = (win.1 + 1).min(rec.history.len());
        let clip = painter.with_clip_rect(r.expand(1.0));
        for &c in chans {
            let stroke = Stroke::new(1.5, pen(c));
            let mut trace = Trace::default();
            for smp in rec.history.range(a..b) {
                let v = smp.get(c);
                if v.is_finite() {
                    trace.push(self.x(smp.t), self.y(v));
                } else {
                    trace.flush(&clip, stroke);
                }
            }
            trace.flush(&clip, stroke);
        }

        if let (true, &[c]) = (label, chans) {
            let name = &rec.channels[c].name;
            let g = painter.layout_no_wrap(name.clone(), FontId::proportional(13.0), pen(c));
            let bx = Rect::from_min_size(r.left_top() + vec2(6.0, 4.0), g.size() + vec2(10.0, 4.0));
            painter.rect_filled(bx, CornerRadius::same(3), Color32::from_black_alpha(160));
            painter.galley(bx.min + vec2(5.0, 2.0), g, pen(c));
        }
    }
}

/// Builds one pen stroke. When many samples fall in one pixel column, only
/// their first, lowest, highest and last values are kept, so the trace looks the
/// same but a long history stays cheap to draw.
#[derive(Default)]
struct Trace {
    points: Vec<Pos2>,
    col: Option<Column>,
}

struct Column {
    x: i64,
    first: Pos2,
    last: Pos2,
    min: f32,
    max: f32,
    n: usize,
}

impl Trace {
    fn push(&mut self, x: f32, y: f32) {
        let p = pos2(x, y);
        let xi = x.floor() as i64;
        match &mut self.col {
            Some(c) if c.x == xi => {
                c.last = p;
                c.min = c.min.min(y);
                c.max = c.max.max(y);
                c.n += 1;
            }
            _ => {
                self.end_column();
                self.col = Some(Column { x: xi, first: p, last: p, min: y, max: y, n: 1 });
            }
        }
    }

    fn end_column(&mut self) {
        let Some(c) = self.col.take() else { return };
        self.points.push(c.first);
        if c.n > 2 {
            // Go to the nearer extreme first, then the farther one.
            let x = c.first.x;
            let (a, b) = if c.first.y - c.min < c.max - c.first.y { (c.min, c.max) } else { (c.max, c.min) };
            self.points.push(pos2(x, a));
            self.points.push(pos2(x, b));
        }
        if c.n > 1 {
            self.points.push(c.last);
        }
    }

    /// Draw what has been collected (a gap in the data ends the stroke).
    fn flush(&mut self, painter: &Painter, stroke: Stroke) {
        self.end_column();
        match self.points.len() {
            0 => {}
            1 => {
                painter.circle_filled(self.points[0], stroke.width, stroke.color);
            }
            _ => {
                painter.add(Shape::line(std::mem::take(&mut self.points), stroke));
            }
        }
        self.points.clear();
    }
}
