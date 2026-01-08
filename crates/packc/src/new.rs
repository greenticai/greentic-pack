#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use clap::Parser;
use greentic_distributor_client::{DistClient, DistOptions};
use std::fs;
use std::path::{Path, PathBuf};

use crate::runtime::{NetworkPolicy, RuntimeContext};

#[derive(Debug, Parser)]
pub struct NewArgs {
    /// Directory to create the pack in
    #[arg(long = "dir", value_name = "DIR")]
    pub dir: PathBuf,
    /// Pack id to use
    #[arg(value_name = "PACK_ID")]
    pub pack_id: String,
}

const TEMPLATE_REF: &str = "oci://ghcr.io/greentic-ai/components/templates:latest";
const PLACEHOLDER_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

pub async fn handle(args: NewArgs, json: bool, runtime: &RuntimeContext) -> Result<()> {
    let root = args.dir.canonicalize().unwrap_or_else(|_| args.dir.clone());
    fs::create_dir_all(&root)?;

    write_pack_yaml(&root, &args.pack_id)?;
    write_flow(&root)?;
    let resolved_digest = resolve_template_digest(runtime).await;
    write_flow_sidecar(&root, &resolved_digest)?;
    create_components_dir(&root)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "pack_dir": root,
            }))?
        );
    } else {
        println!("created pack at {}", root.display());
    }

    Ok(())
}

fn write_pack_yaml(root: &Path, pack_id: &str) -> Result<()> {
    let pack_yaml = format!(
        r#"pack_id: {pack_id}
version: 0.1.0
kind: application
publisher: Greentic

components: []

flows:
  - id: main
    file: flows/main.ygtc
    tags: [default]
    entrypoints: [default]

dependencies: []

assets: []

extensions:
  greentic.components:
    kind: greentic.components
    version: v1
    inline:
      refs:
        - "oci://ghcr.io/greentic-ai/components/templates:latest"
      mode: eager
      allow_tags: true
"#
    );
    let path = root.join("pack.yaml");
    fs::write(&path, pack_yaml).with_context(|| format!("failed to write {}", path.display()))
}

fn write_flow(root: &Path) -> Result<()> {
    let flows_dir = root.join("flows");
    fs::create_dir_all(&flows_dir)?;
    let flow_path = flows_dir.join("main.ygtc");
    const FLOW: &str = r#"id: main
title: Welcome
description: Minimal starter flow
type: messaging
start: start

nodes:
  start:
    templating.handlebars:
      text: "Hello from greentic-pack starter!"
    routing: out
"#;
    fs::write(&flow_path, FLOW).with_context(|| format!("failed to write {}", flow_path.display()))
}

fn write_flow_sidecar(root: &Path, digest: &str) -> Result<()> {
    let sidecar_path = root.join("flows/main.ygtc.resolve.json");
    let sidecar = serde_json::json!({
        "schema_version": 1,
        "flow": "flows/main.ygtc",
        "nodes": {
            "start": {
                "source": {
                    "kind": "oci",
                    "ref": TEMPLATE_REF,
                    "digest": digest,
                }
            }
        }
    });
    let rendered = serde_json::to_string_pretty(&sidecar)?;
    fs::write(&sidecar_path, rendered)
        .with_context(|| format!("failed to write {}", sidecar_path.display()))
}

fn create_components_dir(root: &Path) -> Result<()> {
    let components_dir = root.join("components");
    fs::create_dir_all(&components_dir)
        .with_context(|| format!("failed to create {}", components_dir.display()))
}

async fn resolve_template_digest(runtime: &RuntimeContext) -> String {
    if runtime.network_policy() == NetworkPolicy::Offline {
        eprintln!(
            "warning: offline mode prevents resolving {}; using placeholder digest (run `greentic-pack resolve` when online)",
            TEMPLATE_REF
        );
        return PLACEHOLDER_DIGEST.to_string();
    }
    let opts = DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
    };
    let client = DistClient::new(opts);
    match client.resolve_ref(TEMPLATE_REF).await {
        Ok(resolved) => resolved.digest,
        Err(err) => {
            eprintln!(
                "warning: failed to resolve {}: {}; using placeholder digest",
                TEMPLATE_REF, err
            );
            PLACEHOLDER_DIGEST.to_string()
        }
    }
}
