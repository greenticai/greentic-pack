#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use greentic_pack::pack_lock::read_pack_lock;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Args)]
pub struct InspectLockArgs {
    /// Pack root directory containing pack.yaml.
    #[arg(long = "in", value_name = "DIR", default_value = ".")]
    pub input: PathBuf,

    /// Path to pack.lock.cbor (default: pack.lock.cbor under pack root).
    #[arg(long = "lock", value_name = "FILE")]
    pub lock: Option<PathBuf>,
}

pub fn handle(args: InspectLockArgs) -> Result<()> {
    let pack_dir = args
        .input
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", args.input.display()))?;
    let lock_path = resolve_lock_path(&pack_dir, args.lock.as_deref());
    let lock = read_pack_lock(&lock_path)
        .with_context(|| format!("failed to read {}", lock_path.display()))?;

    let json = to_sorted_json(&lock)?;
    println!("{json}");
    Ok(())
}

fn resolve_lock_path(pack_dir: &Path, override_path: Option<&Path>) -> PathBuf {
    match override_path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => pack_dir.join(path),
        None => pack_dir.join("pack.lock.cbor"),
    }
}

fn to_sorted_json<T: Serialize>(value: &T) -> Result<String> {
    let value = serde_json::to_value(value).context("encode lock to json")?;
    let sorted = sort_json(value);
    serde_json::to_string_pretty(&sorted).context("serialize json")
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, sort_json(value));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json).collect()),
        other => other,
    }
}
