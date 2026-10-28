//! Focused synthetic probes: cargo run --release --locked --example performance_review
use clap::Parser;
use ratatui::{Terminal, backend::TestBackend};
use recon::{
    Config,
    engine::Engine,
    model::{FileRow, Model, Status, Update},
    ui::{View, draw},
};
use std::{collections::BTreeSet, fs, time::Instant};

fn main() -> anyhow::Result<()> {
    let config = Config::parse_from(["recon", "--no-animations"]);
    for count in [1_000, 10_000, 50_000, 100_000] {
        let rows: Vec<_> = (0..count)
            .map(|i| {
                (
                    FileRow {
                        path: format!("dir/file{i:06}.txt"),
                        status: Status::Modified,
                        git: " M".into(),
                        note: String::new(),
                        activity: Instant::now(),
                        remove_from_tree: false,
                        modified_at: None,
                    },
                    None,
                )
            })
            .collect();
        let mut model = Model::new(1000, 64 * 1024 * 1024);
        let start = Instant::now();
        for batch in rows.chunks(64) {
            model.apply(Update::Files(batch.to_vec()));
        }
        let load_ms = start.elapsed().as_secs_f64() * 1000.;
        model.apply(Update::Done);
        let mut terminal = Terminal::new(TestBackend::new(160, 48))?;
        let mut view = View::new();
        let start = Instant::now();
        terminal.draw(|f| draw(f, &model, &mut view, &config))?;
        let cold_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut draws = Vec::new();
        for i in 0..21 {
            // Exercise selection changes without invalidating structural rows.
            view.selected = Some(format!("dir/file{:06}.txt", i * (count / 21)));
            let start = Instant::now();
            terminal.draw(|f| draw(f, &model, &mut view, &config))?;
            draws.push(start.elapsed().as_secs_f64() * 1000.);
        }
        draws.sort_by(f64::total_cmp);
        println!(
            "rows={count} load_ms={load_ms:.3} cold_draw_ms={cold_ms:.3} cached_draw_median_ms={:.3}",
            draws[10]
        );
    }
    let dir = tempfile::tempdir()?;
    let repo = git2::Repository::init(dir.path())?;
    let path = dir.path().join("large.bin");
    let mut bytes = vec![b'x'; 32 * 1024 * 1024];
    fs::write(&path, &bytes)?;
    let mut index = repo.index()?;
    index.add_path(std::path::Path::new("large.bin"))?;
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = git2::Signature::now("Benchmark", "benchmark@example.invalid")?;
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])?;
    bytes[0] = b'y';
    fs::write(&path, bytes)?;
    let config = Config::parse_from(["recon", dir.path().to_str().unwrap()]);
    let mut engine = Engine::new(&config)?;
    engine.reset(false, "", &mut |_| {})?;
    for full in [false, true] {
        let start = Instant::now();
        for _ in 0..10 {
            let paths = (!full).then(|| BTreeSet::from(["large.bin".into()]));
            engine.refresh(paths, false, &mut |_| {})?;
        }
        println!(
            "large_32mib full={full} refresh_mean_ms={:.3}",
            start.elapsed().as_secs_f64() * 100.
        );
    }
    Ok(())
}
