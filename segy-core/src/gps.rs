use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::SegyError;

#[derive(Debug, Clone)]
pub struct GpsStation {
    pub device_id: String,
    pub longitude: f64,
    pub latitude: f64,
    pub elevation: f64,
    pub utm_x: f64,
    pub utm_y: f64,
}

pub fn parse_gps_file<P: AsRef<Path>>(path: P) -> Result<Vec<GpsStation>, SegyError> {
    let file = File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    let mut stations = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(SegyError::Parse(format!(
                "GPS line {} has {} fields, expected at least 6",
                line_num + 1,
                fields.len()
            )));
        }

        let device_id = fields[0].to_string();
        let longitude: f64 = fields[1]
            .parse()
            .map_err(|e| SegyError::Parse(format!("GPS line {}: invalid longitude: {}", line_num + 1, e)))?;
        let latitude: f64 = fields[2]
            .parse()
            .map_err(|e| SegyError::Parse(format!("GPS line {}: invalid latitude: {}", line_num + 1, e)))?;
        let elevation: f64 = fields[3]
            .parse()
            .map_err(|e| SegyError::Parse(format!("GPS line {}: invalid elevation: {}", line_num + 1, e)))?;
        let utm_x: f64 = fields[4]
            .parse()
            .map_err(|e| SegyError::Parse(format!("GPS line {}: invalid utm_x: {}", line_num + 1, e)))?;
        let utm_y: f64 = fields[5]
            .parse()
            .map_err(|e| SegyError::Parse(format!("GPS line {}: invalid utm_y: {}", line_num + 1, e)))?;

        stations.push(GpsStation {
            device_id,
            longitude,
            latitude,
            elevation,
            utm_x,
            utm_y,
        });
    }

    Ok(stations)
}

pub fn gps_to_map(stations: Vec<GpsStation>) -> HashMap<String, GpsStation> {
    stations.into_iter().map(|s| (s.device_id.clone(), s)).collect()
}
