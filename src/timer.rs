extern crate time;

/// A struct that uses RAII to log durations: when dropped, it logs the number of milliseconds it
/// existed, prefixed by `name`.
pub struct Timer {
    name: String,
    start_time_ns: u64
}

impl Timer {
    pub fn new(name: String) -> Timer {
        Timer {
            name: name,
            start_time_ns: time::precise_time_ns(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        info!("{}: {} ms", self.name, (time::precise_time_ns() - self.start_time_ns) / 1_000_000);
    }
}
