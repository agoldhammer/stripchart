//! stripchart — a strip chart recorder for the terminal.

mod data;
mod paper;
mod parse;
mod scale;
mod source;
mod tui;

use std::fs::File;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;

use crate::data::Recorder;

#[derive(Parser)]
#[command(
    version,
    about = "A strip chart recorder for the terminal",
    long_about = "A strip chart recorder for the terminal.\n\n\
        Reads numbers line by line and plots them against time. Each line is one \
        sample; values separated by spaces, commas or semicolons go to channels \
        1, 2, 3... Named values (temp=21.5 or rh:40) go to channels by name, and a \
        first line of words names the channels. Trailing units are ignored.",
    after_help = "Examples:\n  \
        stripchart --demo\n  \
        vmstat 1 | awk 'NR>2 {print $13, $14; fflush()}' | stripchart -n user,sys --min 0 --max 100\n  \
        stripchart -c \"cut -d' ' -f1 /proc/loadavg\" -i 2s -s 10m\n  \
        stripchart /dev/ttyUSB0 -o log.csv\n  \
        ping localhost | grep --line-buffered -o 'time=[0-9.]*' | stripchart --paper"
)]
struct Cli {
    /// File, FIFO or serial device to read (default: stdin)
    input: Option<PathBuf>,

    /// Run a shell command every --interval and plot the numbers it prints
    #[arg(short, long, value_name = "CMD", conflicts_with_all = ["input", "demo"])]
    cmd: Option<String>,

    /// Plot built-in test signals
    #[arg(long, conflicts_with = "input")]
    demo: bool,

    /// Sampling interval for --cmd (default 1s) and --demo (default 50ms)
    #[arg(short, long, value_name = "DUR", value_parser = parse::parse_duration)]
    interval: Option<Duration>,

    /// Time across the chart width, e.g. 30s, 5m, 1h
    #[arg(short, long, value_name = "DUR", default_value = "60s", value_parser = parse::parse_duration)]
    span: Duration,

    /// Fix the bottom of the scale
    #[arg(long, allow_negative_numbers = true)]
    min: Option<f64>,

    /// Fix the top of the scale
    #[arg(long, allow_negative_numbers = true)]
    max: Option<f64>,

    /// Give each channel its own lane and scale
    #[arg(short, long)]
    lanes: bool,

    /// Channel names, comma separated
    #[arg(short, long, value_delimiter = ',', value_name = "NAMES")]
    names: Vec<String>,

    /// Record every sample to a CSV file
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Paper mode: scroll down one row per sample instead of full screen
    #[arg(short, long)]
    paper: bool,

    /// Paper mode width (default: terminal width)
    #[arg(short, long)]
    width: Option<usize>,

    /// Paper mode: timestamp every N rows
    #[arg(long, default_value_t = 10, value_name = "N")]
    mark: usize,

    /// Maximum samples kept in memory for scrolling back
    #[arg(long, default_value_t = 500_000, value_name = "N")]
    history: usize,

    /// Paper mode: disable colors (also honors NO_COLOR)
    #[arg(long)]
    no_color: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stripchart: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> io::Result<()> {
    if let (Some(lo), Some(hi)) = (cli.min, cli.max) {
        if lo >= hi {
            return Err(io::Error::other("--min must be less than --max"));
        }
    }

    let (tx, rx) = mpsc::sync_channel(65536);
    if cli.demo {
        source::spawn_demo(cli.interval.unwrap_or(Duration::from_millis(50)), tx);
    } else if let Some(cmd) = cli.cmd {
        source::spawn_command(cmd, cli.interval.unwrap_or(Duration::from_secs(1)), tx);
    } else {
        match cli.input {
            Some(path) if path.as_os_str() != "-" => {
                let file = File::open(&path)
                    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
                source::spawn_reader(file, tx);
            }
            _ => {
                if io::stdin().is_terminal() {
                    return Err(io::Error::other(
                        "no input: pipe data in, name a file, or use --cmd or --demo (see --help)",
                    ));
                }
                source::spawn_reader(io::stdin(), tx);
            }
        }
    }

    let log = cli.output.map(File::create).transpose()?;
    let mut rec = Recorder::new(cli.names, cli.history, log);

    if cli.paper {
        let color = !cli.no_color && io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        let width = cli
            .width
            .or_else(|| crossterm::terminal::size().ok().map(|(w, _)| w as usize))
            .unwrap_or(80);
        let opts = paper::Options { width, lanes: cli.lanes, min: cli.min, max: cli.max, color, mark_every: cli.mark };
        paper::run(&mut rec, rx, opts)
    } else {
        if !io::stdout().is_terminal() {
            return Err(io::Error::other("the chart needs a terminal; use --paper for plain output"));
        }
        let opts = tui::Options { span: cli.span.as_secs_f64(), lanes: cli.lanes, min: cli.min, max: cli.max };
        tui::run(&mut rec, rx, opts)
    }
}
