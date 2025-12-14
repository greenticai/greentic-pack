#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Clone, Parser)]
pub struct ConfigArgs {}

pub fn handle(
    _args: ConfigArgs,
    json: bool,
    runtime: &crate::runtime::RuntimeContext,
) -> Result<()> {
    let report = runtime.resolved.explain();
    if json {
        println!("{}", serde_json::to_string_pretty(&report.as_json())?);
    } else {
        println!("{report}");
    }
    Ok(())
}
