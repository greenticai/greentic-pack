#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_pack::pack_lock::{LockedComponent, PackLockV1, read_pack_lock, write_pack_lock};
use greentic_pack::resolver::{ComponentResolver, FixtureResolver};
use greentic_types::cbor::canonical;
use greentic_types::i18n_text::I18nText;
use greentic_types::schemas::component::v0_6_0::{ComponentDescribe, schema_hash};
use greentic_types::schemas::pack::v0_6_0::{PackDescribe, PackInfo};
use greentic_types::{ComponentCapabilities, ComponentProfiles};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use wasmtime::Engine;
use wasmtime::component::{Component as WasmtimeComponent, Linker};
use wit_component::DecodedWasm;

use crate::component_host_stubs::{DescribeHostState, add_describe_host_imports};
use crate::config::{ComponentConfig, ComponentOperationConfig, FlowKindLabel, PackConfig};
use crate::runtime::{NetworkPolicy, RuntimeContext};

#[derive(Debug, Subcommand)]
pub enum WizardCommand {
    /// Create a deterministic application pack skeleton.
    NewApp(NewAppArgs),
    /// Create a deterministic extension pack skeleton.
    NewExtension(NewExtensionArgs),
    /// Add a component to a pack using its self-described metadata.
    AddComponent(AddComponentArgs),
}

#[derive(Debug, Args)]
pub struct NewAppArgs {
    /// Pack identifier.
    #[arg(value_name = "PACK_ID")]
    pub pack_id: String,
    /// Output directory.
    #[arg(long = "out", value_name = "DIR")]
    pub out: PathBuf,
    /// Locale tag for i18n bundle.
    #[arg(long = "locale", default_value = "en")]
    pub locale: String,
    /// Optional display name (defaults to pack id).
    #[arg(long = "name", value_name = "NAME")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct NewExtensionArgs {
    /// Pack identifier.
    #[arg(value_name = "PACK_ID")]
    pub pack_id: String,
    /// Extension kind (directory under extensions/).
    #[arg(long = "kind", value_name = "KIND")]
    pub kind: String,
    /// Output directory.
    #[arg(long = "out", value_name = "DIR")]
    pub out: PathBuf,
    /// Locale tag for i18n bundle.
    #[arg(long = "locale", default_value = "en")]
    pub locale: String,
    /// Optional display name (defaults to pack id).
    #[arg(long = "name", value_name = "NAME")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct AddComponentArgs {
    /// Component reference (file://, oci://, repo://, store://, sha256:...).
    #[arg(value_name = "REF_OR_ID")]
    pub reference: String,
    /// Pack root directory containing pack.yaml.
    #[arg(long = "pack", value_name = "DIR", default_value = ".")]
    pub pack_dir: PathBuf,
    /// Allow fallback to cached describe payload (<component>.wasm.describe.cbor).
    #[arg(long = "use-describe-cache", default_value_t = false)]
    pub use_describe_cache: bool,
    /// Overwrite an existing components/<id>.wasm.
    #[arg(long = "force", default_value_t = false)]
    pub force: bool,
    /// Show planned changes without writing files.
    #[arg(long = "dry-run", default_value_t = false)]
    pub dry_run: bool,
}

pub fn handle(cmd: WizardCommand, runtime: &RuntimeContext) -> Result<()> {
    match cmd {
        WizardCommand::NewApp(args) => new_pack(
            PackKind::App,
            &args.pack_id,
            None,
            &args.out,
            &args.locale,
            args.name.as_deref(),
        ),
        WizardCommand::NewExtension(args) => new_pack(
            PackKind::Extension,
            &args.pack_id,
            Some(&args.kind),
            &args.out,
            &args.locale,
            args.name.as_deref(),
        ),
        WizardCommand::AddComponent(args) => add_component(args, runtime),
    }
}

#[derive(Clone, Copy)]
enum PackKind {
    App,
    Extension,
}

