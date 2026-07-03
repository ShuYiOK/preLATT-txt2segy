#![windows_subsystem = "windows"]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui;

use segy_core::{
    gps::{gps_to_map, parse_gps_file},
    synthesize_segy, DatasetBuilder, MonitorEngine, MonitorEvent, MonitorOptions, OutputMode,
    SegyRevision, SynthesisOptions, scan_date_folders, scan_device_folders,
};

// ── 后台线程发给 UI 的消息 ──
enum WorkerMsg {
    Log(String),
    Progress(f32),
    Done(Result<Vec<String>, String>),
    // 监视相关
    MonitorStatus {
        processed: u64,
        generated: u64,
        buffered: u64,
    },
    MonitorDone(Result<(), String>),
}

struct SegyGuiApp {
    input_dir: String,
    output_dir: String,
    file_prefix: String,
    file_template: String,
    gps_file: String,
    sample_rate: f64,
    auto_sample_rate: bool,
    per_minute: bool,
    revision_idx: usize,

    mode: AppMode,

    // 设备文件夹列表
    all_devices: Vec<String>,
    selected_devices: HashSet<String>,

    // 日期文件夹列表 + 范围（批处理模式）
    all_dates: Vec<String>,
    use_date_range: bool,
    date_start: String,
    date_end: String,

    // 实时监视选项
    monitor_duration: u32,
    wait_window_secs: u64,
    ignore_late: bool,
    backfill: bool,

    // 监视状态
    monitor_processed: u64,
    monitor_generated: u64,
    monitor_buffered: u64,

    converting: bool,
    progress: f32,
    logs: Vec<String>,
    rx: Option<Receiver<WorkerMsg>>,
    cancel_flag: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Batch,
    Monitor,
}

const REVISIONS: [&str; 3] = ["Rev 0", "Rev 1", "Rev 2"];
const TEMPLATE_PRESETS: &[(&str, &str)] = &[
    ("前缀_年月日_时分", "{prefix}_{YYYY}{MM}{DD}_{hh}{mm}"),
    ("前缀_年-月-日_时_分_秒", "{prefix}_{YYYY}-{MM}-{DD}_{hh}_{mm}_{ss}"),
    ("年-月-日_时_分_秒", "{YYYY}-{MM}-{DD}_{hh}_{mm}_{ss}"),
    ("年月日时分秒", "{YYYY}{MM}{DD}{hh}{mm}{ss}"),
    ("仅前缀", "{prefix}"),
];

impl Default for SegyGuiApp {
    fn default() -> Self {
        Self {
            input_dir: String::new(),
            output_dir: String::new(),
            file_prefix: "microseismic".into(),
            file_template: "{prefix}_{YYYY}{MM}{DD}_{hh}{mm}".into(),
            gps_file: String::new(),
            sample_rate: 5000.0,
            auto_sample_rate: true,
            per_minute: true,
            revision_idx: 2,
            mode: AppMode::Batch,
            all_devices: Vec::new(),
            selected_devices: HashSet::new(),
            all_dates: Vec::new(),
            use_date_range: false,
            date_start: String::new(),
            date_end: String::new(),
            monitor_duration: 60,
            wait_window_secs: 60,
            ignore_late: false,
            backfill: true,
            monitor_processed: 0,
            monitor_generated: 0,
            monitor_buffered: 0,
            converting: false,
            progress: 0.0,
            logs: Vec::new(),
            rx: None,
            cancel_flag: None,
        }
    }
}

impl SegyGuiApp {
    fn log(&mut self, msg: impl Into<String>) {
        self.logs.push(msg.into());
        if self.logs.len() > 500 {
            let drop = self.logs.len() - 500;
            self.logs.drain(0..drop);
        }
    }

