//! "Paper" mode: the chart scrolls down the terminal one row per sample, like
//! a pen recorder with the paper running vertically. Output is plain text, so it
//! stays in the scrollback and can be redirected to a file.

use std::io::{self, Write};
use std::sync::mpsc::Receiver;

use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};

use crate::data::{palette, Recorder, Sample};
use crate::scale;
use crate::source::Msg;

/// Width of the timestamp column.
const GUTTER: usize = 11;
const GRID: Color = Color::DarkGrey;
const DIM: Color = Color::Grey;

pub struct Options {
    pub width: usize,
    pub lanes: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub color: bool,
    /// Print a timestamp and division line every this many rows.
    pub mark_every: usize,
}

pub fn run(rec: &mut Recorder, rx: Receiver<Msg>, opts: Options) -> io::Result<()> {
    let mut out = io::stdout().lock();
    let mut paper = Paper { opts, ranges: Vec::new(), prev: Vec::new(), channels: 0, rows: 0 };
    for msg in rx {
        match msg {
            Msg::Line(at, line) => {
                if rec.ingest(at, &line)? {
                    paper.feed(&mut out, rec)?;
                }
            }
            Msg::Eof => break,
            Msg::Error(e) => eprintln!("stripchart: {e}"),
        }
    }
    out.flush()
}

type Row = Vec<(char, Color)>;

struct Paper {
    opts: Options,
    /// Scale per lane; `None` until the lane has seen a value. Ranges only grow.
    ranges: Vec<Option<(f64, f64)>>,
    /// Previous value per channel, for drawing the pen's travel.
    prev: Vec<f64>,
    channels: usize,
    rows: usize,
}

impl Paper {
    fn lanes(&self, channels: usize) -> Vec<Vec<usize>> {
        if self.opts.lanes {
            (0..channels).map(|c| vec![c]).collect()
        } else {
            vec![(0..channels).collect()]
        }
    }

    fn lane_width(&self, n: usize) -> usize {
        (self.opts.width.saturating_sub(GUTTER + n - 1) / n).max(5)
    }

    fn feed(&mut self, out: &mut impl Write, rec: &Recorder) -> io::Result<()> {
        let Some(s) = rec.history.back() else { return Ok(()) };
        let lanes = self.lanes(rec.channels.len());
        let mut rescale = rec.channels.len() != self.channels;
        self.channels = rec.channels.len();
        self.prev.resize(self.channels, f64::NAN);
        self.ranges.resize(lanes.len(), None);

        for (li, chans) in lanes.iter().enumerate() {
            for &c in chans {
                rescale |= self.expand(li, s.get(c));
            }
        }
        let lw = self.lane_width(lanes.len());
        if rescale {
            self.print_scale(out, rec, &lanes, lw)?;
        }
        let row = self.row(s, &lanes, lw);
        self.rows += 1;
        emit(out, &row, self.opts.color)
    }

    /// Grow lane `li`'s range to include `v`. Returns true if it changed.
    fn expand(&mut self, li: usize, v: f64) -> bool {
        if !v.is_finite() {
            return false;
        }
        let (lo, hi) = match self.ranges[li] {
            Some((lo, hi)) if (lo..=hi).contains(&v) => return false,
            Some((lo, hi)) => (lo.min(v), hi.max(v)),
            None => (v, v),
        };
        // Generous headroom: paper can't be redrawn, so rescale rarely.
        let (mut lo, mut hi) = scale::padded_range(lo, hi, 0.25);
        if let Some(m) = self.opts.min {
            lo = m;
        }
        if let Some(m) = self.opts.max {
            hi = m;
        }
        if !(hi > lo) {
            hi = lo + 1.0;
        }
        let changed = self.ranges[li] != Some((lo, hi));
        self.ranges[li] = Some((lo, hi));
        changed
    }

