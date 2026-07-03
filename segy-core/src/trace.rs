use byteorder::{BigEndian, WriteBytesExt};
use chrono::{DateTime, Datelike, Timelike, Utc};
use std::io::Write;

use crate::header::TRACE_HEADER_SIZE;

#[derive(Debug, Clone, Default)]
pub struct TraceHeader {
    pub trace_sequence_line: i32,
    pub trace_sequence_file: i32,
    pub field_record_number: i32,
    pub trace_number_within_field: i32,
    pub energy_source_point: i32,
    pub ensemble_number: i32,
    pub trace_number_within_ensemble: i32,
    pub trace_id_code: u16,
    pub number_of_horizontally_summed: u16,
    pub number_of_vertically_summed: u16,
    pub data_use: u16,
    pub source_to_receiver_distance: i32,
    pub receiver_group_elevation: i32,
    pub surface_elevation_at_source: i32,
    pub source_depth_below_surface: i32,
    pub datum_elevation_at_receiver: i32,
    pub datum_elevation_at_source: i32,
    pub water_depth_at_source: i32,
    pub water_depth_at_group: i32,
    pub scalar_for_elevations: i16,
    pub scalar_for_coordinates: i16,
    pub source_x: i32,
    pub source_y: i32,
    pub group_x: i32,
    pub group_y: i32,
    pub coordinate_units: u16,
    pub weathering_velocity: u16,
    pub subweathering_velocity: u16,
    pub uphole_time_at_source: u16,
    pub uphole_time_at_group: u16,
    pub source_static_correction: u16,
    pub group_static_correction: u16,
    pub total_static_applied: u16,
    pub lag_time_a: u16,
    pub lag_time_b: u16,
    pub delay_recording_time: u16,
    pub mute_time_start: u16,
    pub mute_time_end: u16,
    pub number_of_samples: u16,
    pub sample_interval: u16,
    pub gain_type: u16,
    pub instrument_gain_constant: u16,
    pub instrument_initial_gain: u16,
    pub correlated: u16,
    pub sweep_freq_start: u16,
    pub sweep_freq_end: u16,
    pub sweep_length: u16,
    pub sweep_type: u16,
    pub sweep_taper_length_start: u16,
    pub sweep_taper_length_end: u16,
    pub taper_type: u16,
    pub alias_filter_freq: u16,
    pub alias_filter_slope: u16,
    pub notch_filter_freq: u16,
    pub notch_filter_slope: u16,
    pub low_cut_freq: u16,
    pub high_cut_freq: u16,
    pub low_cut_slope: u16,
    pub high_cut_slope: u16,
    pub year: u16,
    pub day_of_year: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub time_basis_code: u16,
    pub trace_weighting_factor: u16,
    pub group_number_of_roll_switch: u16,
    pub group_of_first_trace: u16,
    pub group_of_last_trace: u16,
    pub gap_size: u16,
    pub over_travel: u16,
}