    fn stop_convert(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, Ordering::Relaxed);
            self.log("正在停止...");
        }
    }

    fn start_convert(&mut self) {
        let input = PathBuf::from(&self.input_dir);
        let output = PathBuf::from(&self.output_dir);
        let prefix = self.file_prefix.clone();
        let template = self.file_template.clone();
        let gps_path = if self.gps_file.is_empty() {
            None
        } else {
            Some(PathBuf::from(&self.gps_file))
        };
        let auto_sr = self.auto_sample_rate;
        let sample_rate = self.sample_rate;
        let per_minute = self.per_minute;
        let revision = match self.revision_idx {
            0 => SegyRevision::Rev0,
            1 => SegyRevision::Rev1,
            _ => SegyRevision::Rev2,
        };
        let use_gps = gps_path.is_some();

        // 选中设备列表（空则全部）
        let selected = if self.selected_devices.is_empty() {
            None
        } else {
            Some(self.selected_devices.clone())
        };

        // 日期范围
        let date_range = if self.use_date_range
            && !self.date_start.trim().is_empty()
            && !self.date_end.trim().is_empty()
        {
            match (
                chrono::NaiveDate::parse_from_str(self.date_start.trim(), "%Y-%m-%d"),
                chrono::NaiveDate::parse_from_str(self.date_end.trim(), "%Y-%m-%d"),
            ) {
                (Ok(s), Ok(e)) => Some((s, e)),
                _ => None,
            }
        } else {
            None
        };

        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.rx = Some(rx);
        self.cancel_flag = Some(cancel_flag.clone());
        self.converting = true;
        self.progress = 0.0;
        self.logs.clear();
        self.log("开始转换...");

        std::thread::spawn(move || {
            let t = tx.clone();

            let _ = t.send(WorkerMsg::Log(format!("输入: {}", input.display())));
            let _ = t.send(WorkerMsg::Log(format!("输出: {}", output.display())));

            // GPS
            let gps_map = if let Some(ref gp) = gps_path {
                let _ = t.send(WorkerMsg::Log(format!("GPS: {}", gp.display())));
                match parse_gps_file(gp) {
                    Ok(stations) => {
                        let _ = t.send(WorkerMsg::Log(format!("  {} 个站点", stations.len())));
                        gps_to_map(stations)
                    }
                    Err(e) => {
                        let _ = t.send(WorkerMsg::Done(Err(format!("GPS 解析错误: {e}"))));
                        return;
                    }
                }
            } else {
                HashMap::new()
            };

            // Dataset
            let mut builder = DatasetBuilder::new(&input).with_gps(gps_map.clone());
            if !auto_sr {
                builder = builder.with_sample_rate(sample_rate);
            }
            if let Some(ref dev_set) = selected {
                builder = builder.with_devices(dev_set.clone());
            }
            if let Some((ds, de)) = date_range {
                builder = builder.with_date_range(ds, de);
                let _ = t.send(WorkerMsg::Log(format!(
                    "日期范围: {} ~ {}",
                    ds, de
                )));
            }

            let dataset = match builder.build() {
                Ok(ds) => ds,
                Err(e) => {
                    let _ = t.send(WorkerMsg::Done(Err(format!("数据集构建失败: {e}"))));
                    return;
                }
            };

            let _ = t.send(WorkerMsg::Log(format!(
                "设备: {}  时间片: {}",
                dataset.num_devices(),
                dataset.num_time_slices()
            )));
            let _ = t.send(WorkerMsg::Log(format!(
                "采样率: {:.2} Hz  每道采样数: {}",
                dataset.sample_rate_hz, dataset.samples_per_minute
            )));

            if let Some((start, end)) = dataset.time_range() {
                let _ = t.send(WorkerMsg::Log(format!(
                    "时间: {} ~ {}",
                    start.format("%Y-%m-%d %H:%M:%S"),
                    end.format("%Y-%m-%d %H:%M:%S")
                )));
            }

            let total = dataset.num_time_slices() as u64;
            let t2 = tx.clone();
            let mut options = SynthesisOptions::default();
            options.output_dir = output;
            options.file_prefix = prefix;
            options.file_template = template;
            options.revision = revision;
            options.per_minute_files = per_minute;
            options.use_gps_coords = use_gps;
            options.cancel_flag = Some(cancel_flag.clone());
            options.progress_callback = Some(Box::new(move |current, _tt| {
                let pct = if total > 0 {
                    current as f32 / total as f32
                } else {
                    0.0
                };
                let _ = t2.send(WorkerMsg::Progress(pct));
                if current % 10 == 0 || current == total {
                    let _ = t2.send(WorkerMsg::Log(format!(
                        "  {current}/{total} ({:.0}%)",
                        pct * 100.0
                    )));
                }
            }));

            let _ = t.send(WorkerMsg::Log("正在合成 SEG-Y...".into()));

            match synthesize_segy(&dataset, &options) {
                Ok(files) => {
                    let mut info = Vec::new();
                    for f in &files {
                        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                        info.push(format!(
                            "  {} ({:.2} MB)",
                            f.display(),
                            size as f64 / 1024.0 / 1024.0
                        ));
                    }
                    let _ = tx.send(WorkerMsg::Done(Ok(info)));
                }
                Err(e) => {
                    let _ = tx.send(WorkerMsg::Done(Err(format!("合成失败: {e}"))));
                }
            }
        });
    }

    fn start_monitor(&mut self) {
        let input = PathBuf::from(&self.input_dir);
        let output = PathBuf::from(&self.output_dir);
        let prefix = self.file_prefix.clone();
        let template = self.file_template.clone();
        let gps_path = if self.gps_file.is_empty() {
            None
        } else {
            Some(PathBuf::from(&self.gps_file))
        };
        let revision = match self.revision_idx {
            0 => SegyRevision::Rev0,
            1 => SegyRevision::Rev1,
            _ => SegyRevision::Rev2,
        };
        let use_gps = gps_path.is_some();

        let selected = if self.selected_devices.is_empty() {
            None
        } else {
            Some(self.selected_devices.clone())
        };

        let output_mode = if self.per_minute {
            OutputMode::PerMinute
        } else {
            OutputMode::Duration(self.monitor_duration)
        };

        let ignore_late = self.ignore_late;
        let backfill = self.backfill;
        let wait_window = self.wait_window_secs;

        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.rx = Some(rx);
        self.cancel_flag = Some(cancel_flag.clone());
        self.converting = true;
        self.progress = 0.0;
        self.logs.clear();
        self.monitor_processed = 0;
        self.monitor_generated = 0;
        self.monitor_buffered = 0;
        self.log("开始实时监视...");

        std::thread::spawn(move || {
            // GPS
            let gps_map = if let Some(ref gp) = gps_path {
                let _ = tx.send(WorkerMsg::Log(format!("GPS: {}", gp.display())));
                match parse_gps_file(gp) {
                    Ok(stations) => {
                        let _ = tx.send(WorkerMsg::Log(format!("  {} 个站点", stations.len())));
                        gps_to_map(stations)
                    }
                    Err(e) => {
                        let _ = tx.send(WorkerMsg::MonitorDone(Err(format!(
                            "GPS 解析错误: {e}"
                        ))));
                        return;
                    }
                }
            } else {
                HashMap::new()
            };

            let mut options = MonitorOptions::default();
            options.root_dir = input;
            options.output_dir = output;
            options.file_prefix = prefix;
            options.file_template = template;
            options.revision = revision;
            options.use_gps_coords = use_gps;
            options.gps = gps_map;
            options.selected_devices = selected;
            options.output_mode = output_mode;
            options.ignore_late = ignore_late;
            options.backfill = backfill;
            options.wait_window_secs = wait_window;
            options.cancel_flag = Some(cancel_flag);

            let engine = MonitorEngine::new(options);

            // MonitorEvent -> WorkerMsg 转发线程
            let (monitor_tx, monitor_rx) = mpsc::channel();
            let tx_clone = tx.clone();

            std::thread::spawn(move || {
                let mut done_sent = false;
                while let Ok(event) = monitor_rx.recv() {
                    match event {
                        MonitorEvent::Log(s) => {
                            let _ = tx_clone.send(WorkerMsg::Log(s));
                        }
                        MonitorEvent::Status {
                            processed,
                            generated,
                            buffered,
                        } => {
                            let _ = tx_clone.send(WorkerMsg::MonitorStatus {
                                processed,
                                generated,
                                buffered,
                            });
                        }
                        MonitorEvent::FileGenerated(path) => {
                            let _ = tx_clone.send(WorkerMsg::Log(format!(
                                "生成文件: {}",
                                path.display()
                            )));
                        }
                        MonitorEvent::LateDataWarning { file, time } => {
                            let _ = tx_clone.send(WorkerMsg::Log(format!(
                                "警告: 迟到数据已忽略: {} ({})",
                                file, time
                            )));
                        }
                        MonitorEvent::LateDataSaved(path) => {
                            let _ = tx_clone.send(WorkerMsg::Log(format!(
                                "迟到数据已保存: {}",
                                path
                            )));
                        }
                        MonitorEvent::Done(res) => {
                            let _ = tx_clone.send(WorkerMsg::MonitorDone(res));
                            done_sent = true;
                            break;
                        }
                    }
                }
                if !done_sent {
                    let _ = tx_clone
                        .send(WorkerMsg::MonitorDone(Err("监视引擎异常退出".into())));
                }
            });

            // 运行监视引擎（阻塞直到取消或出错）
            engine.run(monitor_tx);
        });
    }

    fn poll_worker(&mut self) {
        let rx = match self.rx.take() {
            Some(rx) => rx,
            None => return,
        };
        let mut keep = true;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                WorkerMsg::Log(s) => self.log(s),
                WorkerMsg::Progress(p) => self.progress = p,
                WorkerMsg::Done(res) => {
                    self.converting = false;
                    match res {
                        Ok(file_info) => {
                            self.progress = 1.0;
                            self.log("");
                            self.log("输出文件:");
                            for line in &file_info {
                                self.log(line.clone());
                            }
                            self.log(format!("完成! 共 {} 个文件", file_info.len()));
                        }
                        Err(e) => {
                            self.log(format!("错误: {e}"));
                        }
                    }
                    keep = false;
                }
                WorkerMsg::MonitorStatus {
                    processed,
                    generated,
                    buffered,
                } => {
                    self.monitor_processed = processed;
                    self.monitor_generated = generated;
                    self.monitor_buffered = buffered;
                }
                WorkerMsg::MonitorDone(res) => {
                    self.converting = false;
                    match res {
                        Ok(()) => {
                            self.log("监视已停止");
                        }
                        Err(e) => {
                            self.log(format!("错误: {e}"));
                        }
                    }
                    keep = false;
                }
            }
        }
        if keep {
            self.rx = Some(rx);
        }
    }
}

