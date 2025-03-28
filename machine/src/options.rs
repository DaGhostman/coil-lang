use serde::Deserialize;

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

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct GcOptions {
    growth: usize,
    threshold: usize,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            growth: 2,
            threshold: 4096,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct StackOptions {
    size: usize,
}

impl Default for StackOptions {
    fn default() -> Self {
        Self { size: 16 }
    }
}

#[derive(Debug, Deserialize)]
pub struct MachineOptions {
    debug: bool,
    quiet: bool,

    buffering: OutputBuffering,

    gc: GcOptions,
    stack: StackOptions,
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

    pub fn gc_growth(&self) -> usize {
        self.gc.growth
    }

    pub fn gc_threshold(&self) -> usize {
        self.gc.threshold
    }

    pub fn stack_size(&self) -> usize {
        self.stack.size
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
            gc: GcOptions::default(),
            stack: StackOptions::default(),
        }
    }
}
