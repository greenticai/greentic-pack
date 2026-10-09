//! Shared `greentic-flow doctor --json --stdin` invocation.
//!
//! Both the canonical pack path (which iterates `manifest.flows`) and the DW
//! application pack path (which has exactly one known flow entry,
//! `flows/main.ygtc`) need to hand a flow's bytes to `greentic-flow` and read
//! back a verdict. They differ only in where the flow list comes from, so the
//! spawn — including the "greentic-flow is not installed" and "this
//! greentic-flow predates `--stdin`" fallbacks — lives here once.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::{io, process::Output};

use anyhow::{Context, Result};
use serde_json::Value;

/// What `greentic-flow doctor` had to say about one flow.
pub(crate) enum FlowDoctorOutcome {
    /// The flow passed.
    Ok,
    /// The flow failed; `data` carries the tool's stdout/stderr for the report.
    Failed { data: Value },
    /// No verdict was obtained. Callers report this as a warning and stop
    /// asking — the cause applies to every flow, not just this one.
    Unavailable {
        message: &'static str,
        hint: &'static str,
        data: Value,
    },
}

/// Run `greentic-flow doctor --json --stdin` over one flow's bytes.
///
/// Returns `Err` only for genuinely unexpected IO failures. A missing
/// `greentic-flow` binary, or one too old to understand `--stdin`, is an
/// [`FlowDoctorOutcome::Unavailable`] — not every environment installs it, and
/// its absence must never fail a pack.
pub(crate) fn run_flow_doctor(bytes: &[u8]) -> Result<FlowDoctorOutcome> {
    let flow_bin = crate::external_tools::resolve("greentic-flow")
        .unwrap_or_else(|| PathBuf::from("greentic-flow"));
    let mut command = Command::new(&flow_bin);
    command
        .args(["doctor", "--json", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(FlowDoctorOutcome::Unavailable {
                message: "greentic-flow not available; skipping flow doctor checks",
                hint: "install greentic-flow or pass --no-flow-doctor",
                data: Value::Null,
            });
        }
        Err(err) => {
            return Err(err).with_context(|| format!("run {} doctor", flow_bin.display()));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .context("write flow content to greentic-flow stdin")?;
    }
    let output = child
        .wait_with_output()
        .context("wait for greentic-flow doctor")?;

    if output.status.success() {
        return Ok(FlowDoctorOutcome::Ok);
    }
    if flow_doctor_unsupported(&output) {
        return Ok(FlowDoctorOutcome::Unavailable {
            message: "greentic-flow does not support --stdin; skipping flow doctor checks",
            hint: "update greentic-flow or pass --no-flow-doctor",
            data: json_diagnostic_data(&output),
        });
    }
    Ok(FlowDoctorOutcome::Failed {
        data: json_diagnostic_data(&output),
    })
}

pub(crate) fn flow_doctor_unsupported(output: &Output) -> bool {
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let combined = combined.to_lowercase();
    combined.contains("--stdin") && combined.contains("unknown")
        || combined.contains("found argument '--stdin'")
        || combined.contains("unexpected argument '--stdin'")
        || combined.contains("unrecognized option '--stdin'")
}

pub(crate) fn json_diagnostic_data(output: &Output) -> Value {
    serde_json::json!({
        "status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout).trim_end(),
        "stderr": String::from_utf8_lossy(&output.stderr).trim_end(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn flow_doctor_unsupported_detects_common_cli_errors() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: b"error: unexpected argument '--stdin' found".to_vec(),
        };

        assert!(flow_doctor_unsupported(&output));
    }

    #[test]
    fn flow_doctor_unsupported_ignores_ordinary_failures() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: b"error: flow node `foo` has no handler".to_vec(),
        };

        assert!(!flow_doctor_unsupported(&output));
    }
}
