use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::gps::GpsStation;
use crate::parser::MseismFile;
use crate::SegyError;

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub device_id: String,
    pub date_folder: String,
    pub time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TimeAlignedDataset {
    pub devices: Vec<String>,
    pub gps: HashMap<String, GpsStation>,
    pub time_slices: BTreeMap<DateTime<Utc>, HashMap<String, PathBuf>>,
    pub sample_rate_hz: f64,
    pub samples_per_minute: usize,
}

pub struct DatasetBuilder {
    root_dir: PathBuf,
    gps: HashMap<String, GpsStation>,
    sample_rate_hz: f64,
    auto_detect_sample_rate: bool,
    selected_devices: Option<HashSet<String>>,
    date_range: Option<(NaiveDate, NaiveDate)>,
}

impl DatasetBuilder {
    pub fn new<P: AsRef<Path>>(root_dir: P) -> Self {
        Self {
            root_dir: root_dir.as_ref().to_path_buf(),
            gps: HashMap::new(),
            sample_rate_hz: 5000.0,
            auto_detect_sample_rate: true,
            selected_devices: None,
            date_range: None,
        }
    }

    pub fn with_gps(mut self, gps: HashMap<String, GpsStation>) -> Self {
        self.gps = gps;
        self
    }

    pub fn with_sample_rate(mut self, hz: f64) -> Self {
        self.sample_rate_hz = hz;
        self.auto_detect_sample_rate = false;
        self
    }

    pub fn with_devices(mut self, devices: HashSet<String>) -> Self {
        self.selected_devices = Some(devices);
        self
    }

    pub fn with_date_range(mut self, start: NaiveDate, end: NaiveDate) -> Self {
        self.date_range = Some((start, end));
        self
    }

    pub fn build(self) -> Result<TimeAlignedDataset, SegyError> {
        let mut files: Vec<FileInfo> = Vec::new();
        let mut device_set = HashSet::new();

        for entry in WalkDir::new(&self.root_dir)
            .min_depth(3)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "txt" {
                continue;
            }

            let file_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

            if file_name.contains("GPS") {
                continue;
            }

            let device_dir = match path.parent() {
                Some(p) => p,
                None => continue,
            };
            let device_id = device_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            if device_id.is_empty() {
                continue;
            }

            let date_folder = device_dir
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            if let Some(ref selected) = self.selected_devices {
                if !selected.contains(&device_id) {
                    continue;
                }
            }

            if let Some((start_d, end_d)) = self.date_range {
                if let Ok(d) = NaiveDate::parse_from_str(&date_folder, "%Y-%m-%d") {
                    if d < start_d || d > end_d {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            let time_str = match parse_time_from_filename(file_name) {
                Some(t) => t,
                None => continue,
            };

            device_set.insert(device_id.clone());
            files.push(FileInfo {
                path: path.to_path_buf(),
                device_id,
                date_folder,
                time: time_str,
            });
        }

        if files.is_empty() {
            return Err(SegyError::InvalidFormat(
                "No valid microseismic data files found".into(),
            ));
        }

        let mut devices: Vec<String> = device_set.into_iter().collect();
        devices.sort();

        let mut time_slices: BTreeMap<DateTime<Utc>, HashMap<String, PathBuf>> = BTreeMap::new();

        for f in &files {
            time_slices
                .entry(f.time)
                .or_insert_with(HashMap::new)
                .insert(f.device_id.clone(), f.path.clone());
        }

        let samples_per_minute = if self.auto_detect_sample_rate {
            let first_file = &files[0];
            let data = MseismFile::from_file(&first_file.path)
                .map_err(|e| SegyError::Parse(format!(
                    "Failed to parse first file {}: {}",
                    first_file.path.display(),
                    e
                )))?;
            let _actual_rate = data.sample_count as f64 / 60.0;
            data.sample_count
        } else {
            (self.sample_rate_hz * 60.0) as usize
        };

        let sample_rate = samples_per_minute as f64 / 60.0;

        Ok(TimeAlignedDataset {
            devices,
            gps: self.gps,
            time_slices,
            sample_rate_hz: sample_rate,
            samples_per_minute,
        })
    }
}

/// 扫描根目录下所有日期文件夹名（YYYY-MM-DD 格式），按日期排序。
pub fn scan_date_folders<P: AsRef<Path>>(root: P) -> Vec<String> {
    let mut folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.as_ref()) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if NaiveDate::parse_from_str(name, "%Y-%m-%d").is_ok() {
                        folders.push(name.to_string());
                    }
                }
            }
        }
    }
    folders.sort();
    folders
}

/// 扫描根目录下所有设备文件夹名（跨所有日期文件夹去重），按字母序返回。
pub fn scan_device_folders<P: AsRef<Path>>(root: P) -> Vec<String> {
    let mut set = HashSet::new();
    if let Ok(date_entries) = std::fs::read_dir(root.as_ref()) {
        for date_entry in date_entries.flatten() {
            let date_path = date_entry.path();
            if !date_path.is_dir() {
                continue;
            }
            if let Ok(dev_entries) = std::fs::read_dir(&date_path) {
                for dev_entry in dev_entries.flatten() {
                    if dev_entry.path().is_dir() {
                        if let Some(name) = dev_entry.file_name().to_str() {
                            set.insert(name.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut folders: Vec<String> = set.into_iter().collect();
    folders.sort();
    folders
}

fn parse_time_from_filename(filename: &str) -> Option<DateTime<Utc>> {
    let dash_idx = filename.rfind('-')?;
    let time_part = &filename[..dash_idx];
    let naive = chrono::NaiveDateTime::parse_from_str(time_part, "%Y-%m-%d_%H_%M").ok()?;
    Some(naive.and_utc())
}

impl TimeAlignedDataset {
    pub fn num_devices(&self) -> usize {
        self.devices.len()
    }

    pub fn num_time_slices(&self) -> usize {
        self.time_slices.len()
    }

    pub fn time_range(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let first = self.time_slices.keys().next()?;
        let last = self.time_slices.keys().next_back()?;
        Some((*first, *last))
    }

    pub fn sample_interval_us(&self) -> u32 {
        (1_000_000.0 / self.sample_rate_hz) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_from_filename() {
        let t = parse_time_from_filename("2026-01-25_00_00-24002").unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-01-25 00:00");
    }
}
