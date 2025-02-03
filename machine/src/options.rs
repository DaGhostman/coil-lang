use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct MemoryOptions {
    limit: usize,
}

impl MemoryOptions {
    pub fn limit(&self) -> usize {
        self.limit * 1024
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize)]
pub enum OutputBufferingMode {
    None,
    #[default]
    NewLine,
    Sized,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct OutputBuffering {
    mode: OutputBufferingMode,
    size: usize,
}

impl OutputBuffering {
    pub fn mode(&self) -> OutputBufferingMode {
        self.mode
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

#[derive(Debug, Deserialize)]
pub struct MachineOptions {
    debug: bool,
    quiet: bool,

    memory: MemoryOptions,
    buffering: OutputBuffering,
}

impl MachineOptions {
    pub fn set_quiet(&mut self, state: bool) {
        self.quiet = state;
    }

    pub fn set_debug(&mut self, state: bool) {
        self.debug = state;
    }

    pub fn set_output_buffering(&mut self, buffering: OutputBuffering) {
        self.buffering = buffering;
    }

    pub fn is_debug(&self) -> bool {
        self.debug
    }

    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    pub fn output_buffering(&self) -> OutputBuffering {
        self.buffering
    }

    pub fn memory(&self) -> MemoryOptions {
        self.memory
    }
}

impl Default for MachineOptions {
    fn default() -> Self {
        Self {
            debug: true,
            quiet: false,

            buffering: OutputBuffering {
                mode: OutputBufferingMode::None,
                size: 0,
            },
            memory: MemoryOptions { limit: 1024 },
        }
    }
}