fn new_pack(
    kind: PackKind,
    pack_id: &str,
    extension_kind: Option<&str>,
    out: &Path,
    locale: &str,
    name: Option<&str>,
) -> Result<()> {
    if pack_id.trim().is_empty() {
        return Err(anyhow!("pack id must not be empty"));
    }
    if let Some(kind) = extension_kind
        && kind.trim().is_empty()
    {
        return Err(anyhow!("extension kind must not be empty"));
    }

    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    fs::create_dir_all(out.join("components"))?;
    fs::create_dir_all(out.join("flows"))?;
    fs::create_dir_all(out.join("assets").join("i18n"))?;

    if let PackKind::Extension = kind {
        let kind = extension_kind.expect("extension kind");
        fs::create_dir_all(out.join("extensions").join(kind))?;
    }

    let display_name = name.unwrap_or(pack_id);
    let info = PackInfo {
        id: pack_id.to_string(),
        version: "0.1.0".to_string(),
        role: match kind {
            PackKind::App => "application".to_string(),
            PackKind::Extension => "extension".to_string(),
        },
        display_name: Some(I18nText::new("pack.name", Some(display_name.to_string()))),
    };

    let describe = PackDescribe {
        info,
        provided_capabilities: Vec::new(),
        required_capabilities: Vec::new(),
        units_summary: BTreeMap::new(),
        metadata: BTreeMap::new(),
    };

    let pack_bytes =
        canonical::to_canonical_cbor_allow_floats(&describe).context("encode pack.cbor")?;
    fs::write(out.join("pack.cbor"), pack_bytes).context("write pack.cbor")?;

    let mut i18n = BTreeMap::new();
    i18n.insert("pack.name".to_string(), display_name.to_string());
    let i18n_bytes = to_sorted_json_bytes(&i18n)?;
    let i18n_path = out
        .join("assets")
        .join("i18n")
        .join(format!("{locale}.json"));
    fs::write(i18n_path, i18n_bytes).context("write i18n bundle")?;

    let readme = match kind {
        PackKind::App => format!(
            "# {display_name}\n\nPack id: `{pack_id}`\n\nThis pack was generated by `greentic-pack wizard new-app`.\n"
        ),
        PackKind::Extension => {
            let kind = extension_kind.unwrap_or("extension");
            format!(
                "# {display_name}\n\nPack id: `{pack_id}`\nExtension kind: `{kind}`\n\nThis pack was generated by `greentic-pack wizard new-extension`.\n"
            )
        }
    };
    fs::write(out.join("README.md"), readme).context("write README.md")?;

    Ok(())
}

fn add_component(args: AddComponentArgs, runtime: &RuntimeContext) -> Result<()> {
    let pack_dir = args
        .pack_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", args.pack_dir.display()))?;
    let pack_yaml = pack_dir.join("pack.yaml");
    let mut config = read_pack_yaml(&pack_yaml)?;

    let reference = normalize_reference(&pack_dir, &args.reference)?;
    let dist = DistClient::new(DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
        allow_insecure_local_http: false,
        ..DistOptions::default()
    });
    let resolved = resolve_component_bytes(&dist, runtime, &reference)?;

    let engine = Engine::default();
    let describe = describe_component(
        &engine,
        &resolved.bytes,
        args.use_describe_cache,
        resolved.source_path.as_deref(),
    )?;
    let component_id = describe.info.id.clone();

    let wasm_rel = PathBuf::from("components").join(format!("{component_id}.wasm"));
    let wasm_abs = pack_dir.join(&wasm_rel);
    if wasm_abs.exists() && !args.force {
        bail!(
            "component artifact already exists at {} (use --force to overwrite)",
            wasm_abs.display()
        );
    }
    if !args.dry_run {
        if let Some(parent) = wasm_abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&wasm_abs, &resolved.bytes)
            .with_context(|| format!("write {}", wasm_abs.display()))?;
    }

    let world = infer_component_world_from_bytes(&resolved.bytes);
    update_pack_yaml(&mut config, &describe, wasm_rel, world.clone())?;
    if !args.dry_run {
        write_pack_yaml(&pack_yaml, &config)?;
    }

    let mut lock = read_or_init_lock(&pack_dir)?;
    let locked = locked_component_from_describe(&describe, &reference, &resolved.digest, world)?;
    lock.components.insert(component_id.clone(), locked);
    let lock_path = pack_dir.join("pack.lock.cbor");
    if !args.dry_run {
        write_pack_lock(&lock_path, &lock)?;
    }

    if args.dry_run {
        eprintln!(
            "{}",
            crate::cli_i18n::t("cli.wizard.dry_run.update_pack_yaml")
        );
        eprintln!(
            "{}",
            crate::cli_i18n::tf(
                "cli.wizard.dry_run.would_write",
                &[&lock_path.display().to_string()]
            )
        );
        eprintln!(
            "{}",
            crate::cli_i18n::tf(
                "cli.wizard.dry_run.would_write",
                &[&wasm_abs.display().to_string()]
            )
        );
    } else {
        eprintln!("{}", crate::cli_i18n::t("cli.wizard.updated_pack_yaml"));
        eprintln!(
            "{}",
            crate::cli_i18n::tf("cli.common.wrote_path", &[&lock_path.display().to_string()])
        );
    }
    Ok(())
}

