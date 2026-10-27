//! Worker-to-model and terminal-render measurements; fixture setup is external.
use clap::Parser;
use ratatui::{Terminal, backend::TestBackend};
use recon::{
    Config,
    model::{Model, Update},
    ui::{View, draw},
    watch::Worker,
};
use std::{
    collections::HashSet,
    fs,
    time::{Duration, Instant},
};

fn until(worker: &Worker, model: &mut Model, mut done: impl FnMut(&Model, &Update) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "benchmark refresh timed out");
        if let Ok(update) = worker.updates.recv_timeout(Duration::from_millis(100)) {
            if let Update::WatchStatus(message) = &update {
                panic!("Native benchmark unavailable: {message}");
            }
            let finished = done(model, &update);
            model.apply(update);
            if finished {
                return;
            }
        }
    }
}
fn main() {
    let path = std::env::args().nth(1).expect("fixture path");
    let burst: usize = std::env::args()
        .nth(2)
        .unwrap_or("2000".into())
        .parse()
        .unwrap();
    let config = Config::parse_from([
        "recon",
        &path,
        "--reconcile-seconds",
        "300",
        "--no-animations",
    ]);
    let mut model = Model::new(
        config.history_entries as usize,
        config.history_bytes as usize,
    );
    let start = Instant::now();
    let worker = Worker::spawn(config.clone()).unwrap();
    until(&worker, &mut model, |_, u| matches!(u, Update::Done));
    println!(
        "startup_ms={} files={} retained_entries={} history_bytes={}",
        start.elapsed().as_millis(),
        model.files.len(),
        model.entries.len(),
        model.bytes
    );
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    let mut view = View::new();
    let mut render = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        render.push(t.elapsed().as_micros());
    }
    render.sort();
    println!("render_p95_us={}", render[18]);
    let mut latency = Vec::new();
    for i in 0..20 {
        let mut text = "original text!!\n".repeat(65536);
        text.replace_range(0..8, &format!("edit{i:04}"));
        let start = Instant::now();
        fs::write(format!("{path}/normal.txt"), text).unwrap();
        until(
            &worker,
            &mut model,
            |_, u| matches!(u, Update::Files(rows) if rows.iter().any(|(r,e)| r.path == "normal.txt" && e.is_some())),
        );
        latency.push(start.elapsed().as_millis());
        until(&worker, &mut model, |_, u| matches!(u, Update::Done));
    }
    latency.sort();
    println!("edit_1mib_p95_ms={} max_ms={}", latency[18], latency[19]);
    // Drain while writing so the bounded queue does not add artificial benchmark backpressure.
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        for i in 0..burst {
            fs::write(
                format!("{writer_path}/d{:04}/f{i:06}.txt", i / 1000),
                format!("burst final {i}\n"),
            )
            .unwrap();
        }
        Instant::now()
    });
    let mut seen = HashSet::new();
    let start = Instant::now();
    until(&worker, &mut model, |_, u| {
        if let Update::Files(rows) = u {
            for (r, e) in rows {
                if e.as_ref().is_some_and(|e| e.diff.contains("+burst final")) {
                    seen.insert(r.path.clone());
                }
            }
        }
        seen.len() == burst
    });
    let finished = Instant::now();
    let writes_done = writer.join().unwrap();
    println!(
        "burst_files={} write_and_converge_ms={} after_final_write_ms={} retained_history_bytes={}",
        seen.len(),
        finished.duration_since(start).as_millis(),
        finished.saturating_duration_since(writes_done).as_millis(),
        model.bytes
    );
    // In-process resource accounting also works when sandbox policy blocks ps.
    fn resources() -> (f64, u64) {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage initializes the supplied writable rusage on success.
        let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        assert_eq!(result, 0);
        let usage = unsafe { usage.assume_init() };
        let cpu = usage.ru_utime.tv_sec as f64
            + usage.ru_utime.tv_usec as f64 / 1e6
            + usage.ru_stime.tv_sec as f64
            + usage.ru_stime.tv_usec as f64 / 1e6;
        let rss = usage.ru_maxrss as u64;
        #[cfg(target_os = "macos")]
        let bytes = rss;
        #[cfg(not(target_os = "macos"))]
        let bytes = rss * 1024;
        (cpu, bytes)
    }
    until(&worker, &mut model, |_, u| matches!(u, Update::Done));
    std::thread::sleep(Duration::from_millis(500));
    let before = resources();
    std::thread::sleep(Duration::from_secs(5));
    let after = resources();
    println!(
        "idle_worker_cpu_percent={:.2} peak_rss_mib={:.2}",
        (after.0 - before.0) / 5. * 100.,
        after.1 as f64 / 1048576.
    );
    worker.stop();
}
