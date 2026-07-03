use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::dataset::TimeAlignedDataset;
use crate::gps::GpsStation;
use crate::header::{BinaryHeader, SegyRevision, TextHeader};
use crate::parser::MseismFile;
use crate::trace::TraceHeader;
use crate::writer::SegyWriter;
use crate::SegyError;

pub struct SynthesisOptions {
    pub output_dir: PathBuf,
    pub file_prefix: String,
    pub file_template: String,
    pub revision: SegyRevision,
    pub use_gps_coords: bool,
    pub per_minute_files: bool,
    pub progress_callback: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for SynthesisOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SynthesisOptions")
            .field("output_dir", &self.output_dir)
            .field("file_prefix", &self.file_prefix)
            .field("file_template", &self.file_template)
            .field("revision", &self.revision)
            .field("use_gps_coords", &self.use_gps_coords)
            .field("per_minute_files", &self.per_minute_files)
            .field("progress_callback", &self.progress_callback.is_some())
            .finish()
    }
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./output"),
            file_prefix: "microseismic".to_string(),
            file_template: "{prefix}_{YYYY}{MM}{DD}_{hh}{mm}".to_string(),
            revision: SegyRevision::Rev2,
            use_gps_coords: true,
            per_minute_files: false,
            progress_callback: None,
            cancel_flag: None,
        }
    }
}

pub(crate) fn format_file_name(template: &str, prefix: &str, time: Option<DateTime<Utc>>) -> String {
    let mut result = template.to_string();
    result = result.replace("{prefix}", prefix);

    if let Some(t) = time {
        result = result.replace("{YYYY}", &t.format("%Y").to_string());
        result = result.replace("{YY}", &t.format("%y").to_string());
        result = result.replace("{MM}", &t.format("%m").to_string());
        result = result.replace("{DD}", &t.format("%d").to_string());
        result = result.replace("{hh}", &t.format("%H").to_string());
        result = result.replace("{mm}", &t.format("%M").to_string());
        result = result.replace("{ss}", &t.format("%S").to_string());
        result = result.replace("{DOY}", &t.format("%j").to_string());
        result = result.replace("{j}", &t.format("%j").to_string());
    } else {
        for placeholder in &[
            "{YYYY}", "{YY}", "{MM}", "{DD}", "{hh}", "{mm}", "{ss}", "{DOY}", "{j}",
        ] {
            result = result.replace(placeholder, "");
        }
    }

    if result.is_empty() {
        "output".to_string()
    } else {
        result
    }
}

pub fn synthesize_segy(
    dataset: &TimeAlignedDataset,
    options: &SynthesisOptions,
) -> Result<Vec<PathBuf>, SegyError> {
    std::fs::create_dir_all(&options.output_dir)?;

    if options.cancel_flag.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false) {
        return Err(SegyError::Cancelled);
    }

    if options.per_minute_files {
        synthesize_per_minute(dataset, options)
    } else {
        synthesize_single_file(dataset, options)
    }
}

fn synthesize_single_file(
    dataset: &TimeAlignedDataset,
    options: &SynthesisOptions,
) -> Result<Vec<PathBuf>, SegyError> {
    let num_slices = dataset.time_slices.len() as u64;
    let total_traces = num_slices * dataset.devices.len() as u64;
    let samples_per_trace = dataset.samples_per_minute;

    let file_name = format_file_name(
        &options.file_template,
        &options.file_prefix,
        dataset.time_range().map(|(s, _e)| s),
    );
    let output_path = options
        .output_dir
        .join(format!("{file_name}.segy"));

    let mut text_header = TextHeader::default();
    text_header.use_utf8 = true;
    text_header.set_line(1, "SEG-Y Rev 2.0 - Microseismic Monitoring Data");
    text_header.set_line(2, &format!("Stations: {}", dataset.devices.len()));
    text_header.set_line(
        3,
        &format!(
            "Sample rate: {:.2} Hz, Samples/trace: {}",
            dataset.sample_rate_hz, samples_per_trace
        ),
    );
    if let Some((start, end)) = dataset.time_range() {
        text_header.set_line(4, &format!("Start: {}", start.format("%Y-%m-%d %H:%M:%S UTC")));
        text_header.set_line(5, &format!("End:   {}", end.format("%Y-%m-%d %H:%M:%S UTC")));
    }
    text_header.set_line(6, &format!("Total traces: {}", total_traces));

    let sample_interval_us = dataset.sample_interval_us();

    let mut binary_header = BinaryHeader::default()
        .with_revision(options.revision);
    binary_header.samples_per_trace = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
    binary_header.samples_per_trace_original = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
    binary_header.sample_interval = sample_interval_us as u16;
    binary_header.sample_interval_original = sample_interval_us as u16;
    binary_header.traces_per_ensemble = dataset.devices.len() as u16;

    let mut writer = SegyWriter::create(&output_path)?;
    writer.write_header(&text_header, &binary_header)?;

    let mut trace_seq: i32 = 0;

    for (slice_idx, (_time, slice_files)) in dataset.time_slices.iter().enumerate() {
        if options.cancel_flag.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(SegyError::Cancelled);
        }

        let traces_for_slice = read_slice_traces(
            slice_files,
            &dataset.devices,
            &dataset.gps,
            options.use_gps_coords,
            samples_per_trace,
        )?;

        for (mut thdr, samples) in traces_for_slice {
            trace_seq += 1;
            thdr.trace_sequence_file = trace_seq;
            thdr.trace_sequence_line = trace_seq;
            thdr.ensemble_number = (slice_idx + 1) as i32;
            writer.write_trace(&thdr, &samples)?;
        }

        if let Some(cb) = &options.progress_callback {
            cb((slice_idx + 1) as u64, num_slices);
        }
    }

    writer.finish()?;
    Ok(vec![output_path])
}