fn read_pack_yaml(path: &Path) -> Result<PackConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let config: PackConfig = serde_yaml_bw::from_str(&contents)
        .with_context(|| format!("{} is not a valid pack.yaml", path.display()))?;
    Ok(config)
}

fn write_pack_yaml(path: &Path, config: &PackConfig) -> Result<()> {
    let serialized = serde_yaml_bw::to_string(config).context("serialize pack.yaml")?;
    fs::write(path, serialized).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn update_pack_yaml(
    config: &mut PackConfig,
    describe: &ComponentDescribe,
    wasm_rel: PathBuf,
    world: Option<String>,
) -> Result<()> {
    let mut component = match config
        .components
        .iter()
        .position(|entry| entry.id == describe.info.id)
    {
        Some(idx) => config.components.remove(idx),
        None => default_component(describe.info.id.clone(), wasm_rel.clone()),
    };

    component.id = describe.info.id.clone();
    component.version = describe.info.version.clone();
    component.wasm = wasm_rel;
    if let Some(world) = world {
        component.world = world;
    }

    component.operations = describe
        .operations
        .iter()
        .map(|op| {
            let input_schema =
                serde_json::to_value(&op.input.schema).context("serialize input schema")?;
            let output_schema =
                serde_json::to_value(&op.output.schema).context("serialize output schema")?;
            Ok(ComponentOperationConfig {
                name: op.id.clone(),
                input_schema,
                output_schema,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    component.operations.sort_by(|a, b| a.name.cmp(&b.name));

    component.config_schema =
        Some(serde_json::to_value(&describe.config_schema).context("serialize config schema")?);

    config.components.push(component);
    config.components.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}

fn default_component(id: String, wasm: PathBuf) -> ComponentConfig {
    ComponentConfig {
        id,
        version: "0.1.0".to_string(),
        world: "greentic:component/stub".to_string(),
        supports: vec![FlowKindLabel::Messaging],
        profiles: ComponentProfiles {
            default: Some("default".to_string()),
            supported: vec!["default".to_string()],
        },
        capabilities: ComponentCapabilities::default(),
        wasm,
        operations: Vec::new(),
        config_schema: None,
        resources: None,
        configurators: None,
    }
}

fn read_or_init_lock(pack_dir: &Path) -> Result<PackLockV1> {
    let path = pack_dir.join("pack.lock.cbor");
    if path.exists() {
        read_pack_lock(&path)
    } else {
        Ok(PackLockV1::new(BTreeMap::new()))
    }
}

fn locked_component_from_describe(
    describe: &ComponentDescribe,
    reference: &str,
    digest: &str,
    world: Option<String>,
) -> Result<LockedComponent> {
    let describe_hash = compute_describe_hash(describe)?;
    let mut operations = Vec::new();
    for op in &describe.operations {
        let hash = schema_hash(&op.input.schema, &op.output.schema, &describe.config_schema)
            .map_err(|err| anyhow!("schema_hash for {}: {}", op.id, err))?;
        if hash != op.schema_hash {
            bail!(
                "operation {} schema_hash mismatch (describe={}, computed={})",
                op.id,
                op.schema_hash,
                hash
            );
        }
        operations.push(greentic_pack::pack_lock::LockedOperation {
            operation_id: op.id.clone(),
            schema_hash: hash,
        });
    }
    operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));

    Ok(LockedComponent {
        component_id: describe.info.id.clone(),
        r#ref: Some(reference.to_string()),
        abi_version: "0.6.0".to_string(),
        resolved_digest: digest.to_string(),
        describe_hash,
        operations,
        world,
        component_version: Some(describe.info.version.clone()),
        role: Some(describe.info.role.clone()),
    })
}

fn normalize_reference(pack_dir: &Path, input: &str) -> Result<String> {
    if input.contains("://") || input.starts_with("sha256:") {
        return Ok(input.to_string());
    }
    let candidate = pack_dir.join(input);
    if candidate.exists() {
        let abs = candidate
            .canonicalize()
            .with_context(|| format!("resolve {}", candidate.display()))?;
        return Ok(format!("file://{}", abs.display()));
    }
    let path = PathBuf::from(input);
    if path.exists() {
        let abs = path
            .canonicalize()
            .with_context(|| format!("resolve {}", path.display()))?;
        return Ok(format!("file://{}", abs.display()));
    }
    Ok(format!("repo://{}", input))
}

struct ResolvedBytes {
    bytes: Vec<u8>,
    digest: String,
    source_path: Option<PathBuf>,
}

fn resolve_component_bytes(
    dist: &DistClient,
    runtime: &RuntimeContext,
    reference: &str,
) -> Result<ResolvedBytes> {
    if reference.starts_with("fixture://") {
        let fixture_id = reference
            .trim_start_matches("fixture://")
            .trim_end_matches('/');
        let root = fixture_root()?;
        let resolver = FixtureResolver::new(root);
        let resolved = resolver.resolve(greentic_pack::resolver::ResolveReq {
            component_id: fixture_id.to_string(),
            reference: reference.to_string(),
            expected_digest: String::new(),
            abi_version: "0.6.0".to_string(),
            world: None,
            component_version: None,
        })?;
        return Ok(ResolvedBytes {
            bytes: resolved.bytes,
            digest: resolved.resolved_digest,
            source_path: resolved.source_path,
        });
    }
    if let Some(path) = reference.strip_prefix("file://") {
        let bytes = fs::read(path).with_context(|| format!("read {}", path))?;
        let digest = digest_for_bytes(&bytes);
        return Ok(ResolvedBytes {
            bytes,
            digest,
            source_path: Some(PathBuf::from(path)),
        });
    }

    let handle = Handle::try_current().context("component resolution requires a Tokio runtime")?;
    let resolved = if runtime.network_policy() == NetworkPolicy::Offline {
        block_on(&handle, dist.ensure_cached(reference))
            .map_err(|err| anyhow!("offline cache miss for {}: {}", reference, err))?
    } else {
        block_on(&handle, dist.resolve_ref(reference))
            .map_err(|err| anyhow!("resolve {}: {}", reference, err))?
    };
    let path = resolved
        .cache_path
        .ok_or_else(|| anyhow!("resolved component missing path for {}", reference))?;
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let digest = digest_for_bytes(&bytes);
    if digest != resolved.digest {
        bail!(
            "digest mismatch for {} (expected {}, got {})",
            reference,
            resolved.digest,
            digest
        );
    }
    Ok(ResolvedBytes {
        bytes,
        digest,
        source_path: Some(path),
    })
}

fn digest_for_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn fixture_root() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("GREENTIC_PACK_FIXTURE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if cfg!(test) {
        return Ok(PathBuf::from("crates/packc/tests/fixtures/registry"));
    }
    Err(anyhow!(
        "fixture resolver requires GREENTIC_PACK_FIXTURE_DIR to be set"
    ))
}

fn block_on<F, T, E>(handle: &Handle, fut: F) -> std::result::Result<T, E>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

fn describe_component(
    engine: &Engine,
    bytes: &[u8],
    use_cache: bool,
    source_path: Option<&Path>,
) -> Result<ComponentDescribe> {
    match describe_component_untyped(engine, bytes) {
        Ok(describe) => Ok(describe),
        Err(err) => {
            if use_cache && let Some(describe) = load_describe_from_cache_path(source_path)? {
                return Ok(describe);
            }
            Err(err)
        }
    }
}

fn describe_component_untyped(engine: &Engine, bytes: &[u8]) -> Result<ComponentDescribe> {
    let component = WasmtimeComponent::from_binary(engine, bytes)
        .map_err(|err| anyhow!("decode component bytes: {err}"))?;
    let mut store = wasmtime::Store::new(engine, DescribeHostState::default());
    let mut linker = Linker::new(engine);
    add_describe_host_imports(&mut linker)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|err| anyhow!("instantiate component root world: {err}"))?;

    let descriptor = [
        "component-descriptor",
        "greentic:component/component-descriptor",
        "greentic:component/component-descriptor@0.6.0",
    ]
    .iter()
    .find_map(|name| instance.get_export_index(&mut store, None, name))
    .ok_or_else(|| anyhow!("missing exported descriptor instance"))?;
    let describe_export = [
        "describe",
        "greentic:component/component-descriptor@0.6.0#describe",
    ]
    .iter()
    .find_map(|name| instance.get_export_index(&mut store, Some(&descriptor), name))
    .ok_or_else(|| anyhow!("missing exported describe function"))?;
    let describe_func = instance
        .get_typed_func::<(), (Vec<u8>,)>(&mut store, &describe_export)
        .map_err(|err| anyhow!("lookup component-descriptor.describe: {err}"))?;
    let (describe_bytes,) = describe_func
        .call(&mut store, ())
        .map_err(|err| anyhow!("call component-descriptor.describe: {err}"))?;
    canonical::from_cbor(&describe_bytes).context("decode ComponentDescribe")
}

fn load_describe_from_cache_path(path: Option<&Path>) -> Result<Option<ComponentDescribe>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let describe_path = PathBuf::from(format!("{}.describe.cbor", path.display()));
    if !describe_path.exists() {
        return Ok(None);
    }
    let bytes =
        fs::read(&describe_path).with_context(|| format!("read {}", describe_path.display()))?;
    canonical::ensure_canonical(&bytes).context("describe cache must be canonical")?;
    let describe = canonical::from_cbor(&bytes).context("decode ComponentDescribe from cache")?;
    Ok(Some(describe))
}

