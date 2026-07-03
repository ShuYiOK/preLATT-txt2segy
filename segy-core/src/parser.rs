use chrono::{DateTime, NaiveDateTime, Utc};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::SegyError;

#[derive(Debug, Clone)]
pub struct MseismFile {
    pub device_id: String,
    pub start_time: DateTime<Utc>,
    pub status_code: u32,
    pub x: f64,
    pub y: f64,
    pub elevation: f64,
    pub samples: Vec<f32>,
    pub sample_count: usize,
}

impl MseismFile {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, SegyError> {
        let path_ref = path.as_ref();
        let mut file = File::open(path_ref)
            .map_err(|e| SegyError::Parse(format!("Failed to open file {}: {}", path_ref.display(), e)))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| SegyError::Parse(format!("Failed to read file {}: {}", path_ref.display(), e)))?;
        Self::parse(&content, path_ref)
    }

    fn parse(content: &str, path: &Path) -> Result<Self, SegyError> {
        let device_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('-').last())
            .unwrap_or("")
            .to_string();

        let mut lines = content.lines().peekable();

        let mut start_time: Option<DateTime<Utc>> = None;
        let mut status_code: u32 = 0;
        let mut x: f64 = 0.0;
        let mut y: f64 = 0.0;
        let mut elevation: f64 = 0.0;
        let mut samples = Vec::with_capacity(300000);
        let mut first_header = true;

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.contains(',') {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() >= 5 {
                    let dt = NaiveDateTime::parse_from_str(fields[0], "%Y-%m-%d %H:%M:%S")
                        .map_err(|e| SegyError::Parse(format!("Failed to parse time '{}': {}", fields[0], e)))?
                        .and_utc();

                    if first_header {
                        start_time = Some(dt);
                        status_code = fields[1].trim().parse().unwrap_or(0);
                        x = fields[2].trim().parse().unwrap_or(0.0);
                        y = fields[3].trim().parse().unwrap_or(0.0);
                        elevation = fields[4].trim().parse().unwrap_or(0.0);
                        first_header = false;
                    }
                    continue;
                }
            }

            parse_samples_line(line, &mut samples)?;
        }

        let start_time = start_time.ok_or_else(|| {
            SegyError::Parse("No valid header line found in file".into())
        })?;

        let sample_count = samples.len();

        Ok(Self {
            device_id,
            start_time,
            status_code,
            x,
            y,
            elevation,
            samples,
            sample_count,
        })
    }
}

fn parse_samples_line(line: &str, out: &mut Vec<f32>) -> Result<(), SegyError> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= len {
            break;
        }

        let start = i;
        while i < len && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }

        let num_str = &line[start..i];
        let val: f32 = num_str
            .parse()
            .map_err(|e| SegyError::Parse(format!("Failed to parse float '{}': {}", num_str, e)))?;
        out.push(val);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_samples_line() {
        let mut out = Vec::new();
        parse_samples_line("1.0 2.5 -3.14", &mut out).unwrap();
        assert_eq!(out, vec![1.0, 2.5, -3.14]);
    }

    #[test]
    fn test_parse_with_leading_trailing_spaces() {
        let mut out = Vec::new();
        parse_samples_line("  0.5  -1.0  2.0  ", &mut out).unwrap();
        assert_eq!(out, vec![0.5, -1.0, 2.0]);
    }

    #[test]
    fn test_parse_header_line() {
        let header = "2026-01-25 00:00:00,713,10451.92567,2930.01390,369.9";
        let fields: Vec<&str> = header.split(',').collect();
        assert_eq!(fields.len(), 5);
        assert_eq!(fields[0], "2026-01-25 00:00:00");
        assert_eq!(fields[1], "713");
        assert_eq!(fields[2], "10451.92567");
        assert_eq!(fields[3], "2930.01390");
        assert_eq!(fields[4], "369.9");
    }
}
