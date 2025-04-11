use std::io::Write;

use crate::options::{MachineOptions, OutputBufferingMode};

pub type WriterFactory = fn() -> Box<dyn Write>;

pub struct Output {
    factory: WriterFactory,
    quiet: bool,
    mode: OutputBufferingMode,
    hwm: usize,
    buffer: Vec<u8>,
}

impl Output {
    pub fn new(options: &MachineOptions, factory: WriterFactory) -> Self {
        Self {
            factory,
            quiet: options.is_quiet(),
            mode: options.output_buffering().mode(),
            hwm: options.output_buffering().size(),
            buffer: vec![],
        }
    }

    pub fn write(&mut self, value: &str) {
        if self.quiet {
            return;
        }

        self.buffer.extend(value.as_bytes());

        match self.mode {
            OutputBufferingMode::None => {
                if let Err(e) = self.flush() {
                    unreachable!("{:?}", e);
                }
            }
            OutputBufferingMode::Sized => {
                self.buffer.extend(value.as_bytes());
                if self.buffer.len() >= self.hwm {
                    if let Err(e) = self.flush() {
                        unreachable!("{:?}", e);
                    }
                }
            }
            OutputBufferingMode::NewLine => {
                self.buffer.extend(value.as_bytes());
                let mut last = 0;
                if self.buffer.contains(&10) {
                    for (idx, ch) in self.buffer.iter().enumerate() {
                        if *ch as char == '\n' {
                            last = idx;
                        }
                    }
                }

                if let Err(e) =
                    (self.factory)().write_all(&self.buffer.drain(0..last).collect::<Vec<u8>>())
                {
                    unreachable!("{:?}", e);
                }
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), std::io::Error> {
        return (self.factory)().write_all(self.buffer.drain(..).collect::<Vec<u8>>().as_ref());
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            if let Err(e) = self.flush() {
                dbg!(e);
            }
        }
    }
}
