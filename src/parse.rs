//! Parsing of input lines and command-line durations.

use std::time::Duration;

/// One numeric value found on an input line, optionally tagged with a channel name.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: Option<String>,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// Blank line, or a comment without channel names.
    Empty,
    /// A line of words only: channel names.
    Header(Vec<String>),
    Values(Vec<Field>),
}

/// Parse one line of input.
///
/// Accepted forms (separators are whitespace, `,` or `;`):
///   `1.5 2.7 -3`            positional values, one per channel
///   `temp=21.5 rh:40%`      named values
///   `time temp humidity`    header naming the channels
///   `# time temp`           comment header
/// Trailing units (`21.5C`, `40%`) are ignored. Words mixed in with numbers are skipped.
pub fn parse_line(line: &str) -> Parsed {
    let line = line.trim();
    let (line, comment) = match line.strip_prefix('#') {
        Some(rest) => (rest, true),
        None => (line, false),
    };

    let mut fields = Vec::new();
    let mut words = Vec::new();
    for tok in line
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|t| !t.is_empty())
    {
        if let Some((key, val)) = split_key(tok) {
            if let Some(value) = parse_number(val) {
                fields.push(Field { key: Some(key.to_string()), value });
            }
        } else if let Some(value) = parse_number(tok) {
            fields.push(Field { key: None, value });
        } else {
            words.push(tok.to_string());
        }
    }

    if comment {
        if fields.is_empty() && !words.is_empty() {
            Parsed::Header(words)
        } else {
            Parsed::Empty
        }
    } else if !fields.is_empty() {
        Parsed::Values(fields)
    } else if !words.is_empty() {
        Parsed::Header(words)
    } else {
        Parsed::Empty
    }
}

/// Split `key=value` or `key:value`. The key must start like an identifier so that
/// things like `12:30` are not mistaken for named values.
fn split_key(tok: &str) -> Option<(&str, &str)> {
    let (key, val) = tok.split_once('=').or_else(|| tok.split_once(':'))?;
    let first = key.chars().next()?;
    (first.is_alphabetic() || first == '_').then_some((key, val))
}

/// Parse a number, tolerating a trailing unit such as `V`, `%` or `°C`.
pub fn parse_number(s: &str) -> Option<f64> {
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    let stripped = s.trim_end_matches(|c: char| c.is_alphabetic() || c == '%' || c == '°');
    if stripped.is_empty() || stripped.len() == s.len() {
        return None;
    }
    stripped.parse().ok()
}

/// Parse a duration such as `250ms`, `2s`, `1.5m`, `1h` or a bare number of seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let num = s.trim_end_matches(|c: char| c.is_alphabetic());
    let unit = &s[num.len()..];
    let n: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration '{s}'"))?;
    let mult = match unit {
        "" | "s" | "sec" => 1.0,
        "ms" => 1e-3,
        "us" => 1e-6,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => return Err(format!("unknown unit '{unit}' in duration '{s}'")),
    };
    let secs = n * mult;
    if !(secs.is_finite() && secs > 0.0) {
        return Err(format!("duration '{s}' must be positive"));
    }
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(line: &str) -> Vec<Field> {
        match parse_line(line) {
            Parsed::Values(f) => f,
            other => panic!("expected values, got {other:?}"),
        }
    }

    fn pos(value: f64) -> Field {
        Field { key: None, value }
    }

    fn named(key: &str, value: f64) -> Field {
        Field { key: Some(key.into()), value }
    }

    #[test]
    fn positional() {
        assert_eq!(vals("1 2.5, -3;4e2"), [pos(1.0), pos(2.5), pos(-3.0), pos(400.0)]);
    }

    #[test]
    fn named_and_units() {
        assert_eq!(vals("temp=21.5C rh:40% V 3.3V"), [named("temp", 21.5), named("rh", 40.0), pos(3.3)]);
    }

    #[test]
    fn headers() {
        assert_eq!(parse_line("time temp"), Parsed::Header(vec!["time".into(), "temp".into()]));
        assert_eq!(parse_line("# a b"), Parsed::Header(vec!["a".into(), "b".into()]));
        assert_eq!(parse_line("# 1 2"), Parsed::Empty);
        assert_eq!(parse_line("   "), Parsed::Empty);
    }

    #[test]
    fn clock_is_not_a_key() {
        assert_eq!(vals("12:30 5"), [pos(5.0)]);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("2").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("1.5m").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("1e-3s").unwrap(), Duration::from_millis(1));
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("3 parsecs").is_err());
    }
}