fn synthesize_per_minute(
    dataset: &TimeAlignedDataset,
    options: &SynthesisOptions,
) -> Result<Vec<PathBuf>, SegyError> {
    let mut output_files = Vec::new();
    let num_slices = dataset.time_slices.len() as u64;
    let samples_per_trace = dataset.samples_per_minute;
    let sample_interval_us = dataset.sample_interval_us();

    for (slice_idx, (time, slice_files)) in dataset.time_slices.iter().enumerate() {
        if options.cancel_flag.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(SegyError::Cancelled);
        }

        let file_name = format_file_name(
            &options.file_template,
            &options.file_prefix,
            Some(*time),
        );
        let output_path = options.output_dir.join(format!("{file_name}.segy"));

        let mut text_header = TextHeader::default();
        text_header.use_utf8 = true;
        text_header.set_line(1, "SEG-Y Rev 2.0 - Microseismic Monitoring Data");
        text_header.set_line(2, &format!("Stations: {}", dataset.devices.len()));
        text_header.set_line(
            3,
            &format!(
                "Sample rate: {:.2} Hz",
                dataset.sample_rate_hz
            ),
        );
        text_header.set_line(4, &format!("Time: {}", time.format("%Y-%m-%d %H:%M:%S UTC")));

        let mut binary_header = BinaryHeader::default()
            .with_revision(options.revision);
        binary_header.samples_per_trace = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.samples_per_trace_original = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
        binary_header.sample_interval = sample_interval_us as u16;
        binary_header.sample_interval_original = sample_interval_us as u16;
        binary_header.traces_per_ensemble = dataset.devices.len() as u16;

        let mut writer = SegyWriter::create(&output_path)?;
        writer.write_header(&text_header, &binary_header)?;

        let traces = read_slice_traces(
            slice_files,
            &dataset.devices,
            &dataset.gps,
            options.use_gps_coords,
            samples_per_trace,
        )?;

        for (trace_idx, (mut thdr, samples)) in traces.into_iter().enumerate() {
            thdr.trace_sequence_file = (trace_idx + 1) as i32;
            thdr.trace_sequence_line = (trace_idx + 1) as i32;
            thdr.ensemble_number = 1;
            thdr.trace_number_within_ensemble = (trace_idx + 1) as i32;
            writer.write_trace(&thdr, &samples)?;
        }

        writer.finish()?;
        output_files.push(output_path);

        if let Some(cb) = &options.progress_callback {
            cb((slice_idx + 1) as u64, num_slices);
        }
    }

    Ok(output_files)
}

pub(crate) fn read_slice_traces(
    slice_files: &HashMap<String, PathBuf>,
    devices: &[String],
    gps: &HashMap<String, GpsStation>,
    use_gps: bool,
    expected_samples: usize,
) -> Result<Vec<(TraceHeader, Vec<f32>)>, SegyError> {
    let mut results = Vec::with_capacity(devices.len());

    for device in devices {
        let path = match slice_files.get(device) {
            Some(p) => p,
            None => {
                results.push((missing_trace_header(device, expected_samples, use_gps, gps), vec![0.0; expected_samples]));
                continue;
            }
        };

        let data = MseismFile::from_file(path)?;
        let mut thdr = build_trace_header(&data, use_gps, gps);
        thdr.number_of_samples = if data.sample_count <= 65535 { data.sample_count as u16 } else { 0 };

        let samples = if data.sample_count >= expected_samples {
            data.samples[..expected_samples].to_vec()
        } else {
            let mut v = data.samples.clone();
            v.resize(expected_samples, 0.0);
            v
        };

        results.push((thdr, samples));
    }

    Ok(results)
}

