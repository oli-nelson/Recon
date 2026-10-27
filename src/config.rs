use crate::{Config, Theme};
use anyhow::{Context, Result, bail};
use clap::{ArgMatches, CommandFactory, FromArgMatches, parser::ValueSource};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const DEFAULT_CONFIG: &str = include_str!("../config.default.toml");

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    fps: Option<u16>,
    theme: Option<Theme>,
    reduced_motion: Option<bool>,
    no_animations: Option<bool>,
    animation_ms: Option<u64>,
    sidebar_width: Option<u16>,
    ignore: Option<Vec<String>>,
    history_entries: Option<u32>,
    history_bytes: Option<u64>,
    max_text_bytes: Option<u64>,
    snapshot_bytes: Option<u64>,
    debounce_ms: Option<u64>,
    reconcile_seconds: Option<u64>,
}
impl Config {
    pub fn load() -> Result<Self> {
        let matches = Self::command().get_matches();
        let path = matches
            .get_one::<PathBuf>("config")
            .cloned()
            .map(Ok)
            .unwrap_or_else(default_path)?;
        Self::from_matches_and_file(&matches, &path)
    }
    /// Keep parsing deterministic for tests and embedded use: disk configuration
    /// is read explicitly at the executable boundary, not by clap's Parser.
    pub fn from_matches_and_file(matches: &ArgMatches, path: &Path) -> Result<Self> {
        ensure_default(path)?;
        let input = fs::read_to_string(path)
            .with_context(|| format!("Cannot read configuration {}", path.display()))?;
        let settings: Settings = toml::from_str(&input)
            .with_context(|| format!("Invalid configuration {}", path.display()))?;
        let mut config = Self::from_arg_matches(matches)?;
        macro_rules! apply {
            ($field:ident) => {
                if matches.value_source(stringify!($field)) != Some(ValueSource::CommandLine) {
                    if let Some(value) = settings.$field {
                        config.$field = value;
                    }
                }
            };
        }
        apply!(fps);
        apply!(theme);
        apply!(reduced_motion);
        apply!(no_animations);
        apply!(animation_ms);
        apply!(sidebar_width);
        apply!(history_entries);
        apply!(history_bytes);
        apply!(max_text_bytes);
        apply!(snapshot_bytes);
        apply!(debounce_ms);
        apply!(reconcile_seconds);
        if matches.value_source("ignores") != Some(ValueSource::CommandLine) {
            if let Some(ignore) = settings.ignore {
                config.ignores = ignore;
            }
        }
        config.config = Some(path.to_path_buf());
        config
            .validate()
            .with_context(|| format!("Invalid settings in {}", path.display()))?;
        Ok(config)
    }
    fn validate(&self) -> Result<()> {
        macro_rules! range {
            ($name:ident,$min:expr,$max:expr) => {
                if !($min..=$max).contains(&self.$name) {
                    bail!(
                        "{} must be between {} and {}",
                        stringify!($name),
                        $min,
                        $max
                    );
                }
            };
        }
        range!(fps, 1, 240);
        range!(animation_ms, 200, 2000);
        range!(sidebar_width, 20, 85);
        range!(history_entries, 1, u32::MAX);
        range!(history_bytes, 1024, usize::MAX as u64);
        range!(max_text_bytes, 1, usize::MAX as u64);
        range!(snapshot_bytes, 1024, usize::MAX as u64);
        range!(debounce_ms, 10, 2000);
        range!(reconcile_seconds, 1, u64::MAX);
        for pattern in &self.ignores {
            globset::Glob::new(pattern)
                .with_context(|| format!("Invalid ignore pattern: {pattern}"))?;
        }
        Ok(())
    }
}
pub fn default_path() -> Result<PathBuf> {
    platform_path(cfg!(windows), |key| std::env::var_os(key))
}
fn platform_path(windows: bool, env: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    let nonempty = |key| env(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if windows {
        nonempty("APPDATA")
            .or_else(|| nonempty("USERPROFILE").map(|p| p.join("AppData/Roaming")))
            .context(
                "APPDATA or USERPROFILE is required to find the Recon configuration directory",
            )?
    } else {
        nonempty("XDG_CONFIG_HOME").filter(|p|p.is_absolute())
            .or_else(||nonempty("HOME").map(|p|p.join(".config")))
            .context("HOME or an absolute XDG_CONFIG_HOME is required to find the Recon configuration directory")?
    };
    Ok(base.join("recon/config.toml"))
}
pub fn ensure_default(path: &Path) -> Result<()> {
    match fs::metadata(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Cannot inspect configuration {}", path.display()));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("Cannot create configuration directory {}", parent.display()))?;
    // Publish a complete file without replacing an existing config, even if two
    // instances start together. This also respects directory/file symlinks.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(DEFAULT_CONFIG.as_bytes())?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.error)
            .with_context(|| format!("Cannot create default configuration {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn load(path: &Path, args: &[&str]) -> Result<Config> {
        let matches = Config::command().try_get_matches_from(args)?;
        Config::from_matches_and_file(&matches, path)
    }
    #[test]
    fn creates_defaults_once_and_cli_wins_over_partial_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recon/config.toml");
        let default = load(&path, &["recon"]).unwrap();
        assert_eq!(default.fps, 60);
        assert_eq!(default.animation_ms, 800);
        fs::write(
            &path,
            "fps = 144\nsidebar_width = 55\nreduced_motion = true\nignore = [\"*.log\"]\n",
        )
        .unwrap();
        let config = load(&path, &["recon", "--fps", "120", "--ignore", "*.tmp"]).unwrap();
        assert_eq!(config.fps, 120);
        assert_eq!(config.sidebar_width, 55);
        assert!(config.reduced_motion);
        assert_eq!(config.ignores, ["*.tmp"]);
        assert!(fs::read_to_string(&path).unwrap().starts_with("fps = 144"));
    }

    #[test]
    fn explicit_boolean_flags_can_override_true_file_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "reduced_motion = true\nno_animations = true\n").unwrap();
        let config = load(
            &path,
            &["recon", "--reduced-motion=false", "--no-animations=false"],
        )
        .unwrap();
        assert!(!config.reduced_motion && !config.no_animations);
    }
    #[test]
    fn concurrent_first_startups_read_a_complete_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recon/config.toml");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    load(&path, &["recon"]).unwrap().fps
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), 60);
        }
    }
    #[cfg(unix)]
    #[test]
    fn defaults_are_created_through_a_dotfiles_directory_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles/recon");
        fs::create_dir_all(&target).unwrap();
        let link = dir.path().join("recon");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let config = load(&link.join("config.toml"), &["recon"]).unwrap();
        assert_eq!(config.fps, 60);
        assert_eq!(
            fs::read_to_string(target.join("config.toml")).unwrap(),
            DEFAULT_CONFIG
        );
    }
    #[test]
    fn invalid_and_unknown_settings_fail_without_overwriting_user_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for text in [
            "fps = 0",
            "animation_ms = 10000",
            "sidebar_width = 99",
            "unknown = true",
            "theme = \"neon\"",
            "ignore = [\"[\"]",
        ] {
            fs::write(&path, text).unwrap();
            assert!(load(&path, &["recon"]).is_err(), "{text}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }
    #[test]
    fn platform_locations_use_xdg_and_windows_roaming_appdata() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        let home = dir.path().join("home");
        let env = |key: &str| -> Option<OsString> {
            match key {
                "XDG_CONFIG_HOME" => Some(xdg.clone().into_os_string()),
                "HOME" => Some(home.clone().into_os_string()),
                "APPDATA" => Some("C:/Users/test/AppData/Roaming".into()),
                _ => None,
            }
        };
        assert_eq!(
            platform_path(false, env).unwrap(),
            xdg.join("recon/config.toml")
        );
        assert_eq!(
            platform_path(true, env).unwrap(),
            PathBuf::from("C:/Users/test/AppData/Roaming/recon/config.toml")
        );
        let relative = |key: &str| -> Option<OsString> {
            match key {
                "XDG_CONFIG_HOME" => Some("relative".into()),
                "HOME" => Some(home.clone().into_os_string()),
                _ => None,
            }
        };
        assert_eq!(
            platform_path(false, relative).unwrap(),
            home.join(".config/recon/config.toml")
        );
    }
}
