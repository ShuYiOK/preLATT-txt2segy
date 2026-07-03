//! 实时监视模块
//!
//! 轮询扫描根目录下的日期文件夹，自动发现新文件并合成 SEG-Y。
//! 跨日期文件夹无缝衔接：0 点后新日期文件夹内的文件会被自动纳入。

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use crate::gps::GpsStation;
use crate::header::{BinaryHeader, SegyRevision, TextHeader};
use crate::parser::MseismFile;
use crate::synth::{format_file_name, read_slice_traces};
use crate::writer::SegyWriter;
use crate::SegyError;

/// 输出文件粒度
#[derive(Debug, Clone)]
pub enum OutputMode {
    /// 每分钟一个文件
    PerMinute,
    /// 按指定分钟数聚合
    Duration(u32),
}

/// 监视引擎配置
pub struct MonitorOptions {
    pub root_dir: PathBuf,
    pub output_dir: PathBuf,
    pub file_prefix: String,
    pub file_template: String,
    pub revision: SegyRevision,
    pub use_gps_coords: bool,
    pub gps: HashMap<String, GpsStation>,
    pub selected_devices: Option<HashSet<String>>,
    pub output_mode: OutputMode,
    /// 等待窗口（秒），时间片结束后等多久再写入
    pub wait_window_secs: u64,
    /// 忽略迟到数据（true=忽略，false=单独保存到 late/ 目录）
    pub ignore_late: bool,
    /// 轮询间隔（秒）
    pub poll_interval_secs: u64,
    /// 回填模式：true=处理启动时已有文件，false=只处理启动后的新文件
    pub backfill: bool,
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            root_dir: PathBuf::new(),
            output_dir: PathBuf::from("./output"),
            file_prefix: "microseismic".into(),
            file_template: "{prefix}_{YYYY}{MM}{DD}_{hh}{mm}".into(),
            revision: SegyRevision::Rev2,
            use_gps_coords: true,
            gps: HashMap::new(),
            selected_devices: None,
            output_mode: OutputMode::PerMinute,
            wait_window_secs: 60,
            ignore_late: false,
            poll_interval_secs: 10,
            backfill: true,
            cancel_flag: None,
        }
    }
}

/// 监视事件（通过 channel 发送给 UI）
#[derive(Debug, Clone)]
pub enum MonitorEvent {
    Log(String),
    Status {
        processed: u64,
        generated: u64,
        buffered: u64,
    },
    FileGenerated(PathBuf),
    LateDataWarning {
        file: String,
        time: String,
    },
    LateDataSaved(String),
    Done(Result<(), String>),
}

struct ScannedFile {
    path: PathBuf,
    device_id: String,
    time: DateTime<Utc>,
}

/// 实时监视引擎
pub struct MonitorEngine {
    options: MonitorOptions,
}

impl MonitorEngine {
    pub fn new(options: MonitorOptions) -> Self {
        Self { options }
    }

