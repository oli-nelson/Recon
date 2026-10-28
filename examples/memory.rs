//! Stress retained snapshot text and history under their default budgets.
use clap::Parser;
use recon::{Config, engine::Engine, model::Model};
fn main() {
    let path = std::env::args().nth(1).expect("fixture path");
    let config = Config::parse_from(["recon", &path]);
    let mut engine = Engine::new(&config).unwrap();
    let mut model = Model::new(
        config.history_entries as usize,
        config.history_bytes as usize,
    );
    let start = std::time::Instant::now();
    engine
        .reset(false, "memory stress", &mut |u| model.apply(u))
        .unwrap();
    assert_eq!(model.files.len(), 300);
    assert!(model.bytes <= config.history_bytes as usize);
    #[cfg(not(unix))]
    println!(
        "memory_stress_files={} elapsed_ms={} retained_history_bytes={} evicted_entries={} (peak RSS measurement and 512 MiB budget check unavailable on this platform)",
        model.files.len(),
        start.elapsed().as_millis(),
        model.bytes,
        model.evicted
    );
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage writes a valid rusage to the supplied pointer on success.
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        let usage = unsafe { usage.assume_init() };
        let bytes = if cfg!(target_os = "macos") {
            usage.ru_maxrss as u64
        } else {
            usage.ru_maxrss as u64 * 1024
        };
        println!(
            "memory_stress_files={} elapsed_ms={} peak_rss_mib={:.2} retained_history_bytes={} evicted_entries={}",
            model.files.len(),
            start.elapsed().as_millis(),
            bytes as f64 / 1048576.,
            model.bytes,
            model.evicted
        );
        assert!(
            bytes < 512 * 1024 * 1024,
            "exceeded reference-fixture 512 MiB memory budget"
        );
    }
}
