//! The recorder: channel registry, sample history and CSV logging.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::style::Color;

use crate::parse::{parse_line, Parsed};

const PALETTE: [Color; 7] =
    [Color::Red, Color::Cyan, Color::Green, Color::Yellow, Color::Magenta, Color::Blue, Color::White];

/// Pen color for a channel.
pub fn palette(i: usize) -> Color {
    PALETTE[i % PALETTE.len()]
}

pub struct Channel {
    pub name: String,
    pub last: f64,
    pub visible: bool,
}

pub struct Sample {
    /// Seconds since the recorder started.
    pub t: f64,
    /// One value per channel; NaN where a channel had no reading.
    pub v: Vec<f64>,
}

impl Sample {
    pub fn get(&self, ch: usize) -> f64 {
        self.v.get(ch).copied().unwrap_or(f64::NAN)
    }
}

pub struct Recorder {
    start: Instant,
    pub channels: Vec<Channel>,
    pub history: VecDeque<Sample>,
    max_history: usize,
    /// Channels named on the command line; input headers don't rename these.
    fixed_names: usize,
    log: Option<BufWriter<File>>,
    log_header: bool,
    pub total: u64,
}

impl Recorder {
    pub fn new(names: Vec<String>, max_history: usize, log: Option<File>) -> Self {
        let fixed_names = names.len();
        let channels = names
            .into_iter()
            .map(|name| Channel { name, last: f64::NAN, visible: true })
            .collect();
        Recorder {
            start: Instant::now(),
            channels,
            history: VecDeque::new(),
            max_history: max_history.max(2),
            fixed_names,
            log: log.map(BufWriter::new),
            log_header: false,
            total: 0,
        }
    }

    /// Seconds since start.
    pub fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Parse and record one line. Returns true if a sample was added.
    pub fn ingest(&mut self, at: Instant, line: &str) -> io::Result<bool> {
        let fields = match parse_line(line) {
            Parsed::Empty => return Ok(false),
            Parsed::Header(names) => {
                // Only a header before the first sample names channels; a stray word
                // line mid-stream (an error message, say) must not rename them.
                if self.total == 0 {
                    for (i, name) in names.into_iter().enumerate().skip(self.fixed_names) {
                        self.channel(i).name = name;
                    }
                }
                return Ok(false);
            }
            Parsed::Values(fields) => fields,
        };

        let mut v = vec![f64::NAN; self.channels.len()];
        let mut pos = 0;
        for f in fields {
            let idx = match f.key {
                Some(k) => self.channel_named(k),
                None => {
                    let i = pos;
                    pos += 1;
                    self.channel(i);
                    i
                }
            };
            if idx >= v.len() {
                v.resize(idx + 1, f64::NAN);
            }
            v[idx] = f.value;
            self.channels[idx].last = f.value;
        }

        let sample = Sample { t: at.saturating_duration_since(self.start).as_secs_f64(), v };
        self.write_log(&sample)?;
        self.history.push_back(sample);
        if self.history.len() > self.max_history {
            self.history.pop_front();
        }
        self.total += 1;
        Ok(true)
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Get channel `i`, creating it (and any before it) if needed.
    fn channel(&mut self, i: usize) -> &mut Channel {
        while self.channels.len() <= i {
            let name = format!("ch{}", self.channels.len() + 1);
            self.channels.push(Channel { name, last: f64::NAN, visible: true });
        }
        &mut self.channels[i]
    }

    fn channel_named(&mut self, name: String) -> usize {
        if let Some(i) = self.channels.iter().position(|c| c.name == name) {
            return i;
        }
        self.channels.push(Channel { name, last: f64::NAN, visible: true });
        self.channels.len() - 1
    }

    /// Indices `[start, end)` of samples with `t0 <= t <= t1`.
    pub fn window(&self, t0: f64, t1: f64) -> (usize, usize) {
        let a = self.history.partition_point(|s| s.t < t0);
        let b = self.history.partition_point(|s| s.t <= t1);
        (a, b.max(a))
    }

    /// Recent sample rate in samples per second.
    pub fn rate(&self) -> Option<f64> {
        let n = self.history.len().min(200);
        if n < 2 {
            return None;
        }
        let dt = self.history.back()?.t - self.history[self.history.len() - n].t;
        (dt > 0.0).then(|| (n - 1) as f64 / dt)
    }

    fn write_log(&mut self, s: &Sample) -> io::Result<()> {
        let Some(w) = self.log.as_mut() else { return Ok(()) };
        if !self.log_header {
            write!(w, "unix_time,elapsed")?;
            for c in &self.channels {
                write!(w, ",{}", c.name)?;
            }
            writeln!(w)?;
            self.log_header = true;
        }
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        write!(w, "{unix:.3},{:.3}", s.t)?;
        for v in &s.v {
            if v.is_finite() {
                write!(w, ",{v}")?;
            } else {
                write!(w, ",")?;
            }
        }
        writeln!(w)?;
        w.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_from_header_and_keys() {
        let mut r = Recorder::new(vec![], 100, None);
        let now = Instant::now();
        assert!(!r.ingest(now, "a b").unwrap());
        assert!(r.ingest(now, "1 2").unwrap());
        assert!(r.ingest(now, "c=3 b=4").unwrap());
        assert!(!r.ingest(now, "oops").unwrap());
        let names: Vec<_> = r.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        let last = r.history.back().unwrap();
        assert!(last.get(0).is_nan());
        assert_eq!((last.get(1), last.get(2)), (4.0, 3.0));
    }

    #[test]
    fn cli_names_win() {
        let mut r = Recorder::new(vec!["x".into()], 100, None);
        r.ingest(Instant::now(), "a b").unwrap();
        let names: Vec<_> = r.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["x", "b"]);
    }

    #[test]
    fn history_is_bounded() {
        let mut r = Recorder::new(vec![], 3, None);
        for i in 0..10 {
            r.ingest(Instant::now(), &i.to_string()).unwrap();
        }
        assert_eq!(r.history.len(), 3);
        assert_eq!(r.history[0].get(0), 7.0);
        assert_eq!(r.total, 10);
    }
}
