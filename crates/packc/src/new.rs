#![forbid(unsafe_code)]

use crate::manifest::PackSpec;
use anyhow::{Context, Result, anyhow};
use clap::{Args, ValueEnum};
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use greentic_pack::builder::PACK_VERSION;
use pkcs8::LineEnding;
use rand_core_06::OsRng;
use serde::Serialize;
use serde_json::Map as JsonMap;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_VERSION: &str = "0.1.0";
const DEFAULT_FLOW_FILE: &str = "flows/welcome.ygtc";
const DEFAULT_FLOW_CONTENT: &str = r#"id: welcome
title: Welcome Flow
description: Send a single greeting message.
type: messaging
start: show_welcome

nodes:
  show_welcome:
    templating.handlebars:
      text: |
        👋 Welcome to your new Greentic pack!

        Edit flows/welcome.ygtc to start experimenting.
    routing:
      - out: true
"#;

const BUILD_SCRIPT: &str = r#"#!/usr/bin/env bash
set -euo pipefail
mkdir -p dist
packc build --in . --out dist/pack.wasm --manifest dist/manifest.cbor --sbom dist/sbom.cdx.json
echo "Built dist/pack.wasm"
"#;

const GITIGNORE: &str = r#"dist/
.DS_Store
*.swp
"#;

#[derive(Debug, Clone, ValueEnum, Eq, PartialEq)]
pub enum TemplateKind {
    Minimal,
}

#[derive(Debug, Clone, Args)]
pub struct NewArgs {
    /// Identifier for the new pack (written to pack.yaml)
    #[arg(value_name = "ID")]
    pub id: String,

    /// Output directory for the scaffolded pack (defaults to ./<id>)
    #[arg(long = "dir", value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Pack template to apply (only "minimal" is currently available)
    #[arg(long, value_enum, default_value = "minimal")]
    pub template: TemplateKind,

    /// Generate a development Ed25519 keypair in ./keys/
    #[arg(long)]
    pub sign: bool,