    fn row(&mut self, s: &Sample, lanes: &[Vec<usize>], lw: usize) -> Row {
        let mut row: Row = vec![(' ', Color::Reset); GUTTER + lanes.len() * (lw + 1) - 1];
        let mark = self.rows % self.opts.mark_every.max(1) == 0;
        if mark {
            let label = scale::fmt_elapsed(s.t, 0.1);
            put_str(&mut row, (GUTTER - 1).saturating_sub(label.len()), &label, DIM);
        }

        let grid = grid_cols(lw);
        for (li, chans) in lanes.iter().enumerate() {
            let x0 = GUTTER + li * (lw + 1);
            for c in 0..lw {
                let edge = c == 0 || c == lw - 1;
                let ch = match (mark, edge, grid.contains(&c)) {
                    (false, true, _) => '│',
                    (false, false, true) => '┊',
                    (false, false, false) => ' ',
                    (true, true, _) if c == 0 => '├',
                    (true, true, _) => '┤',
                    (true, false, true) => '┼',
                    (true, false, false) => '┈',
                };
                row[x0 + c] = (ch, GRID);
            }

            let Some((lo, hi)) = self.ranges[li] else { continue };
            let col = |v: f64| x0 + ((v - lo) / (hi - lo) * (lw - 1) as f64).round().clamp(0.0, (lw - 1) as f64) as usize;
            for &c in chans {
                let v = s.get(c);
                let color = palette(c);
                if !v.is_finite() {
                    self.prev[c] = f64::NAN;
                    continue;
                }
                let cur = col(v);
                let prev = self.prev[c];
                self.prev[c] = v;
                if !prev.is_finite() {
                    row[cur] = ('╷', color); // pen touches down
                    continue;
                }
                // The pen comes down from the previous row at `from`, then slides to `cur`.
                let from = col(prev);
                let (l, r) = (from.min(cur), from.max(cur));
                for cell in &mut row[l + 1..r.max(l + 1)] {
                    *cell = ('─', color);
                }
                if from < cur {
                    row[from] = ('╰', color);
                    row[cur] = ('╮', color);
                } else if from > cur {
                    row[from] = ('╯', color);
                    row[cur] = ('╭', color);
                } else {
                    row[cur] = ('│', color);
                }
            }
        }
        row
    }

    /// Print the legend and a ruler with scale labels (again whenever a scale changes).
    fn print_scale(&self, out: &mut impl Write, rec: &Recorder, lanes: &[Vec<usize>], lw: usize) -> io::Result<()> {
        let width = GUTTER + lanes.len() * (lw + 1) - 1;
        let mut legend: Row = vec![(' ', Color::Reset); width];
        let mut labels = legend.clone();
        let mut ruler = legend.clone();
        let grid = grid_cols(lw);

        let mut x = GUTTER;
        for (li, chans) in lanes.iter().enumerate() {
            let x0 = GUTTER + li * (lw + 1);
            x = x.max(x0);
            for &c in chans {
                x = put_str(&mut legend, x, &format!("■ {}  ", rec.channels[c].name), palette(c));
            }

            for c in 0..lw {
                let ch = match c {
                    0 => '┌',
                    c if c == lw - 1 => '┐',
                    c if grid.contains(&c) => '┬',
                    _ => '─',
                };
                ruler[x0 + c] = (ch, GRID);
            }

            if let Some((lo, hi)) = self.ranges[li] {
                let step = (hi - lo) / 4.0;
                let lo_s = scale::fmt_value(lo, step);
                let mid_s = scale::fmt_value((lo + hi) / 2.0, step);
                let hi_s = scale::fmt_value(hi, step);
                let lo_end = put_str(&mut labels, x0, &lo_s, DIM);
                let hi_x = (x0 + lw).saturating_sub(hi_s.chars().count());
                let mid_x = (x0 + grid[2]).saturating_sub(mid_s.chars().count() / 2);
                if mid_x > lo_end && mid_x + mid_s.chars().count() < hi_x {
                    put_str(&mut labels, mid_x, &mid_s, DIM);
                }
                if hi_x > lo_end {
                    put_str(&mut labels, hi_x, &hi_s, DIM);
                }
            }
        }
        put_str(&mut labels, 0, "   elapsed", DIM);
        emit(out, &legend, self.opts.color)?;
        emit(out, &labels, self.opts.color)?;
        emit(out, &ruler, self.opts.color)
    }
}

/// Columns of the quarter-scale divisions within a lane.
fn grid_cols(lw: usize) -> [usize; 5] {
    std::array::from_fn(|k| (k as f64 * (lw - 1) as f64 / 4.0).round() as usize)
}

/// Write `s` into `row` at `x`, clipped; returns the column after it.
fn put_str(row: &mut Row, mut x: usize, s: &str, color: Color) -> usize {
    for ch in s.chars() {
        if x >= row.len() {
            break;
        }
        row[x] = (ch, color);
        x += 1;
    }
    x
}

fn emit(out: &mut impl Write, row: &[(char, Color)], color: bool) -> io::Result<()> {
    let end = row.iter().rposition(|&(ch, _)| ch != ' ').map_or(0, |i| i + 1);
    let mut cur = None;
    let mut run = String::new();
    for &(ch, c) in &row[..end] {
        if color && Some(c) != cur {
            if !run.is_empty() {
                queue!(out, Print(&run))?;
                run.clear();
            }
            queue!(out, SetForegroundColor(c))?;
            cur = Some(c);
        }
        run.push(ch);
    }
    queue!(out, Print(&run))?;
    if color {
        queue!(out, ResetColor)?;
    }
    queue!(out, Print('\n'))?;
    out.flush()
}
