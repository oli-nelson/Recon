use anyhow::Result;
use recon::{Config, engine::Engine, model::Model, ui, watch::Worker};

fn main() -> Result<()> {
    let config = Config::load()?;
    if config.once {
        let mut engine = Engine::new(&config)?;
        let mut model = Model::new(
            config.history_entries as usize,
            config.history_bytes as usize,
        );
        engine.reset(false, "Opening snapshot", &mut |u| model.apply(u))?;
        println!(
            "RECON • branch {} • baseline {} • {} files",
            model.branch,
            model.baseline,
            model.files.len()
        );
        println!("Root: {}", model.root.display());
        for row in model.files.values() {
            println!(
                "{} [{}] {} {}",
                row.status.symbol(),
                row.git,
                row.path,
                row.note
            );
        }
        for entry in model.entries.iter().rev() {
            println!("\n{} • {}\n{}", entry.path, entry.activity, entry.diff);
        }
        if model.evicted > 0 {
            println!("{} oldest entries evicted", model.evicted);
        }
        return Ok(());
    }
    let worker = Worker::spawn(config.clone())?;
    ui::run(&config, worker)
}
