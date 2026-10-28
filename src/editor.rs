use anyhow::{Context, Result, bail};
use std::{ffi::OsString, path::Path, process::Command};

pub(crate) fn command(root: &Path, path: &Path) -> Result<Command> {
    prepare(root, path, |key| std::env::var_os(key), cfg!(windows))
}

fn prepare(
    root: &Path,
    path: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Result<Command> {
    if !path.is_file() {
        bail!(
            "Cannot open {}: file is missing or is not a regular file",
            path.display()
        );
    }
    let value = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(env)
        .find(|value| !value.to_string_lossy().trim().is_empty())
        .unwrap_or_else(|| if windows { "notepad" } else { "vi" }.into());
    // Also accept an unquoted executable path containing spaces. Arguments use
    // shell-style quoting, but are passed directly, without shell evaluation.
    let words: Vec<OsString> = if Path::new(&value).is_file() {
        vec![value]
    } else {
        let value = value
            .to_str()
            .context("Editor command is not valid UTF-8")?;
        shlex::split(value)
            .context("Invalid quoting in VISUAL/EDITOR")?
            .into_iter()
            .map(OsString::from)
            .collect()
    };
    let (program, args) = words.split_first().context("Editor command is empty")?;
    if program.is_empty() {
        bail!("Editor executable is empty");
    }
    let mut command = Command::new(program);
    command.args(args).arg(path).current_dir(root);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_precedence_arguments_and_literal_file_paths() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file with spaces;$(echo bad).txt");
        std::fs::write(&file, "content").unwrap();
        let env = |key: &str| match key {
            "VISUAL" => Some("code --wait 'two words'".into()),
            "EDITOR" => Some("vim".into()),
            _ => None,
        };
        let command = prepare(root.path(), &file, env, false).unwrap();
        assert_eq!(command.get_program(), "code");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--wait".as_ref(), "two words".as_ref(), file.as_os_str()]
        );
        assert_eq!(command.get_current_dir(), Some(root.path()));
        let command = prepare(
            root.path(),
            &file,
            |key| match key {
                "VISUAL" => Some("  ".into()),
                "EDITOR" => Some("nvim".into()),
                _ => None,
            },
            false,
        )
        .unwrap();
        assert_eq!(command.get_program(), "nvim");
        for (windows, fallback) in [(false, "vi"), (true, "notepad")] {
            assert_eq!(
                prepare(root.path(), &file, |_| None, windows)
                    .unwrap()
                    .get_program(),
                fallback
            );
        }
    }

    #[test]
    fn editor_paths_with_spaces_and_windows_quotes() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("my editor");
        std::fs::write(&executable, "placeholder").unwrap();
        let command = prepare(
            root.path(),
            &executable,
            |_| Some(executable.clone().into_os_string()),
            false,
        )
        .unwrap();
        assert_eq!(command.get_program(), executable.as_os_str());
        let command = prepare(
            root.path(),
            &executable,
            |_| Some(r#""C:\Program Files\Editor\editor.exe" --wait"#.into()),
            true,
        )
        .unwrap();
        assert_eq!(command.get_program(), r"C:\Program Files\Editor\editor.exe");
        assert_eq!(command.get_args().next().unwrap(), "--wait");
        assert!(
            prepare(
                root.path(),
                &executable,
                |_| Some("'unclosed".into()),
                false
            )
            .is_err()
        );
        assert!(prepare(root.path(), &root.path().join("missing"), |_| None, false).is_err());
    }
}
