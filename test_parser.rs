use std::path::Path;
use segy_core::MseismFile;

fn main() {
    let path = Path::new(r"Y:\115open\Mseism\microseismic\24002\2026-01-25_00_00-24002.txt");
    match MseismFile::from_file(path) {
        Ok(f) => {
            println!("Device: {}", f.device_id);
            println!("Time: {}", f.start_time);
            println!("Status: {}", f.status_code);
            println!("X: {}, Y: {}, Elev: {}", f.x, f.y, f.elevation);
            println!("Samples: {}", f.sample_count);
            println!("Sample rate: {:.2} Hz", f.sample_count as f64 / 60.0);
            println!("First 5 samples: {:?}", &f.samples[..5]);
        }
        Err(e) => {
            println!("Error: {:?}", e);
        }
    }
}
