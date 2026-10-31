//! Minimal `--name value` / `--flag` parsing with unknown options rejected.

use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct Options {
    pub positional: Vec<String>,
    values: HashMap<String, Vec<String>>,
    flags: Vec<String>,
}

impl Options {
    pub fn parse(arguments: &[String], values: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut options = Options::default();
        let mut iter = arguments.iter();
        while let Some(argument) = iter.next() {
            let Some(rest) = argument.strip_prefix("--") else {
                options.positional.push(argument.clone());
                continue;
            };
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (rest, None),
            };
            if flags.contains(&name) {
                options.flags.push(name.to_string());
            } else if values.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => iter
                        .next()
                        .cloned()
                        .ok_or_else(|| format!("--{name} requires a value"))?,
                };
                options
                    .values
                    .entry(name.to_string())
                    .or_default()
                    .push(value);
            } else {
                return Err(format!("unknown option --{name}"));
            }
        }
        Ok(options)
    }

    pub fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    pub fn all(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    pub fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.one(name) {
            Some(value) => value
                .parse()
                .map_err(|_| format!("--{name} must be a number, got {value}")),
            None => Ok(default),
        }
    }

    /// A positive duration in seconds; `default` applies when the option is absent.
    pub fn seconds(&self, name: &str, default: f64) -> Result<Duration, String> {
        let seconds: f64 = self.number(name, default)?;
        if !(seconds > 0.0 && seconds.is_finite()) {
            return Err(format!("--{name} must be positive"));
        }
        Ok(Duration::from_secs_f64(seconds))
    }

    pub fn require_positional(&self, count: usize, usage: &str) -> Result<&[String], String> {
        if self.positional.len() == count {
            Ok(&self.positional)
        } else {
            Err(format!("expected {usage}"))
        }
    }
}

/// Options shared by every command that touches the corpus or the worker.
pub const COMMON_VALUES: [&str; 3] = ["corpus", "revision", "engine"];
