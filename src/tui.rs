//! Full-screen, horizontally scrolling chart drawn with braille dots.

use std::io::{self, Write};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};

use crate::data::{palette, Recorder};
use crate::scale;
use crate::source::Msg;

/// Width of the y-axis label column, including the axis line.
const GUTTER: usize = 10;
const FRAME: Duration = Duration::from_millis(33);
const GRID: Color = Color::DarkGrey;
const DIM: Color = Color::Grey;
const MIN_SPAN: f64 = 0.1;
const MAX_SPAN: f64 = 7.0 * 86400.0;

pub struct Options {
    pub span: f64,
    pub lanes: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

pub fn run(rec: &mut Recorder, rx: Receiver<Msg>, opts: Options) -> io::Result<()> {
    let _guard = TermGuard::enter()?;
    let mut out = io::BufWriter::with_capacity(1 << 16, io::stdout());
    let mut ui = Ui {
        span: opts.span,
        lanes: opts.lanes,
        min: opts.min,
        max: opts.max,
        frozen: None,
        pan: 0.0,
        locked: None,
        last_ranges: Vec::new(),
        input: "waiting for data".into(),
    };

    let mut last_draw: Option<Instant> = None;
    loop {
        ui.drain(rec, &rx)?;
        if last_draw.map_or(true, |t| t.elapsed() >= FRAME) {
            ui.draw(rec, &mut out)?;
            last_draw = Some(Instant::now());
        }
        let wait = last_draw.map_or(Duration::ZERO, |t| FRAME.saturating_sub(t.elapsed()));
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if !ui.key(k, rec) {
                        return Ok(());
                    }
                    last_draw = None;
                }
                Event::Resize(..) => last_draw = None,
                _ => {}
            }
        }
    }
}

/// Puts the terminal in raw/alternate-screen mode and restores it on drop or panic.
struct TermGuard;

