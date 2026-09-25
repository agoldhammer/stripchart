# stripchart

A strip chart recorder for the terminal. It reads numbers as they arrive and plots them against time, the way a pen recorder draws on a moving roll of paper.

It has two display modes:

- **Chart** (default): full screen, scrolling right to left, drawn in braille dots (2×4 dots per character cell). You can pause it, scroll back, zoom the time span, split channels into lanes, and hold the scale.
- **Paper** (`--paper`): the paper runs down the terminal, one row per sample, and each pen's movement is drawn with box-drawing characters. The output is plain text, so it stays in the scrollback and can be redirected to a file.

```
cargo build --release
./target/release/stripchart --demo
```

## Input

Each line of input is one sample. The timestamp is the time the line arrives.

| Line                      | Meaning                                              |
|---------------------------|------------------------------------------------------|
| `1.5 2.7 -3`              | values for channels 1, 2, 3 (separated by spaces, `,` or `;`) |
| `temp=21.5 rh:40`         | values for channels by name                          |
| `time temp humidity`      | a header: names the channels (only before the first data line) |
| `# temp humidity`         | a header given as a comment                          |
| `21.5C, 40%`              | trailing units are ignored                           |

Words mixed in with numbers are skipped. Lines can end in `\n`, `\r\n` or `\r`, so raw serial output works.

Input sources:

- **stdin**: `some-command | stripchart`
- **a file, FIFO or device**: `stripchart /dev/ttyUSB0`. Set the baud rate first with `stty`.
- **a command run at a fixed interval**: `stripchart -c 'cat /sys/class/thermal/thermal_zone0/temp' -i 2s`
- **test signals**: `stripchart --demo`

## Examples

```sh
# CPU user/system % from vmstat, on a fixed 0–100 scale
vmstat 1 | awk 'NR>2 {print $13, $14; fflush()}' | stripchart -n user,sys --min 0 --max 100

# Load average over the last 10 minutes
stripchart -c "cut -d' ' -f1 /proc/loadavg" -i 2s -s 10m

# Ping round-trip time on paper, also saved to CSV
ping example.com | grep --line-buffered -o 'time=[0-9.]*' | stripchart --paper -o ping.csv

# One lane per channel
stripchart --demo --lanes
```

## Options

| Option | |
|---|---|
| `-c, --cmd CMD` | run a shell command every `--interval` and plot its output |
| `--demo` | built-in signals: a sine, a random walk, a lagged square wave |
| `-i, --interval DUR` | sampling interval for `--cmd` (default `1s`) and `--demo` (default `50ms`) |
| `-s, --span DUR` | time across the chart width (default `60s`) |
| `--min N`, `--max N` | fix either end of the scale (otherwise it autoscales) |
| `-l, --lanes` | give each channel its own lane and scale |
| `-n, --names A,B,…` | channel names (these take precedence over a header line) |
| `-o, --output FILE` | save every sample to CSV (`unix_time,elapsed,<channels…>`) |
| `-p, --paper` | paper mode |
| `-w, --width N` | paper width (default: terminal width) |
| `--mark N` | paper mode: timestamp every N rows (default 10) |
| `--history N` | samples kept in memory for scrolling back (default 500000) |
| `--no-color` | paper mode: no colors (`NO_COLOR` is also honored) |

Durations accept `ms`, `s`, `m`, `h`, `d`, or a plain number of seconds.

## Keys (chart mode)

| Key | |
|---|---|
| `space` / `p` | pause or resume. Recording continues while paused. |
| `←` `→`, `PgUp` `PgDn` | scroll back or forward (`←` pauses first). `End` returns to live. |
| `+` / `-` | shorten or lengthen the time span (1-2-5 steps) |
| `s` | switch between lanes and overlay |
| `a` | hold the current scale, or go back to autoscaling |
| `1`–`9` | show or hide a channel |
| `c` | clear the history |
| `q` / `Esc` / `Ctrl-C` | quit |

## Behavior

- Vertical grid lines are placed at fixed times since the start, so they scroll with the trace like the divisions printed on chart paper. The time axis shows elapsed time.
- A value outside a fixed `--min`/`--max` scale is drawn at the edge of the chart.
- In paper mode the scale can only grow, because rows that are already printed can't be redrawn. When the scale grows, the legend and ruler are printed again. Use `--min`/`--max` to keep the scale fixed.
- The CSV header is written when the first sample arrives. If a new named channel appears later, its column is appended to later rows but not to the header.