    fn is_cancelled(&self) -> bool {
        self.options
            .cancel_flag
            .as_ref()
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    fn log(&self, sender: &Sender<MonitorEvent>, msg: impl Into<String>) {
        let _ = sender.send(MonitorEvent::Log(msg.into()));
    }

    /// 扫描根目录下所有日期/设备文件夹中的 txt 文件
    /// 跨日期文件夹扫描，不遗漏任何一天的数据
    fn scan_all_files(&self) -> Vec<ScannedFile> {
        let mut files = Vec::new();

        let date_entries = match std::fs::read_dir(&self.options.root_dir) {
            Ok(e) => e,
            Err(_) => return files,
        };

        for date_entry in date_entries.flatten() {
            let date_path = date_entry.path();
            if !date_path.is_dir() {
                continue;
            }

            // 遍历设备文件夹
            let dev_entries = match std::fs::read_dir(&date_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for dev_entry in dev_entries.flatten() {
                let dev_path = dev_entry.path();
                if !dev_path.is_dir() {
                    continue;
                }

                let device_id = dev_entry
                    .file_name()
                    .to_str()
                    .unwrap_or("")
                    .to_string();

                if device_id.is_empty() {
                    continue;
                }

                // 设备过滤
                if let Some(ref selected) = self.options.selected_devices {
                    if !selected.contains(&device_id) {
                        continue;
                    }
                }

                let txt_entries = match std::fs::read_dir(&dev_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                for txt_entry in txt_entries.flatten() {
                    let txt_path = txt_entry.path();
                    if !txt_path.is_file() {
                        continue;
                    }

                    let ext = txt_path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext != "txt" {
                        continue;
                    }

                    let file_name = txt_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");

                    if file_name.contains("GPS") {
                        continue;
                    }

                    let time = match parse_time_from_filename(file_name) {
                        Some(t) => t,
                        None => continue,
                    };

                    files.push(ScannedFile {
                        path: txt_path,
                        device_id: device_id.clone(),
                        time,
                    });
                }
            }
        }

        files
    }

    /// 写入单个时间片的 SEG-Y 文件
    fn write_single_slice(
        &self,
        output_path: &Path,
        time: DateTime<Utc>,
        slice: &HashMap<String, PathBuf>,
        devices: &[String],
        samples_per_trace: usize,
        sample_rate_hz: f64,
    ) -> Result<(), SegyError> {
        let mut text_header = TextHeader::default();
        text_header.use_utf8 = true;
        text_header.set_line(1, "SEG-Y Rev 2.0 - Microseismic Monitoring Data");
        text_header.set_line(2, &format!("Stations: {}", devices.len()));
        text_header.set_line(3, &format!("Sample rate: {:.2} Hz", sample_rate_hz));
        text_header.set_line(4, &format!("Time: {}", time.format("%Y-%m-%d %H:%M:%S UTC")));

        let sample_interval_us = (1_000_000.0 / sample_rate_hz) as u16;

        let mut binary_header = BinaryHeader::default().with_revision(self.options.revision);
        binary_header.samples_per_trace =
            if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.samples_per_trace_original =
            if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.sample_interval = sample_interval_us;
        binary_header.sample_interval_original = sample_interval_us;
        binary_header.traces_per_ensemble = devices.len() as u16;

        let mut writer = SegyWriter::create(output_path)?;
        writer.write_header(&text_header, &binary_header)?;

        let traces = read_slice_traces(
            slice,
            devices,
            &self.options.gps,
            self.options.use_gps_coords,
            samples_per_trace,
        )?;

        for (idx, (mut thdr, samples)) in traces.into_iter().enumerate() {
            thdr.trace_sequence_file = (idx + 1) as i32;
            thdr.trace_sequence_line = (idx + 1) as i32;
            thdr.ensemble_number = 1;
            thdr.trace_number_within_ensemble = (idx + 1) as i32;
            writer.write_trace(&thdr, &samples)?;
        }

        writer.finish()?;
        Ok(())
    }

    /// 写入多时间片大文件
    fn write_multi_slice(
        &self,
        output_path: &Path,
        start_time: DateTime<Utc>,
        slices: &BTreeMap<DateTime<Utc>, HashMap<String, PathBuf>>,
        devices: &[String],
        samples_per_trace: usize,
        sample_rate_hz: f64,
    ) -> Result<(), SegyError> {
        let num_slices = slices.len();
        let total_traces = num_slices * devices.len();

        let mut text_header = TextHeader::default();
        text_header.use_utf8 = true;
        text_header.set_line(1, "SEG-Y Rev 2.0 - Microseismic Monitoring Data");
        text_header.set_line(2, &format!("Stations: {}", devices.len()));
        text_header.set_line(3, &format!("Sample rate: {:.2} Hz", sample_rate_hz));
        if let Some(last) = slices.keys().last() {
            text_header.set_line(
                4,
                &format!("Start: {}", start_time.format("%Y-%m-%d %H:%M:%S UTC")),
            );
            text_header.set_line(
                5,
                &format!("End:   {}", last.format("%Y-%m-%d %H:%M:%S UTC")),
            );
        }
        text_header.set_line(6, &format!("Time slices: {}, Total traces: {}", num_slices, total_traces));

        let sample_interval_us = (1_000_000.0 / sample_rate_hz) as u16;

        let mut binary_header = BinaryHeader::default().with_revision(self.options.revision);
        binary_header.samples_per_trace =
            if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.samples_per_trace_original =
            if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.sample_interval = sample_interval_us;
        binary_header.sample_interval_original = sample_interval_us;
        binary_header.traces_per_ensemble = devices.len() as u16;

        let mut writer = SegyWriter::create(output_path)?;
        writer.write_header(&text_header, &binary_header)?;

        let mut trace_seq: i32 = 0;

        for (slice_idx, (_time, slice_files)) in slices.iter().enumerate() {
            if self.is_cancelled() {
                return Err(SegyError::Cancelled);
            }

            let traces = read_slice_traces(
                slice_files,
                devices,
                &self.options.gps,
                self.options.use_gps_coords,
                samples_per_trace,
            )?;

            for (mut thdr, samples) in traces {
                trace_seq += 1;
                thdr.trace_sequence_file = trace_seq;
                thdr.trace_sequence_line = trace_seq;
                thdr.ensemble_number = (slice_idx + 1) as i32;
                writer.write_trace(&thdr, &samples)?;
            }
        }

        writer.finish()?;
        Ok(())
    }

    /// 保存迟到数据到 late/ 目录
    fn save_late_data(
        &self,
        file: &ScannedFile,
        samples_per_trace: usize,
        sample_rate_hz: f64,
    ) -> Result<PathBuf, SegyError> {
        let late_dir = self.options.output_dir.join("late");
        std::fs::create_dir_all(&late_dir)?;

        let time_str = file.time.format("%Y%m%d_%H%M").to_string();
        let file_name = format!(
            "{}_LATE_{}_{}.segy",
            self.options.file_prefix, time_str, file.device_id
        );
        let output_path = late_dir.join(file_name);

        let mut slice = HashMap::new();
        slice.insert(file.device_id.clone(), file.path.clone());
        let dev_list = vec![file.device_id.clone()];

        self.write_single_slice(
            &output_path,
            file.time,
            &slice,
            &dev_list,
            samples_per_trace,
            sample_rate_hz,
        )?;

        Ok(output_path)
    }

    /// 尝试检测采样率
    fn detect_sample_rate(
        time_buffer: &BTreeMap<DateTime<Utc>, HashMap<String, PathBuf>>,
    ) -> Option<(usize, f64)> {
        for slice in time_buffer.values() {
            for path in slice.values() {
                if let Ok(data) = MseismFile::from_file(path) {
                    let rate = data.sample_count as f64 / 60.0;
                    return Some((data.sample_count, rate));
                }
            }
        }
        None
    }

    /// 主运行循环
    pub fn run(self, sender: Sender<MonitorEvent>) {
        let poll_interval = self.options.poll_interval_secs;
        let wait_window = self.options.wait_window_secs as i64;

        let mut processed_files: HashSet<PathBuf> = HashSet::new();
        let mut flushed_slices: HashSet<DateTime<Utc>> = HashSet::new();
        let mut time_buffer: BTreeMap<DateTime<Utc>, HashMap<String, PathBuf>> =
            BTreeMap::new();
        let mut device_set: HashSet<String> = HashSet::new();
        let mut device_list: Vec<String> = Vec::new();
        let mut generated_count: u64 = 0;
        let mut samples_per_trace = 0usize;
        let mut sample_rate_hz = 0.0;

        let _ = std::fs::create_dir_all(&self.options.output_dir);

        self.log(&sender, "=== 实时监视已启动 ===");
        self.log(
            &sender,
            format!("根目录: {}", self.options.root_dir.display()),
        );
        self.log(
            &sender,
            format!("输出目录: {}", self.options.output_dir.display()),
        );
        self.log(&sender, format!("等待窗口: {}秒", wait_window));
        self.log(&sender, format!("轮询间隔: {}秒", poll_interval));

        match &self.options.output_mode {
            OutputMode::PerMinute => {
                self.log(&sender, "输出模式: 每分钟一个文件");
            }
            OutputMode::Duration(d) => {
                self.log(&sender, format!("输出模式: 每{}分钟一个文件", d));
            }
        }

        if self.options.ignore_late {
            self.log(&sender, "迟到数据: 忽略");
        } else {
            self.log(&sender, "迟到数据: 单独保存到 late/ 目录");
        }

        // 非回填模式：标记所有现有文件为已处理
        if !self.options.backfill {
            let existing = self.scan_all_files();
            let count = existing.len();
            for f in existing {
                processed_files.insert(f.path);
            }
            self.log(
                &sender,
                format!("跳过 {} 个已有文件（仅监视新文件）", count),
            );
        }

        loop {
            if self.is_cancelled() {
                self.log(&sender, "正在停止监视...");

                // 刷新剩余缓冲
                if !time_buffer.is_empty() {
                    self.log(
                        &sender,
                        format!("刷新剩余 {} 个时间片...", time_buffer.len()),
                    );

                    match &self.options.output_mode {
                        OutputMode::PerMinute => {
                            for (time, slice) in &time_buffer {
                                let file_name = format_file_name(
                                    &self.options.file_template,
                                    &self.options.file_prefix,
                                    Some(*time),
                                );
                                let output_path = self
                                    .options
                                    .output_dir
                                    .join(format!("{file_name}.segy"));
                                if self
                                    .write_single_slice(
                                        &output_path,
                                        *time,
                                        slice,
                                        &device_list,
                                        samples_per_trace,
                                        sample_rate_hz,
                                    )
                                    .is_ok()
                                {
                                    generated_count += 1;
                                    self.log(&sender, format!("生成: {}", output_path.display()));
                                    let _ = sender.send(MonitorEvent::FileGenerated(output_path));
                                }
                            }
                        }
                        OutputMode::Duration(_) => {
                            if let Some(&start) = time_buffer.keys().next() {
                                let file_name = format_file_name(
                                    &self.options.file_template,
                                    &self.options.file_prefix,
                                    Some(start),
                                );
                                let output_path = self
                                    .options
                                    .output_dir
                                    .join(format!("{file_name}.segy"));
                                if self
                                    .write_multi_slice(
                                        &output_path,
                                        start,
                                        &time_buffer,
                                        &device_list,
                                        samples_per_trace,
                                        sample_rate_hz,
                                    )
                                    .is_ok()
                                {
                                    generated_count += 1;
                                    self.log(&sender, format!("生成: {}", output_path.display()));
                                    let _ = sender.send(MonitorEvent::FileGenerated(output_path));
                                }
                            }
                        }
                    }
                }

                let _ = sender.send(MonitorEvent::Status {
                    processed: processed_files.len() as u64,
                    generated: generated_count,
                    buffered: 0,
                });
                let _ = sender.send(MonitorEvent::Done(Ok(())));
                return;
            }

            // 1. 扫描新文件
            let all_files = self.scan_all_files();
            let new_files: Vec<ScannedFile> = all_files
                .into_iter()
                .filter(|f| !processed_files.contains(&f.path))
                .collect();

            if !new_files.is_empty() {
                self.log(&sender, format!("发现 {} 个新文件", new_files.len()));
            }

            // 2. 归入时间片或处理迟到数据
            for file in &new_files {
                processed_files.insert(file.path.clone());

                if flushed_slices.contains(&file.time) {
                    // 迟到数据
                    let time_str = file.time.format("%Y-%m-%d %H:%M:%S").to_string();

                    if self.options.ignore_late {
                        self.log(
                            &sender,
                            format!("警告: 迟到数据已忽略: {} ({})", file.path.display(), time_str),
                        );
                        let _ = sender.send(MonitorEvent::LateDataWarning {
                            file: file.path.display().to_string(),
                            time: time_str,
                        });
                    } else {
                        if samples_per_trace == 0 {
                            if let Ok(data) = MseismFile::from_file(&file.path) {
                                samples_per_trace = data.sample_count;
                                sample_rate_hz = data.sample_count as f64 / 60.0;
                            }
                        }

                        match self.save_late_data(file, samples_per_trace, sample_rate_hz) {
                            Ok(path) => {
                                self.log(
                                    &sender,
                                    format!("迟到数据已保存: {}", path.display()),
                                );
                                let _ =
                                    sender.send(MonitorEvent::LateDataSaved(path.display().to_string()));
                            }
                            Err(e) => {
                                self.log(&sender, format!("迟到数据保存失败: {e}"));
                            }
                        }
                    }
                } else {
                    // 加入时间片缓冲
                    time_buffer
                        .entry(file.time)
                        .or_insert_with(HashMap::new)
                        .insert(file.device_id.clone(), file.path.clone());

                    // 更新设备列表
                    if device_set.insert(file.device_id.clone()) {
                        device_list.push(file.device_id.clone());
                        device_list.sort();
                        self.log(&sender, format!("发现新设备: {}", file.device_id));
                    }
                }
            }

            // 3. 检测采样率（首次）
            if samples_per_trace == 0 && !time_buffer.is_empty() {
                if let Some((spt, rate)) = Self::detect_sample_rate(&time_buffer) {
                    samples_per_trace = spt;
                    sample_rate_hz = rate;
                    self.log(
                        &sender,
                        format!("采样率: {:.2} Hz, 每道采样数: {}", rate, spt),
                    );
                }
            }

            // 4. 检查就绪时间片
            let now = Utc::now();
            let mut ready_times: Vec<DateTime<Utc>> = Vec::new();

            for time in time_buffer.keys() {
                let slice_end = *time + chrono::Duration::minutes(1);
                if now > slice_end + chrono::Duration::seconds(wait_window) {
                    ready_times.push(*time);
                }
            }

            // 5. 根据输出模式处理就绪时间片
            match &self.options.output_mode {
                OutputMode::PerMinute => {
                    for time in &ready_times {
                        if let Some(slice) = time_buffer.remove(time) {
                            let file_name = format_file_name(
                                &self.options.file_template,
                                &self.options.file_prefix,
                                Some(*time),
                            );
                            let output_path = self
                                .options
                                .output_dir
                                .join(format!("{file_name}.segy"));

                            match self.write_single_slice(
                                &output_path,
                                *time,
                                &slice,
                                &device_list,
                                samples_per_trace,
                                sample_rate_hz,
                            ) {
                                Ok(()) => {
                                    generated_count += 1;
                                    flushed_slices.insert(*time);
                                    self.log(&sender, format!("生成: {}", output_path.display()));
                                    let _ =
                                        sender.send(MonitorEvent::FileGenerated(output_path));
                                }
                                Err(SegyError::Cancelled) => {
                                    let _ = sender.send(MonitorEvent::Done(Ok(())));
                                    return;
                                }
                                Err(e) => {
                                    self.log(&sender, format!("写入失败: {e}"));
                                    time_buffer.insert(*time, slice);
                                }
                            }
                        }
                    }
                }
                OutputMode::Duration(dur) => {
                    let dur_min = *dur as i64;

                    if !ready_times.is_empty() {
                        // 按 duration 边界对齐分组
                        let first_ready = *ready_times.iter().min().unwrap();
                        let group_start_ts =
                            first_ready.timestamp() - (first_ready.timestamp() % (dur_min * 60));
                        let group_start =
                            Utc.timestamp_opt(group_start_ts, 0).unwrap();
                        let group_end =
                            group_start + chrono::Duration::minutes(dur_min);

                        // 当该组的时间窗口 + 等待窗口已过，才写入
                        if now >= group_end + chrono::Duration::seconds(wait_window) {
                            let mut slices_to_write: BTreeMap<
                                DateTime<Utc>,
                                HashMap<String, PathBuf>,
                            > = BTreeMap::new();

                            for t in &ready_times {
                                if *t >= group_start && *t < group_end {
                                    if let Some(slice) = time_buffer.get(t) {
                                        slices_to_write.insert(*t, slice.clone());
                                    }
                                }
                            }

                            if !slices_to_write.is_empty() {
                                let file_name = format_file_name(
                                    &self.options.file_template,
                                    &self.options.file_prefix,
                                    Some(group_start),
                                );
                                let output_path = self
                                    .options
                                    .output_dir
                                    .join(format!("{file_name}.segy"));

                                match self.write_multi_slice(
                                    &output_path,
                                    group_start,
                                    &slices_to_write,
                                    &device_list,
                                    samples_per_trace,
                                    sample_rate_hz,
                                ) {
                                    Ok(()) => {
                                        generated_count += 1;
                                        for t in slices_to_write.keys() {
                                            time_buffer.remove(t);
                                            flushed_slices.insert(*t);
                                        }
                                        self.log(
                                            &sender,
                                            format!("生成: {}", output_path.display()),
                                        );
                                        let _ = sender
                                            .send(MonitorEvent::FileGenerated(output_path));
                                    }
                                    Err(SegyError::Cancelled) => {
                                        let _ = sender.send(MonitorEvent::Done(Ok(())));
                                        return;
                                    }
                                    Err(e) => {
                                        self.log(&sender, format!("写入失败: {e}"));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 6. 发送状态
            let _ = sender.send(MonitorEvent::Status {
                processed: processed_files.len() as u64,
                generated: generated_count,
                buffered: time_buffer.len() as u64,
            });

            // 7. 等待下一轮
            std::thread::sleep(StdDuration::from_secs(poll_interval));
        }
    }
}

/// 从文件名解析时间戳
fn parse_time_from_filename(filename: &str) -> Option<DateTime<Utc>> {
    let dash_idx = filename.rfind('-')?;
    let time_part = &filename[..dash_idx];
    let naive = NaiveDateTime::parse_from_str(time_part, "%Y-%m-%d_%H_%M").ok()?;
    Some(naive.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_from_filename() {
        let t = parse_time_from_filename("2026-06-28_23_59-24002").unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-06-28 23:59");
    }

    #[test]
    fn test_parse_time_cross_midnight() {
        // 跨午夜文件
        let t = parse_time_from_filename("2026-06-29_00_00-24002").unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-06-29 00:00");
    }
}
