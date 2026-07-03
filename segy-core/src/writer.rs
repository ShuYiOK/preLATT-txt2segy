use byteorder::{BigEndian, WriteBytesExt};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::header::{BinaryHeader, TextHeader};
use crate::trace::TraceHeader;
use crate::SegyError;

pub struct SegyWriter<W: Write> {
    writer: BufWriter<W>,
    traces_written: u64,
    samples_per_trace: u32,
    header_written: bool,
}

impl SegyWriter<File> {
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self, SegyError> {
        let file = File::create(path.as_ref())?;
        let writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
        Ok(Self {
            writer,
            traces_written: 0,
            samples_per_trace: 0,
            header_written: false,
        })
    }
}

impl<W: Write> SegyWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
            traces_written: 0,
            samples_per_trace: 0,
            header_written: false,
        }
    }

    pub fn write_header(
        &mut self,
        text_header: &TextHeader,
        binary_header: &BinaryHeader,
    ) -> Result<(), SegyError> {
        text_header.write_to(&mut self.writer)?;
        binary_header.write_to(&mut self.writer)?;
        let sp = binary_header.samples_per_trace as u32;
        if sp > 0 && sp < 65535 {
            self.samples_per_trace = sp;
        }
        self.header_written = true;
        Ok(())
    }

    pub fn write_trace(&mut self, header: &TraceHeader, samples: &[f32]) -> Result<(), SegyError> {
        if !self.header_written {
            return Err(SegyError::InvalidFormat(
                "Must write header before traces".into(),
            ));
        }
        if self.samples_per_trace == 0 {
            self.samples_per_trace = samples.len() as u32;
        }
        if samples.len() as u32 != self.samples_per_trace {
            return Err(SegyError::DataMismatch(format!(
                "Sample count mismatch: expected {}, got {}",
                self.samples_per_trace,
                samples.len()
            )));
        }

        header.write_to(&mut self.writer)?;

        for &s in samples {
            self.writer.write_f32::<BigEndian>(s)?;
        }

        self.traces_written += 1;
        Ok(())
    }

    pub fn write_trace_raw_f32(&mut self, header: &TraceHeader, samples: &[f32]) -> Result<(), SegyError> {
        self.write_trace(header, samples)
    }

    pub fn traces_written(&self) -> u64 {
        self.traces_written
    }

    pub fn finish(mut self) -> Result<W, SegyError> {
        self.writer.flush()?;
        Ok(self.writer.into_inner().map_err(|e| std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))?)
    }

    pub fn flush(&mut self) -> Result<(), SegyError> {
        self.writer.flush()?;
        Ok(())
    }
}