impl TermGuard {
    fn enter() -> io::Result<Self> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            hook(info);
        }));
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = execute!(io::stdout(), SetAttribute(Attribute::Reset), cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

struct Ui {
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
}

impl Ui {
    fn drain(&mut self, rec: &mut Recorder, rx: &Receiver<Msg>) -> io::Result<()> {
        // Bounded so a firehose on stdin can't starve the display.
        for _ in 0..50_000 {
            match rx.try_recv() {
                Ok(Msg::Line(at, line)) => {
                    if rec.ingest(at, &line)? && self.input != "live" {
                        self.input = "live".into();
                    }
                }
                Ok(Msg::Eof) => self.input = "input closed".into(),
                Ok(Msg::Error(e)) => self.input = e,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    }

    /// Handle a key press. Returns false to quit.
    fn key(&mut self, k: KeyEvent, rec: &mut Recorder) -> bool {
        let now = rec.now();
        match k.code {
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                if self.frozen.take().is_none() {
                    self.frozen = Some(now);
                } else {
                    self.pan = 0.0;
                }
            }
            KeyCode::Left | KeyCode::PageUp => {
                self.frozen.get_or_insert(now);
                let step = if k.code == KeyCode::Left { self.span / 10.0 } else { self.span };
                self.pan += step;
            }
            KeyCode::Right | KeyCode::PageDown if self.frozen.is_some() => {
                let step = if k.code == KeyCode::Right { self.span / 10.0 } else { self.span };
                self.pan = (self.pan - step).max(0.0);
            }
            KeyCode::End => {
                self.frozen = None;
                self.pan = 0.0;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.span = scale::TIME_STEPS
                    .iter()
                    .rev()
                    .copied()
                    .find(|&s| s < self.span * 0.999 && s >= MIN_SPAN)
                    .unwrap_or(self.span.min(MIN_SPAN));
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.span = scale::TIME_STEPS
                    .iter()
                    .copied()
                    .find(|&s| s > self.span * 1.001)
                    .unwrap_or(self.span * 2.0)
                    .min(MAX_SPAN);
            }
            KeyCode::Char('s') => {
                self.lanes = !self.lanes;
                self.locked = None;
            }
            KeyCode::Char('a') => {
                self.locked = match self.locked {
                    Some(_) => None,
                    None => Some(self.last_ranges.clone()),
                };
            }
            KeyCode::Char('c') => rec.clear(),
            KeyCode::Char(d @ '1'..='9') => {
                if let Some(ch) = rec.channels.get_mut(d as usize - '1' as usize) {
                    ch.visible = !ch.visible;
                    if self.lanes {
                        self.locked = None;
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn draw(&mut self, rec: &Recorder, out: &mut impl Write) -> io::Result<()> {
        let (w, h) = terminal::size()?;
        let (w, h) = (w as usize, h as usize);
        let mut s = Screen::new(w, h);
        if w < GUTTER + 20 || h < 8 {
            s.text(0, 0, "terminal too small", Color::Red);
            return s.flush(out);
        }

        let now = rec.now();
        let t_end = self.frozen.unwrap_or(now) - self.pan;
        let t_start = t_end - self.span;
        let pw = w - GUTTER;
        let plot_h = h - 3;

        self.header(&mut s, rec, now);

        let visible: Vec<usize> = (0..rec.channels.len()).filter(|&i| rec.channels[i].visible).collect();
        // Each lane needs at least two rows plus a separator.
        let lanes: Vec<Vec<usize>> = if self.lanes && visible.len() > 1 && plot_h + 1 >= visible.len() * 3 {
            visible.iter().map(|&c| vec![c]).collect()
        } else {
            vec![visible]
        };

        let tstep = scale::time_step(self.span * 12.0 / pw as f64);
        let grid_cols = time_grid(t_start, t_end, self.span, pw, tstep);
        let win = rec.window(t_start, t_end);

        let n = lanes.len();
        let avail = plot_h - (n - 1);
        let mut ranges = Vec::with_capacity(n);
        let mut top = 1;
        for (li, chans) in lanes.iter().enumerate() {
            let lane = Lane {
                top,
                w: pw,
                h: avail / n + usize::from(li < avail % n),
                range: self.range(li, rec, chans, win),
                t_start,
                span: self.span,
            };
            lane.draw(&mut s, rec, chans, win, &grid_cols, n > 1);
            ranges.push(lane.range);
            top += lane.h;
            if li + 1 < n {
                for x in 0..w {
                    s.put(x, top, '─', GRID);
                }
                top += 1;
            }
        }
        self.last_ranges = ranges;

        // Time axis: elapsed time under each vertical grid line.
        let mut free = GUTTER;
        for &(c, t) in &grid_cols {
            let label = scale::fmt_elapsed(t, tstep);
            let len = label.chars().count();
            let x = (GUTTER + c).saturating_sub(len / 2);
            if x >= free && x + len <= w {
                s.text(x, h - 2, &label, DIM);
                free = x + len + 2;
            }
        }

        self.status(&mut s, rec, h - 1);
        s.flush(out)
    }

    /// Vertical scale for a lane: locked, or fitted to what's on screen.
    fn range(&self, li: usize, rec: &Recorder, chans: &[usize], win: (usize, usize)) -> (f64, f64) {
        if let Some(r) = self.locked.as_ref().and_then(|l| l.get(li)) {
            return *r;
        }
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for smp in rec.history.range(win.0..win.1) {
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

    fn header(&self, s: &mut Screen, rec: &Recorder, now: f64) {
        let mut x = if self.frozen.is_some() {
            s.styled(0, 0, " ❚❚ PAUSED ", Color::Yellow, true)
        } else {
            let lamp = if (now * 2.0) as u64 % 2 == 0 { '●' } else { ' ' };
            s.styled(0, 0, &format!(" {lamp} REC "), Color::Red, true)
        };
        x = s.text(x, 0, &format!(" span {} ", scale::fmt_span(self.span)), DIM);
        if self.pan > 0.0 {
            x = s.text(x, 0, &format!("◂{} ", scale::fmt_span(self.pan)), Color::Yellow);
        }
        x = s.text(x, 0, "│", GRID);
        if rec.channels.is_empty() {
            s.text(x, 0, " no channels yet", DIM);
        }
        for (i, ch) in rec.channels.iter().enumerate() {
            let (mark, color) = if ch.visible { ('■', palette(i)) } else { ('□', GRID) };
            x = s.text(x, 0, &format!(" {mark} {} ", ch.name), color);
            x = s.styled(x, 0, &scale::fmt_reading(ch.last), if ch.visible { Color::White } else { GRID }, ch.visible);
            x = s.text(x, 0, " ", GRID);
        }
    }

    fn status(&self, s: &mut Screen, rec: &Recorder, y: usize) {
        let rate = match rec.rate() {
            Some(r) if r >= 10.0 => format!("{r:.0}/s"),
            Some(r) if r >= 1.0 => format!("{r:.1}/s"),
            Some(r) => format!("{r:.2}/s"),
            None => "—/s".into(),
        };
        let right = format!(" {} samples · {rate} · {} ", rec.total, self.input);
        let rlen = right.chars().count();
        let limit = s.w.saturating_sub(rlen + 1);

        let keys = [
            ("q", "quit"),
            ("space", if self.frozen.is_some() { "resume" } else { "pause" }),
            ("←→", "scroll"),
            ("+-", "span"),
            ("s", if self.lanes { "overlay" } else { "lanes" }),
            ("a", if self.locked.is_some() { "autoscale" } else { "hold scale" }),
            ("c", "clear"),
            ("1-9", "channels"),
        ];
        let mut x = 1;
        for (key, what) in keys {
            if x + key.chars().count() + what.len() + 2 > limit {
                break;
            }
            x = s.styled(x, y, key, Color::White, true);
            x = s.text(x, y, &format!(" {what}  "), DIM);
        }
        let color = if self.input == "live" { DIM } else { Color::Yellow };
        s.text(s.w.saturating_sub(rlen), y, &right, color);
    }
}

/// Columns (and times) of vertical grid lines. Lines are anchored to absolute time,
/// so they scroll along with the trace like printed divisions on chart paper.
fn time_grid(t_start: f64, t_end: f64, span: f64, pw: usize, step: f64) -> Vec<(usize, f64)> {
    let mut cols = Vec::new();
    let mut k = (t_start.max(0.0) / step).ceil() as i64;
    loop {
        let t = k as f64 * step;
        if t > t_end {
            break;
        }
        let c = ((t - t_start) / span * pw as f64).floor();
        if c >= 0.0 && (c as usize) < pw {
            cols.push((c as usize, t));
        }
        k += 1;
    }
    cols
}

struct Lane {
    top: usize,
    w: usize,
    h: usize,
    range: (f64, f64),
    t_start: f64,
    span: f64,
}

impl Lane {
    fn draw(
        &self,
        s: &mut Screen,
        rec: &Recorder,
        chans: &[usize],
        win: (usize, usize),
        grid_cols: &[(usize, f64)],
        label: bool,
    ) {
        let (lo, hi) = self.range;
        let dw = (self.w * 2) as f64;
        let dh = (self.h * 4) as f64;
        let ymap = |v: f64| ((hi - v) / (hi - lo) * (dh - 1.0)).clamp(0.0, dh - 1.0);
        let xmap = |t: f64| (t - self.t_start) / self.span * dw;

        // Include one sample either side so the trace runs off the edges.
        let a = win.0.saturating_sub(1);
        let b = (win.1 + 1).min(rec.history.len());
        let mut cv = Canvas::new(self.w, self.h);
        for &c in chans {
            let color = palette(c);
            let mut prev = None;
            for smp in rec.history.range(a..b) {
                let v = smp.get(c);
                if !v.is_finite() {
                    prev = None;
                    continue;
                }
                let p = (xmap(smp.t), ymap(v));
                match prev {
                    Some(q) => cv.line(q, p, color),
                    None => cv.dot(p, color),
                }
                prev = Some(p);
            }
        }

        let (ticks, step) = scale::ticks(lo, hi, (self.h / 2).clamp(2, 8));
        let tick_rows: Vec<(usize, f64)> = ticks.iter().map(|&t| (ymap(t).round() as usize / 4, t)).collect();
        let mut hgrid = vec![false; self.h];
        for &(r, _) in &tick_rows {
            hgrid[r] = true;
        }
        let mut vgrid = vec![false; self.w];
        for &(c, _) in grid_cols {
            vgrid[c] = true;
        }

        for r in 0..self.h {
            for c in 0..self.w {
                let i = r * self.w + c;
                let (ch, fg) = if cv.bits[i] != 0 {
                    (char::from_u32(0x2800 + cv.bits[i] as u32).unwrap_or('?'), cv.color[i])
                } else {
                    let ch = match (hgrid[r], vgrid[c]) {
                        (true, true) => '┼',
                        (true, false) => '┈',
                        (false, true) => '┊',
                        (false, false) => ' ',
                    };
                    (ch, GRID)
                };
                s.put(GUTTER + c, self.top + r, ch, fg);
            }
            s.put(GUTTER - 1, self.top + r, '│', GRID);
        }

        let mut last_row = None;
        for &(r, t) in &tick_rows {
            s.put(GUTTER - 1, self.top + r, '┤', GRID);
            if last_row != Some(r) {
                let label: String = scale::fmt_value(t, step).chars().take(GUTTER - 2).collect();
                let x = GUTTER - 2 - label.chars().count();
                s.text(x, self.top + r, &label, DIM);
                last_row = Some(r);
            }
        }

        if let (true, &[c]) = (label, chans) {
            s.styled(GUTTER, self.top, &format!(" {} ", rec.channels[c].name), palette(c), true);
        }
    }
}

/// A grid of braille cells, each holding 2×4 dots.
struct Canvas {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    color: Vec<Color>,
}

const BRAILLE: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, bits: vec![0; w * h], color: vec![Color::Reset; w * h] }
    }

    fn set(&mut self, x: i64, y: i64, color: Color) {
        if x < 0 || y < 0 || x >= (self.w * 2) as i64 || y >= (self.h * 4) as i64 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let i = (y / 4) * self.w + x / 2;
        self.bits[i] |= BRAILLE[y % 4][x % 2];
        self.color[i] = color;
    }

    fn dot(&mut self, p: (f64, f64), color: Color) {
        self.set(p.0.floor() as i64, p.1.round() as i64, color);
    }

    /// Draw a segment between two dot-space points, clipped to the canvas width.
    fn line(&mut self, a: (f64, f64), b: (f64, f64), color: Color) {
        let (mut a, mut b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        let xmax = (self.w * 2) as f64 - 1e-6;
        if b.0 < 0.0 || a.0 > xmax {
            return;
        }
        let lerp = |a: (f64, f64), b: (f64, f64), x: f64| (x, a.1 + (x - a.0) / (b.0 - a.0) * (b.1 - a.1));
        if a.0 < 0.0 {
            a = lerp(a, b, 0.0);
        }
        if b.0 > xmax {
            b = lerp(a, b, xmax);
        }

        let (mut x0, mut y0) = (a.0.floor() as i64, a.1.round() as i64);
        let (x1, y1) = (b.0.floor() as i64, b.1.round() as i64);
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = ((x1 - x0).signum(), (y1 - y0).signum());
        let mut err = dx + dy;
        loop {
            self.set(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    fg: Color,
    bold: bool,
}

/// An off-screen frame, written to the terminal in one go.
struct Screen {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
}

impl Screen {
    fn new(w: usize, h: usize) -> Self {
        Screen { w, h, cells: vec![Cell { ch: ' ', fg: Color::Reset, bold: false }; w * h] }
    }

    fn put(&mut self, x: usize, y: usize, ch: char, fg: Color) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = Cell { ch, fg, bold: false };
        }
    }

    fn text(&mut self, x: usize, y: usize, s: &str, fg: Color) -> usize {
        self.styled(x, y, s, fg, false)
    }

    /// Write a string; returns the column after it.
    fn styled(&mut self, mut x: usize, y: usize, s: &str, fg: Color, bold: bool) -> usize {
        if y >= self.h {
            return x;
        }
        for ch in s.chars() {
            if x >= self.w {
                break;
            }
            self.cells[y * self.w + x] = Cell { ch, fg, bold };
            x += 1;
        }
        x
    }

    fn flush(&self, out: &mut impl Write) -> io::Result<()> {
        queue!(out, BeginSynchronizedUpdate)?;
        let (mut fg, mut bold) = (None, false);
        let mut run = String::new();
        for y in 0..self.h {
            queue!(out, cursor::MoveTo(0, y as u16))?;
            for cell in &self.cells[y * self.w..(y + 1) * self.w] {
                if Some(cell.fg) != fg || cell.bold != bold {
                    if !run.is_empty() {
                        queue!(out, Print(&run))?;
                        run.clear();
                    }
                    if cell.bold != bold {
                        let attr = if cell.bold { Attribute::Bold } else { Attribute::NormalIntensity };
                        queue!(out, SetAttribute(attr))?;
                        bold = cell.bold;
                    }
                    if Some(cell.fg) != fg {
                        queue!(out, SetForegroundColor(cell.fg))?;
                        fg = Some(cell.fg);
                    }
                }
                run.push(cell.ch);
            }
            queue!(out, Print(&run))?;
            run.clear();
        }
        queue!(out, SetAttribute(Attribute::Reset), EndSynchronizedUpdate)?;
        out.flush()
    }
}