impl eframe::App for SegyGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.converting {
            ctx.request_repaint();
        }
        self.poll_worker();

        // 顶部标题
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("微地震 TXT → SEG-Y 转换工具--lsby1984");
            ui.add_space(2.0);
        });

        // 底部日志
        egui::TopBottomPanel::bottom("bottom_log")
            .resizable(true)
            .min_height(120.0)
            .show(ctx, |ui| {
                ui.label("日志:");
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(line);
                        }
                    });
            });

        // 中间参数面板
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);

            // ── 模式切换 ──
            ui.horizontal(|ui| {
                ui.label("模式:");
                if ui
                    .radio_value(&mut self.mode, AppMode::Batch, "批处理")
                    .changed()
                {
                    if self.converting {
                        self.stop_convert();
                    }
                }
                if ui
                    .radio_value(&mut self.mode, AppMode::Monitor, "实时监视")
                    .changed()
                {
                    if self.converting {
                        self.stop_convert();
                    }
                }
            });

            ui.separator();
            ui.add_space(4.0);

            // ── 输入目录 ──
            ui.horizontal(|ui| {
                ui.label("输入目录:").on_hover_text("包含日期文件夹的根目录");
                ui.add(
                    egui::TextEdit::singleline(&mut self.input_dir)
                        .desired_width(400.0)
                        .hint_text("Y:\\115open\\Mseism\\microseismic"),
                );
                if ui.button("浏览...").clicked() && !self.converting {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("选择输入目录")
                        .pick_folder()
                    {
                        self.input_dir = path.to_string_lossy().into_owned();
                        // 自动扫描子文件夹
                        self.all_devices = scan_device_folders(&self.input_dir);
                        self.selected_devices.clear();
                        self.selected_devices.extend(self.all_devices.iter().cloned());
                        // 扫描日期文件夹
                        self.all_dates = scan_date_folders(&self.input_dir);
                        if self.all_dates.len() >= 1 {
                            self.date_start = self.all_dates[0].clone();
                            self.date_end = self.all_dates.last().unwrap().clone();
                        }
                    }
                }
            });

            ui.add_space(4.0);

            // ── 设备文件夹勾选 ──
            if !self.all_devices.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "设备文件夹 ({} 个, 已选 {}):",
                        self.all_devices.len(),
                        self.selected_devices.len()
                    ));
                    if ui.button("全选").clicked() && !self.converting {
                        self.selected_devices
                            .extend(self.all_devices.iter().cloned());
                    }
                    if ui.button("全不选").clicked() && !self.converting {
                        self.selected_devices.clear();
                    }
                    if ui.button("导入列表...").clicked() && !self.converting {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("导入设备列表 (每行一个文件夹名)")
                            .add_filter("文本文件", &["txt"])
                            .pick_file()
                        {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                let names: HashSet<String> = content
                                    .lines()
                                    .map(|l| l.trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect();
                                self.selected_devices.clear();
                                for name in &names {
                                    if self.all_devices.contains(name) {
                                        self.selected_devices.insert(name.clone());
                                    }
                                }
                            }
                        }
                    }
                    if ui.button("导出列表...").clicked() && !self.converting {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("保存设备列表")
                            .set_file_name("device_list.txt")
                            .save_file()
                        {
                            let content: String = self
                                .selected_devices
                                .iter()
                                .map(|s| s.clone())
                                .collect::<Vec<_>>()
                                .join("\n");
                            let _ = std::fs::write(path, content);
                        }
                    }
                });

                egui::ScrollArea::vertical()
                    .max_height(100.0)
                    .show(ui, |ui| {
                        ui.columns(4, |cols| {
                            let chunk = (self.all_devices.len() + 3) / 4;
                            for (ci, col) in cols.iter_mut().enumerate() {
                                let start = ci * chunk;
                                let end = ((ci + 1) * chunk).min(self.all_devices.len());
                                for dev in &self.all_devices[start..end] {
                                    let mut checked =
                                        self.selected_devices.contains(dev);
                                    if col.checkbox(&mut checked, dev).changed() {
                                        if checked {
                                            self.selected_devices.insert(dev.clone());
                                        } else {
                                            self.selected_devices.remove(dev);
                                        }
                                    }
                                }
                            }
                        });
                    });
            }

            ui.add_space(4.0);

            // ── 模式特有选项 ──
            match self.mode {
                AppMode::Batch => {
                    // 日期范围选择
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.use_date_range, "使用日期范围");
                        if self.use_date_range {
                            ui.label("起始:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.date_start)
                                    .desired_width(100.0)
                                    .hint_text("YYYY-MM-DD"),
                            );
                            ui.label("结束:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.date_end)
                                    .desired_width(100.0)
                                    .hint_text("YYYY-MM-DD"),
                            );
                            if !self.all_dates.is_empty() {
                                ui.label(format!(
                                    "(可用: {} ~ {})",
                                    self.all_dates.first().unwrap(),
                                    self.all_dates.last().unwrap()
                                ));
                            }
                        }
                    });
                    ui.add_space(4.0);

                    // 按分钟拆分
                    ui.checkbox(
                        &mut self.per_minute,
                        "按分钟拆分输出（每分钟一个 .sgy 文件）",
                    );
                }
                AppMode::Monitor => {
                    // 等待窗口
                    ui.horizontal(|ui| {
                        ui.label("等待窗口:");
                        ui.add(
                            egui::DragValue::new(&mut self.wait_window_secs)
                                .range(10..=300)
                                .suffix(" 秒"),
                        );
                        ui.label("(时间片结束后等多久再写入)");
                    });
                    ui.add_space(4.0);

                    // 输出模式
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.per_minute, "每分钟一个文件");
                        if !self.per_minute {
                            ui.label("时长:");
                            ui.add(
                                egui::DragValue::new(&mut self.monitor_duration)
                                    .range(1..=1440)
                                    .suffix(" 分钟"),
                            );
                        }
                    });
                    ui.add_space(4.0);

                    // 迟到数据选项
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.ignore_late, "忽略迟到数据");
                        if !self.ignore_late {
                            ui.label("(迟到数据将保存到 late/ 目录)");
                        }
                    });
                    ui.add_space(4.0);

                    // 回填选项
                    ui.checkbox(&mut self.backfill, "回填已有文件（处理启动时已存在的文件）");
                }
            }

            ui.add_space(4.0);

            // ── 输出目录 ──
            ui.horizontal(|ui| {
                ui.label("输出目录:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.output_dir)
                        .desired_width(400.0)
                        .hint_text("输出 .sgy 文件的目录"),
                );
                if ui.button("浏览...").clicked() && !self.converting {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("选择输出目录")
                        .pick_folder()
                    {
                        self.output_dir = path.to_string_lossy().into_owned();
                    }
                }
            });

            ui.add_space(4.0);

            // ── 文件前缀 + 文件名模板 ──
            ui.horizontal(|ui| {
                ui.label("文件前缀:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.file_prefix)
                        .desired_width(120.0),
                );

                ui.separator();

                ui.label("命名格式:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.file_template)
                        .desired_width(260.0)
                        .hint_text("{prefix}_{YYYY}{MM}{DD}_{hh}{mm}"),
                );

                egui::ComboBox::from_id_salt("template_preset")
                    .selected_text("预设▼")
                    .show_ui(ui, |ui| {
                        for (name, tmpl) in TEMPLATE_PRESETS.iter() {
                            if ui.selectable_label(false, *name).clicked() {
                                self.file_template = (*tmpl).to_string();
                            }
                        }
                    });
            });

            ui.add_space(4.0);

            // ── 采样率 + 版本 ──
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.auto_sample_rate, "自动检测采样率");
                ui.add_enabled(
                    !self.auto_sample_rate,
                    egui::DragValue::new(&mut self.sample_rate)
                        .speed(100.0)
                        .range(1.0..=50000.0)
                        .suffix(" Hz"),
                );

                ui.separator();

                ui.label("SEG-Y:");
                let mut rev_idx = self.revision_idx;
                egui::ComboBox::from_id_salt("rev")
                    .selected_text(REVISIONS[rev_idx])
                    .show_ui(ui, |ui| {
                        for (i, r) in REVISIONS.iter().enumerate() {
                            if ui.selectable_label(rev_idx == i, *r).clicked() {
                                rev_idx = i;
                            }
                        }
                    });
                self.revision_idx = rev_idx;
            });

            ui.add_space(4.0);

            // ── GPS 文件 ──
            ui.horizontal(|ui| {
                ui.label("GPS文件:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.gps_file)
                        .desired_width(400.0)
                        .hint_text("可选，留空则使用 TXT 内嵌坐标"),
                );
                if ui.button("浏览...").clicked() && !self.converting {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("选择 GPS 文件")
                        .add_filter("文本文件", &["txt", "csv"])
                        .pick_file()
                    {
                        self.gps_file = path.to_string_lossy().into_owned();
                    }
                }
            });

            ui.add_space(12.0);

            // ── 转换/监视 按钮 ──
            ui.horizontal(|ui| {
                let can_start =
                    !self.input_dir.is_empty() && !self.output_dir.is_empty() && !self.converting;

                match self.mode {
                    AppMode::Batch => {
                        let btn_text = if self.converting { "转换中..." } else { "开始转换" };
                        let resp = ui.add_enabled(
                            can_start,
                            egui::Button::new(btn_text).min_size([140.0, 36.0].into()),
                        );
                        if resp.clicked() && can_start {
                            self.start_convert();
                        }
                    }
                    AppMode::Monitor => {
                        let btn_text = if self.converting {
                            "监视中..."
                        } else {
                            "开始监视"
                        };
                        let resp = ui.add_enabled(
                            can_start,
                            egui::Button::new(btn_text).min_size([140.0, 36.0].into()),
                        );
                        if resp.clicked() && can_start {
                            self.start_monitor();
                        }
                    }
                }

                if self.converting {
                    let stop_label = if self.mode == AppMode::Monitor {
                        "停止监视"
                    } else {
                        "停止"
                    };
                    let stop_resp = ui.add(
                        egui::Button::new(stop_label)
                            .min_size([100.0, 36.0].into())
                            .fill(egui::Color32::from_rgb(180, 60, 60)),
                    );
                    if stop_resp.clicked() {
                        self.stop_convert();
                    }
                }
            });

            ui.add_space(8.0);

            // ── 进度/状态显示 ──
            match self.mode {
                AppMode::Batch => {
                    ui.add(
                        egui::ProgressBar::new(self.progress)
                            .show_percentage()
                            .desired_width(500.0),
                    );
                    ui.add_space(4.0);
                    if self.converting {
                        ui.label("正在处理...");
                    } else if self.progress >= 1.0 && !self.logs.is_empty() {
                        ui.label("转换完成，请查看日志");
                    } else {
                        ui.label("就绪");
                    }
                }
                AppMode::Monitor => {
                    if self.converting {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "已处理: {}  已生成: {}  缓冲中: {}",
                                self.monitor_processed, self.monitor_generated, self.monitor_buffered
                            ));
                            ui.spinner();
                            ui.label("监视运行中...");
                        });
                    } else {
                        ui.label(format!(
                            "上次状态 - 已处理: {}  已生成: {}  缓冲中: {}",
                            self.monitor_processed, self.monitor_generated, self.monitor_buffered
                        ));
                        ui.label("就绪");
                    }
                }
            }
        });
    }
}

fn load_chinese_font(ctx: &egui::Context) {
    let font_paths = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];

    let mut fonts = egui::FontDefinitions::default();

    for path in font_paths.iter() {
        if let Ok(data) = std::fs::read(path) {
            let name = format!("chinese_{}", path.split('\\').last().unwrap_or("font"));
            fonts
                .font_data
                .insert(name.clone(), std::sync::Arc::new(egui::FontData::from_owned(data)));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, name.clone());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(name);
            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([700.0, 400.0])
            .with_title("微地震 TXT → SEG-Y 转换工具--lsby1984"),
        ..Default::default()
    };
    eframe::run_native(
        "segy-gui",
        opts,
        Box::new(|cc| {
            load_chinese_font(&cc.egui_ctx);
            Ok(Box::new(SegyGuiApp::default()))
        }),
    )
}