impl TraceHeader {
    pub fn set_time(&mut self, dt: &DateTime<Utc>) {
        self.year = dt.year() as u16;
        self.day_of_year = dt.ordinal() as u16;
        self.hour = dt.hour() as u16;
        self.minute = dt.minute() as u16;
        self.second = dt.second() as u16;
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut buf = [0u8; TRACE_HEADER_SIZE];
        let mut cursor = std::io::Cursor::new(&mut buf[..]);

        cursor.write_i32::<BigEndian>(self.trace_sequence_line)?;
        cursor.write_i32::<BigEndian>(self.trace_sequence_file)?;
        cursor.write_i32::<BigEndian>(self.field_record_number)?;
        cursor.write_i32::<BigEndian>(self.trace_number_within_field)?;
        cursor.write_i32::<BigEndian>(self.energy_source_point)?;
        cursor.write_i32::<BigEndian>(self.ensemble_number)?;
        cursor.write_i32::<BigEndian>(self.trace_number_within_ensemble)?;
        cursor.write_u16::<BigEndian>(self.trace_id_code)?;
        cursor.write_u16::<BigEndian>(self.number_of_horizontally_summed)?;
        cursor.write_u16::<BigEndian>(self.number_of_vertically_summed)?;
        cursor.write_u16::<BigEndian>(self.data_use)?;
        cursor.write_i32::<BigEndian>(self.source_to_receiver_distance)?;
        cursor.write_i32::<BigEndian>(self.receiver_group_elevation)?;
        cursor.write_i32::<BigEndian>(self.surface_elevation_at_source)?;
        cursor.write_i32::<BigEndian>(self.source_depth_below_surface)?;
        cursor.write_i32::<BigEndian>(self.datum_elevation_at_receiver)?;
        cursor.write_i32::<BigEndian>(self.datum_elevation_at_source)?;
        cursor.write_i32::<BigEndian>(self.water_depth_at_source)?;
        cursor.write_i32::<BigEndian>(self.water_depth_at_group)?;
        cursor.write_i16::<BigEndian>(self.scalar_for_elevations)?;
        cursor.write_i16::<BigEndian>(self.scalar_for_coordinates)?;
        cursor.write_i32::<BigEndian>(self.source_x)?;
        cursor.write_i32::<BigEndian>(self.source_y)?;
        cursor.write_i32::<BigEndian>(self.group_x)?;
        cursor.write_i32::<BigEndian>(self.group_y)?;
        cursor.write_u16::<BigEndian>(self.coordinate_units)?;

        cursor.write_all(&[0u8; 48])?;

        cursor.write_u16::<BigEndian>(self.mute_time_start)?;
        cursor.write_u16::<BigEndian>(self.mute_time_end)?;
        cursor.write_u16::<BigEndian>(self.number_of_samples)?;
        cursor.write_u16::<BigEndian>(self.sample_interval)?;
        cursor.write_u16::<BigEndian>(self.gain_type)?;
        cursor.write_u16::<BigEndian>(self.instrument_gain_constant)?;
        cursor.write_u16::<BigEndian>(self.instrument_initial_gain)?;
        cursor.write_u16::<BigEndian>(self.correlated)?;
        cursor.write_u16::<BigEndian>(self.sweep_freq_start)?;
        cursor.write_u16::<BigEndian>(self.sweep_freq_end)?;
        cursor.write_u16::<BigEndian>(self.sweep_length)?;
        cursor.write_u16::<BigEndian>(self.sweep_type)?;
        cursor.write_u16::<BigEndian>(self.sweep_taper_length_start)?;
        cursor.write_u16::<BigEndian>(self.sweep_taper_length_end)?;
        cursor.write_u16::<BigEndian>(self.taper_type)?;
        cursor.write_u16::<BigEndian>(self.alias_filter_freq)?;
        cursor.write_u16::<BigEndian>(self.alias_filter_slope)?;
        cursor.write_u16::<BigEndian>(self.notch_filter_freq)?;
        cursor.write_u16::<BigEndian>(self.notch_filter_slope)?;
        cursor.write_u16::<BigEndian>(self.low_cut_freq)?;
        cursor.write_u16::<BigEndian>(self.high_cut_freq)?;
        cursor.write_u16::<BigEndian>(self.low_cut_slope)?;
        cursor.write_u16::<BigEndian>(self.high_cut_slope)?;
        cursor.write_u16::<BigEndian>(self.year)?;
        cursor.write_u16::<BigEndian>(self.day_of_year)?;
        cursor.write_u16::<BigEndian>(self.hour)?;
        cursor.write_u16::<BigEndian>(self.minute)?;
        cursor.write_u16::<BigEndian>(self.second)?;
        cursor.write_u16::<BigEndian>(self.time_basis_code)?;
        cursor.write_u16::<BigEndian>(self.trace_weighting_factor)?;
        cursor.write_u16::<BigEndian>(self.group_number_of_roll_switch)?;
        cursor.write_u16::<BigEndian>(self.group_of_first_trace)?;
        cursor.write_u16::<BigEndian>(self.group_of_last_trace)?;
        cursor.write_u16::<BigEndian>(self.gap_size)?;
        cursor.write_u16::<BigEndian>(self.over_travel)?;

        writer.write_all(&buf)?;
        Ok(())
    }
}