fn compute_describe_hash(describe: &ComponentDescribe) -> Result<String> {
    let bytes =
        canonical::to_canonical_cbor_allow_floats(describe).context("canonicalize describe")?;
    let digest = Sha256::digest(bytes.as_slice());
    Ok(hex::encode(digest))
}

fn infer_component_world_from_bytes(bytes: &[u8]) -> Option<String> {
    let decoded = match wit_component::decode(bytes) {
        Ok(decoded) => decoded,
        Err(_) => return None,
    };
    let (resolve, world_id) = match decoded {
        DecodedWasm::Component(resolve, world) => (resolve, world),
        DecodedWasm::WitPackage(..) => return None,
    };
    let world = &resolve.worlds[world_id];
    let pkg_id = world.package?;
    let pkg = &resolve.packages[pkg_id];

    let mut label = format!("{}:{}/{}", pkg.name.namespace, pkg.name.name, world.name);
    if let Some(version) = &pkg.name.version {
        label.push('@');
        label.push_str(&version.to_string());
    }
    Some(label)
}

fn to_sorted_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("encode json")?;
    let sorted = sort_json(value);
    let bytes = serde_json::to_vec_pretty(&sorted).context("serialize json")?;
    Ok(bytes)
}

fn sort_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, sort_json(value));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sort_json).collect())
        }
        other => other,
    }
}
