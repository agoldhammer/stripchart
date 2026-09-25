//! Input sources. Each runs on its own thread and sends timestamped lines.

use std::f64::consts::TAU;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub enum Msg {
    /// A line of text and the moment it arrived.
    Line(Instant, String),
    Eof,
    Error(String),
}

/// Read lines from a stream (stdin, a file, a FIFO, a serial device).
/// Lines may end in `\n`, `\r\n` or a bare `\r`.
pub fn spawn_reader<R: Read + Send + 'static>(mut r: R, tx: SyncSender<Msg>) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut line = Vec::new();
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) => {
                    if !line.is_empty() {
                        let _ = tx.send(Msg::Line(Instant::now(), lossy(&line)));
                    }
                    let _ = tx.send(Msg::Eof);
                    return;
                }
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("read error: {e}")));
                    return;
                }
            };
            let at = Instant::now();
            for &b in &buf[..n] {
                if b == b'\n' || b == b'\r' {
                    if !line.is_empty() {
                        if tx.send(Msg::Line(at, lossy(&line))).is_err() {
                            return;
                        }
                        line.clear();
                    }
                } else {
                    line.push(b);
                }
            }
        }
    });
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Run a shell command every `interval`; its whole output is one sample.
pub fn spawn_command(cmd: String, interval: Duration, tx: SyncSender<Msg>) {
    thread::spawn(move || {
        let mut next = Instant::now();
        loop {
            let msg = match Command::new("sh").arg("-c").arg(&cmd).stdin(Stdio::null()).output() {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout).replace(['\n', '\r'], " ");
                    Msg::Line(Instant::now(), text)
                }
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let err = err.lines().next().unwrap_or("").trim();
                    Msg::Error(format!("command failed ({}) {err}", out.status))
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("cannot run command: {e}")));
                    return;
                }
            };
            if tx.send(msg).is_err() {
                return;
            }
            next += interval;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    });
}

/// Built-in signal generator: a sine, a random walk and a lagged square wave.
pub fn spawn_demo(interval: Duration, tx: SyncSender<Msg>) {
    thread::spawn(move || {
        let start = Instant::now();
        let mut rng = Rng::seeded();
        let (mut walk, mut square) = (0.0f64, 1.0f64);
        let mut next = start;
        loop {
            let t = start.elapsed().as_secs_f64();
            let sine = 5.0 * (TAU * t / 10.0).sin() + 0.8 * (TAU * t * 1.3).sin();
            walk += rng.gauss() * 0.15 - walk * 0.01;
            let target = if (t / 4.0).fract() < 0.5 { 5.0 } else { 1.0 };
            square += (target - square) * 0.25;
            let noisy = square + rng.gauss() * 0.05;
            let line = format!("sine={sine:.4} walk={walk:.4} square={noisy:.4}");
            if tx.send(Msg::Line(Instant::now(), line)).is_err() {
                return;
            }
            next += interval;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    });
}

/// xorshift64*: plenty for demo noise.
struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        Rng(nanos | 1)
    }

    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Approximately standard normal (Irwin–Hall with 12 terms).
    fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.uniform()).sum::<f64>() - 6.0
    }
}