fn build_trace_header(
    data: &MseismFile,
    use_gps: bool,
    gps_map: &HashMap<String, GpsStation>,
) -> TraceHeader {
    let mut thdr = TraceHeader::default();
    thdr.trace_id_code = 1;
    thdr.set_time(&data.start_time);
    thdr.time_basis_code = 1;
    thdr.data_use = 1;
    thdr.number_of_horizontally_summed = 1;
    thdr.number_of_vertically_summed = 1;
    thdr.coordinate_units = 1;

    if use_gps {
        if let Some(gps) = gps_map.get(&data.device_id) {
            thdr.scalar_for_coordinates = -1000;
            thdr.scalar_for_elevations = -10;
            thdr.group_x = (gps.utm_x * 1000.0) as i32;
            thdr.group_y = (gps.utm_y * 1000.0) as i32;
            thdr.source_x = (gps.utm_x * 1000.0) as i32;
            thdr.source_y = (gps.utm_y * 1000.0) as i32;
            thdr.receiver_group_elevation = (gps.elevation * 10.0) as i32;
            thdr.surface_elevation_at_source = (gps.elevation * 10.0) as i32;
        } else {
            thdr.scalar_for_coordinates = -1000;
            thdr.scalar_for_elevations = -10;
            thdr.group_x = (data.x * 1000.0) as i32;
            thdr.group_y = (data.y * 1000.0) as i32;
            thdr.source_x = (data.x * 1000.0) as i32;
            thdr.source_y = (data.y * 1000.0) as i32;
            thdr.receiver_group_elevation = (data.elevation * 10.0) as i32;
            thdr.surface_elevation_at_source = (data.elevation * 10.0) as i32;
        }
    } else {
        thdr.scalar_for_coordinates = -1000;
        thdr.scalar_for_elevations = -10;
        thdr.group_x = (data.x * 1000.0) as i32;
        thdr.group_y = (data.y * 1000.0) as i32;
        thdr.source_x = (data.x * 1000.0) as i32;
        thdr.source_y = (data.y * 1000.0) as i32;
        thdr.receiver_group_elevation = (data.elevation * 10.0) as i32;
        thdr.surface_elevation_at_source = (data.elevation * 10.0) as i32;
    }

    thdr
}

fn missing_trace_header(
    device_id: &str,
    samples: usize,
    use_gps: bool,
    gps_map: &HashMap<String, GpsStation>,
) -> TraceHeader {
    let mut thdr = TraceHeader::default();
    thdr.trace_id_code = 0;
    thdr.number_of_samples = if samples <= 65535 { samples as u16 } else { 0 };
    thdr.data_use = 0;

    if use_gps {
        if let Some(gps) = gps_map.get(device_id) {
            thdr.scalar_for_coordinates = -1000;
            thdr.group_x = (gps.utm_x * 1000.0) as i32;
            thdr.group_y = (gps.utm_y * 1000.0) as i32;
        }
    }

    thdr
}

pub fn synthesize_single_minute(
    minute_files: &HashMap<String, PathBuf>,
    devices: &[String],
    gps: &HashMap<String, GpsStation>,
    sample_rate_hz: f64,
    output_path: &Path,
    revision: SegyRevision,
) -> Result<(), SegyError> {
    let first_device = devices.first()
        .ok_or_else(|| SegyError::InvalidFormat("No devices".into()))?;
    let first_path = minute_files.get(first_device)
        .ok_or_else(|| SegyError::InvalidFormat("No data for first device".into()))?;
    let first_data = MseismFile::from_file(first_path)?;
    let samples_per_trace = first_data.sample_count;

    let sample_interval_us = (1_000_000.0 / sample_rate_hz) as u16;

    let mut text_header = TextHeader::default();
    text_header.use_utf8 = true;
    text_header.set_line(1, "SEG-Y Rev 2.0 - Microseismic Data");
    text_header.set_line(2, &format!("Stations: {}", devices.len()));
    text_header.set_line(3, &format!("Sample rate: {:.2} Hz", sample_rate_hz));

    let mut binary_header = BinaryHeader::default().with_revision(revision);
    binary_header.samples_per_trace = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
    binary_header.samples_per_trace_original = if samples_per_trace <= 65535 { samples_per_trace as u16 } else { 0 };
    binary_header.sample_interval = sample_interval_us;
    binary_header.sample_interval_original = sample_interval_us;
    binary_header.traces_per_ensemble = devices.len() as u16;

    let mut writer = SegyWriter::create(output_path)?;
    writer.write_header(&text_header, &binary_header)?;

    let traces = read_slice_traces(minute_files, devices, gps, true, samples_per_trace)?;

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
