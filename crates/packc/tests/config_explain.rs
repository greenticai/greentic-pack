#![forbid(unsafe_code)]

use serde_json::Value;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn config_explain_json_outputs_config_and_provenance() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_packc"));
    cmd.current_dir(temp.path()).arg("config").arg("--json");

    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "packc config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout)?;
    assert!(payload.get("config").is_some(), "config missing in output");
    assert!(
        payload.get("provenance").is_some(),
        "provenance missing in output"
    );

    Ok(())
}
