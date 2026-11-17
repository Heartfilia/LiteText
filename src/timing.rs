//! Opt-in phase timings; never log document paths or contents.
pub(crate) struct Phase {
    name: &'static str,
    started: std::time::Instant,
}

pub(crate) fn phase(name: &'static str) -> Phase {
    Phase {
        name,
        started: std::time::Instant::now(),
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ENABLED.get_or_init(|| std::env::var_os("LITETEXT_TIMINGS").is_some()) {
            eprintln!(
                "LiteText phase={} elapsed_ms={:.3}",
                self.name,
                self.started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
