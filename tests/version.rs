use std::process::Command;

#[test]
fn version_matches_package_without_loading_configuration_or_repository() {
    let directory = tempfile::tempdir().unwrap();
    let missing_config = directory.path().join("missing/config.toml");
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_recon"))
            .current_dir(directory.path())
            .args(["--config", missing_config.to_str().unwrap(), flag])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("recon {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty());
        assert!(!missing_config.exists());
    }
}
