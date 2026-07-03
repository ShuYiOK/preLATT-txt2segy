use segy_core::MseismFile;
use std::path::Path;

fn main() {
    let path = Path::new(r"Y:\115open\Mseism\microseismic\24002\2026-01-25_00_00-24002.txt");
    println!("Testing file: {:?}", path);
    match MseismFile::from_file(path) {
        Ok(f) => {
            println!("SUCCESS!");
            println!("  Device: {}", f.device_id);
            println!("  Time: {}", f.start_time);
            println!("  Status: {}", f.status_code);
            println!("  X: {:.5}, Y: {:.5}, Elev: {:.2}", f.x, f.y, f.elevation);
            println!("  Samples: {}", f.sample_count);
            println!("  Sample rate: {:.2} Hz", f.sample_count as f64 / 60.0);
            if f.sample_count > 5 {
                println!("  First 5 samples: {:?}", &f.samples[..5]);
            }
        }
        Err(e) => {
            println!("FAILED: {:?}", e);
        }
    }
}