    /// Overwrite existing files/directories if they already exist
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
struct NewSummary {
    root: String,
    created: Vec<ScaffoldEntry>,
    signing: Option<SigningSummary>,
}

#[derive(Debug, Serialize)]
struct ScaffoldEntry {
    path: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct SigningSummary {
    private_key: String,
    public_key: String,
}

pub fn handle(args: NewArgs, emit_json: bool) -> Result<()> {
    if args.id.trim().is_empty() {
        anyhow::bail!("pack id must not be empty");
    }

    ensure_template_supported(&args.template)?;

    let target_dir = normalize(args.dir.unwrap_or_else(|| PathBuf::from(&args.id)));
    ensure_target_dir(&target_dir, args.force)?;

    let pack_yaml = render_pack_spec(&args.id);
    let readme = render_readme(&args.id, args.sign);

    let mut created = Vec::new();

    write_file(
        &target_dir.join("pack.yaml"),
        pack_yaml.as_bytes(),
        args.force,
    )?;
    created.push(entry("pack.yaml", "file"));

    write_file(
        &target_dir.join(DEFAULT_FLOW_FILE),
        DEFAULT_FLOW_CONTENT.as_bytes(),
        args.force,
    )?;
    created.push(entry(DEFAULT_FLOW_FILE, "file"));

    write_file(&target_dir.join("README.md"), readme.as_bytes(), args.force)?;
    created.push(entry("README.md", "file"));

    write_file(
        &target_dir.join(".gitignore"),
        GITIGNORE.as_bytes(),
        args.force,
    )?;
    created.push(entry(".gitignore", "file"));

    write_file(
        &target_dir.join("scripts").join("build.sh"),
        BUILD_SCRIPT.as_bytes(),
        args.force,
    )?;
    make_executable(&target_dir.join("scripts").join("build.sh"))?;
    created.push(entry("scripts/build.sh", "file"));

    ensure_dir(&target_dir.join("dist"))?;
    created.push(entry("dist", "dir"));

    let mut signing_summary = None;
    if args.sign {
        let keys = generate_dev_keys()?;
        let keys_dir = target_dir.join("keys");
        ensure_dir(&keys_dir)?;
        let private_path = keys_dir.join("dev_ed25519.sk");
        let public_path = keys_dir.join("dev_ed25519.pk");
        write_file(&private_path, keys.private_pem.as_bytes(), true)?;
        restrict_private_key_permissions(&private_path)?;
        write_file(&public_path, keys.public_pem.as_bytes(), true)?;
        signing_summary = Some(SigningSummary {
            private_key: relative_string(&target_dir, &private_path),
            public_key: relative_string(&target_dir, &public_path),
        });
        created.push(entry("keys/dev_ed25519.sk", "file"));
        created.push(entry("keys/dev_ed25519.pk", "file"));
    }

    let summary = NewSummary {
        root: target_dir.display().to_string(),
        created,
        signing: signing_summary,
    };

    if emit_json {
        let json = serde_json::to_string_pretty(&summary)?;
        println!("{json}");
    } else {
        println!("Created Greentic pack scaffold at {}", summary.root);
        for item in &summary.created {
            println!("  - {} ({})", item.path, item.kind);
        }
        if let Some(signing) = &summary.signing {
            println!(
                "Generated dev keypair: priv={} pub={}",
                signing.private_key, signing.public_key
            );
        }
    }

    Ok(())
}

fn ensure_template_supported(template: &TemplateKind) -> Result<()> {
    match template {
        TemplateKind::Minimal => Ok(()),
    }
}

fn render_pack_spec(id: &str) -> String {
    let spec = PackSpec {
        pack_version: PACK_VERSION,
        id: id.to_string(),
        version: DEFAULT_VERSION.to_string(),
        kind: Some(PackKind::Application),
        name: Some(id.to_string()),
        description: Some("Starter Greentic pack".into()),
        authors: vec!["Greentic <engineering@greentic.dev>".into()],
        license: Some("MIT".into()),
        homepage: None,
        support: None,
        vendor: None,
        flow_files: vec![DEFAULT_FLOW_FILE.to_string()],
        template_dirs: Vec::new(),
        entry_flows: vec!["welcome".to_string()],
        imports_required: Vec::new(),
        events: None,
        repo: None,
        messaging: None,
        interfaces: Vec::new(),
        mcp_components: Vec::new(),
        annotations: JsonMap::new(),
        components: Vec::new(),
        distribution: None,
    };
    serde_yaml_bw::to_string(&spec).expect("pack spec serialises")
}

fn render_readme(id: &str, signed: bool) -> String {
    let mut readme = format!(
        "# {id}\n\n\
        This pack was created with `packc new`. Build it locally with:\n\n\
        ```bash\n./scripts/build.sh\n```\n\n\
        After building, load `dist/pack.wasm` in the Greentic runner.\n"
    );

    if signed {
        readme.push_str(
            "\nA development Ed25519 keypair was generated under `./keys/`. \
            Use `packc sign --pack . --key ./keys/dev_ed25519.sk` after building \
            to embed a signature block in the manifest.\n",
        );
    }

    readme
}

fn ensure_target_dir(path: &Path, force: bool) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            anyhow::bail!("target {} exists and is not a directory", path.display());
        }
        if !force && dir_has_entries(path)? {
            anyhow::bail!(
                "target directory {} is not empty (use --force to overwrite)",
                path.display()
            );
        }
    } else {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create directory {}", path.display()))?;
    }
    Ok(())
}

fn dir_has_entries(path: &Path) -> Result<bool> {
    let mut entries =
        fs::read_dir(path).with_context(|| format!("failed to inspect {}", path.display()))?;
    Ok(entries.next().is_some())
}

fn write_file(path: &Path, contents: &[u8], force: bool) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))
}

fn entry(relative: &str, kind: &'static str) -> ScaffoldEntry {
    ScaffoldEntry {
        path: relative.to_string(),
        kind,
    }
}

fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to mark {} executable", path.display()))?;
    }
    Ok(())
}

fn restrict_private_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to update permissions for {}", path.display()))?;
    }
    Ok(())
}

struct KeyMaterial {
    private_pem: String,
    public_pem: String,
}

fn generate_dev_keys() -> Result<KeyMaterial> {
    let signing_key = match std::env::var("GREENTIC_DEV_SEED").ok() {
        Some(seed) => signing_key_from_seed(seed),
        None => SigningKey::generate(&mut OsRng),
    };

    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|err| anyhow!("failed to encode private key: {err}"))?;
    let public_pem = signing_key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|err| anyhow!("failed to encode public key: {err}"))?;

    Ok(KeyMaterial {
        private_pem: private_pem.to_string(),
        public_pem: public_pem.to_string(),
    })
}

fn signing_key_from_seed(seed: String) -> SigningKey {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest[..32]);
    SigningKey::from_bytes(&bytes)
}

fn normalize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
use greentic_pack::PackKind;
