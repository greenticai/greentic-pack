#![forbid(unsafe_code)]

use anyhow::Result;
use clap::Subcommand;

pub mod loveable_convert;

#[derive(Debug, Subcommand)]
pub enum GuiCommand {
    /// Convert a Loveable-generated repo or build output into a GUI .gtpack
    #[command(name = "loveable-convert")]
    LoveableConvert(loveable_convert::Args),
}

pub async fn handle(
    cmd: GuiCommand,
    json: bool,
    runtime: &crate::runtime::RuntimeContext,
) -> Result<()> {
    match cmd {
        GuiCommand::LoveableConvert(args) => loveable_convert::handle(args, json, runtime).await,
    }
}
