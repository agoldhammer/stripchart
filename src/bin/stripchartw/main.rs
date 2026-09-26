//! stripchartw — the strip chart recorder in a window instead of the terminal.

mod app;

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;
use stripchart::data::Recorder;
use stripchart::{parse, source};

#[derive(Parser)]
#[command(
    version,
    about = "A strip chart recorder in a window",
    long_about = "A strip chart recorder in a window.\n\n\
        Reads numbers line by line and plots them against time. Each line is one \
        sample; values separated by spaces, commas or semicolons go to channels \
        1, 2, 3... Named values (temp=21.5 or rh:40) go to channels by name, and a \
        first line of words names the channels. Trailing units are ignored.",
    after_help = "Examples:\n  \
        stripchartw --demo\n  \
        vmstat 1 | awk 'NR>2 {print $13, $14; fflush()}' | stripchartw -n user,sys --min 0 --max 100\n  \
        stripchartw -c \"cut -d' ' -f1 /proc/loadavg\" -i 2s -s 10m\n  \
        stripchartw /dev/ttyUSB0 -o log.csv"
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

    /// Maximum samples kept in memory for scrolling back
    #[arg(long, default_value_t = 500_000, value_name = "N")]
    history: usize,

    /// Initial window size in pixels
    #[arg(long, value_name = "WxH", default_value = "1100x640", value_parser = parse_size)]
    size: (f32, f32),
}

fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WIDTHxHEIGHT, e.g. 1100x640")?;
    let px = |v: &str| v.trim().parse::<f32>().ok().filter(|&n| n >= 200.0).ok_or("width and height must be at least 200");
    Ok((px(w)?, px(h)?))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stripchartw: {e}");
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
    source::start(cli.input, cli.cmd, cli.demo, cli.interval, tx)?;

    let log = cli.output.map(File::create).transpose()?;
    let rec = Recorder::new(cli.names, cli.history, log);
    let opts = app::Options { span: cli.span.as_secs_f64(), lanes: cli.lanes, min: cli.min, max: cli.max };

    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("stripchart")
            .with_app_id("stripchartw")
            .with_inner_size(cli.size)
            .with_min_inner_size([360.0, 240.0]),
        ..Default::default()
    };
    eframe::run_native(
        "stripchartw",
        native,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, rec, rx, opts)))),
    )
    .map_err(|e| io::Error::other(e.to_string()))
}
