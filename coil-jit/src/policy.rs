use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitConfig {
    pub enabled: bool,
    pub function_threshold: u64,
}

impl Default for JitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            function_threshold: 10_000,
        }
    }
}

impl JitConfig {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

#[derive(Default)]
pub struct HotCounters {
    entries: HashMap<u32, u64>,
    compile_requested: HashMap<u32, bool>,
}

impl HotCounters {
    pub fn record_entry(&mut self, entry_pc: u32, config: &JitConfig) -> bool {
        if !config.enabled {
            return false;
        }
        let count = self.entries.entry(entry_pc).or_default();
        *count = count.saturating_add(1);
        if *count < config.function_threshold {
            return false;
        }
        if self.compile_requested.contains_key(&entry_pc) {
            return false;
        }
        self.compile_requested.insert(entry_pc, true);
        true
    }

    pub fn entry_count(&self, entry_pc: u32) -> u64 {
        self.entries.get(&entry_pc).copied().unwrap_or_default()
    }

    pub fn mark_compiled(&mut self, entry_pc: u32) {
        self.compile_requested.insert(entry_pc, false);
    }

    pub fn compile_requested(&self, entry_pc: u32) -> bool {
        self.compile_requested
            .get(&entry_pc)
            .copied()
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_compilation_once_at_threshold() {
        let config = JitConfig {
            function_threshold: 2,
            ..JitConfig::default()
        };
        let mut counters = HotCounters::default();
        assert!(!counters.record_entry(7, &config));
        assert!(counters.record_entry(7, &config));
        assert!(!counters.record_entry(7, &config));
        assert_eq!(counters.entry_count(7), 3);
        assert!(counters.compile_requested(7));
    }

    #[test]
    fn mark_compiled_clears_pending_request() {
        let config = JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        };
        let mut counters = HotCounters::default();
        assert!(counters.record_entry(3, &config));
        counters.mark_compiled(3);
        assert!(!counters.compile_requested(3));
    }

    #[test]
    fn disabled_policy_does_not_count_entries() {
        let config = JitConfig::disabled();
        let mut counters = HotCounters::default();
        assert!(!counters.record_entry(11, &config));
        assert_eq!(counters.entry_count(11), 0);
    }

    #[test]
    fn compiled_entry_is_not_requested_again() {
        let config = JitConfig {
            function_threshold: 1,
            ..JitConfig::default()
        };
        let mut counters = HotCounters::default();
        assert!(counters.record_entry(5, &config));
        counters.mark_compiled(5);
        assert!(!counters.record_entry(5, &config));
        assert!(!counters.compile_requested(5));
        assert_eq!(counters.entry_count(5), 2);
    }
}
