#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use greentic_distributor_client::{DistClient, DistOptions};
use greentic_flow::schema_validate::{Severity, validate_value_against_schema};
use greentic_flow::wizard_ops::{
    WizardAbi, WizardMode as FlowWizardMode, apply_wizard_answers, decode_component_qa_spec,
    fetch_wizard_spec,
};
use greentic_interfaces_host::component_v0_6::exports::greentic::component::node::{
    ComponentDescriptor, SchemaSource,
};
use greentic_pack::pack_lock::read_pack_lock;
use greentic_types::cbor::canonical;
use greentic_types::i18n_text::I18nText;
use greentic_types::qa::QaSpecSource;
use greentic_types::schemas::common::schema_ir::SchemaIr;
use greentic_types::schemas::component::v0_6_0::qa::{
    ComponentQaSpec, QaMode as SpecQaMode, Question, QuestionKind,
};
use greentic_types::schemas::pack::v0_6_0::PackDescribe;
use greentic_types::schemas::pack::v0_6_0::qa::{
    PackQaSpec, QaMode as PackQaMode, Question as PackQuestion, QuestionKind as PackQuestionKind,
};
use hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;

use crate::config::PackConfig;
use crate::runtime::{NetworkPolicy, RuntimeContext};

#[derive(Debug, Args)]
pub struct QaArgs {
    /// Pack root directory containing pack.yaml.
    #[arg(long = "pack", value_name = "DIR", default_value = ".")]
    pub pack_dir: PathBuf,

    /// QA mode to run.
    #[arg(long = "mode", value_enum, default_value = "default")]
    pub mode: QaModeLabel,

    /// Answers file or directory (defaults to <pack>/answers/<mode>.answers.json).
    #[arg(long = "answers", value_name = "FILE_OR_DIR")]
    pub answers: Option<PathBuf>,

    /// Locale tag for i18n lookup.
    #[arg(long = "locale", default_value = "en")]
    pub locale: String,

    /// Disable interactive prompting (fail if required answers missing).
    #[arg(long = "non-interactive", default_value_t = false)]
    pub non_interactive: bool,

    /// Re-ask questions even if answers exist.
    #[arg(long = "reask", default_value_t = false)]
    pub reask: bool,

    /// Run QA for a specific component id (repeatable).
    #[arg(long = "component", value_name = "ID", action = clap::ArgAction::Append)]
    pub components: Vec<String>,

    /// Run QA for every entry in pack.lock.cbor.
    #[arg(long = "all-locked", default_value_t = false)]
    pub all_locked: bool,

    /// Run pack-level QA only (requires PackDescribe.metadata["greentic.qa"]).
    #[arg(long = "pack-only", default_value_t = false)]
    pub pack_only: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum QaModeLabel {
    Default,
    Setup,
    Update,
    #[value(hide = true)]
    Upgrade,
    Remove,
}

impl QaModeLabel {
    fn as_str(&self) -> &'static str {
        match self {
            QaModeLabel::Default => "default",
            QaModeLabel::Setup => "setup",
            QaModeLabel::Update | QaModeLabel::Upgrade => "update",
            QaModeLabel::Remove => "remove",
        }
    }

    fn to_flow_mode(self) -> FlowWizardMode {
        match self {
            QaModeLabel::Default => FlowWizardMode::Default,
            QaModeLabel::Setup => FlowWizardMode::Setup,
            QaModeLabel::Update | QaModeLabel::Upgrade => FlowWizardMode::Update,
            QaModeLabel::Remove => FlowWizardMode::Remove,
        }
    }

    fn to_spec_mode(self) -> SpecQaMode {
        match self {
            QaModeLabel::Default => SpecQaMode::Default,
            QaModeLabel::Setup => SpecQaMode::Setup,
            QaModeLabel::Update | QaModeLabel::Upgrade => SpecQaMode::Update,
            QaModeLabel::Remove => SpecQaMode::Remove,
        }
    }

    fn to_pack_mode(self) -> PackQaMode {
        match self {
            QaModeLabel::Default => PackQaMode::Default,
            QaModeLabel::Setup => PackQaMode::Setup,
            QaModeLabel::Update | QaModeLabel::Upgrade => PackQaMode::Update,
            QaModeLabel::Remove => PackQaMode::Remove,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AnswersDoc {
    schema_version: u32,
    mode: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pack: BTreeMap<String, serde_json::Value>,
    components: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

impl AnswersDoc {
    fn new(mode: &QaModeLabel) -> Self {
        Self {
            schema_version: 1,
            mode: mode.as_str().to_string(),
            pack: BTreeMap::new(),
            components: BTreeMap::new(),
        }
    }
}

pub fn handle(args: QaArgs, runtime: &RuntimeContext) -> Result<()> {
    if matches!(args.mode, QaModeLabel::Upgrade) {
        eprintln!(
            "warning: --mode upgrade is deprecated; use --mode update (alias retained for compatibility)"
        );
    }
    let pack_dir = args
        .pack_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve pack dir {}", args.pack_dir.display()))?;
    let pack_yaml = pack_dir.join("pack.yaml");
    if args.pack_only && (args.all_locked || !args.components.is_empty()) {
        bail!("--pack-only cannot be combined with --component or --all-locked");
    }

    let config = read_pack_config(&pack_yaml)?;
    let lock = if args.pack_only {
        None
    } else {
        Some(
            read_pack_lock(&pack_dir.join("pack.lock.cbor")).with_context(|| {
                format!("failed to read pack.lock.cbor under {}", pack_dir.display())
            })?,
        )
    };

    let (answers_json_path, answers_cbor_path) =
        resolve_answers_paths(&pack_dir, args.answers.as_deref(), args.mode.as_str())?;

    let mut answers = load_answers(&answers_json_path, args.reask, &args.mode)?;

    let i18n_bundle = load_i18n_bundle(&pack_dir, &args.locale)?;
    let pack_qa_spec = load_pack_qa_spec(&pack_dir, args.mode.to_pack_mode(), args.pack_only)?;
    let dist = DistClient::new(DistOptions {
        cache_dir: runtime.cache_dir(),
        allow_tags: true,
        offline: runtime.network_policy() == NetworkPolicy::Offline,
        allow_insecure_local_http: false,
        ..DistOptions::default()
    });

    let wasm_paths = index_component_paths(&config, &pack_dir);
    let targets = if args.pack_only {
        Vec::new()
    } else {
        let lock = lock
            .as_ref()
            .ok_or_else(|| anyhow!("pack.lock.cbor is required unless --pack-only is set"))?;
        select_target_components(&config, lock, &args)?
    };

    if let Some(spec) = pack_qa_spec.as_ref() {
        let pack_answers = collect_answers_for_pack(
            spec,
            if args.reask {
                None
            } else {
                Some(&answers.pack)
            },
            &i18n_bundle,
            args.non_interactive,
            args.reask,
        )?;
        answers.pack = pack_answers;
    }

    for component_id in targets {
        let lock = lock
            .as_ref()
            .ok_or_else(|| anyhow!("pack.lock.cbor is required unless --pack-only is set"))?;
        let locked = lock.components.get(&component_id).ok_or_else(|| {
            anyhow!(
                "component {} missing from pack.lock.cbor (run `greentic-pack resolve`)",
                component_id
            )
        })?;
        let reference = match locked.r#ref.as_deref() {
            Some(reference) => reference.to_string(),
            None => {
                let path = wasm_paths.get(&component_id).ok_or_else(|| {
                    anyhow!(
                        "pack.lock entry {} has no ref and no pack.yaml wasm path",
                        component_id
                    )
                })?;
                format!("file://{}", path.display())
            }
        };
        let resolved =
            resolve_component_bytes(&dist, runtime, &reference, Some(&locked.resolved_digest))?;

        let spec = load_component_qa_spec(&resolved.bytes, args.mode.to_flow_mode())
            .with_context(|| format!("load QA spec for {}", component_id))?;

        if spec.mode != args.mode.to_spec_mode() {
            bail!(
                "component {} returned QA spec for {:?}, expected {:?}",
                component_id,
                spec.mode,
                args.mode.to_spec_mode()
            );
        }

        let existing = answers.components.get(&component_id).cloned();
        let updated = collect_answers_for_component(
            &component_id,
            &spec,
            existing.as_ref(),
            &i18n_bundle,
            args.non_interactive,
            args.reask,
        )?;
        answers.components.insert(component_id.clone(), updated);

        let component_answers = answers
            .components
            .get(&component_id)
            .cloned()
            .unwrap_or_default();
        let answers_cbor = canonical::to_canonical_cbor_allow_floats(&component_answers)
            .with_context(|| format!("encode answers cbor for {}", component_id))?;
        let current_config = canonical::to_canonical_cbor_allow_floats(&serde_json::json!({}))
            .context("encode empty config cbor")?;
        let config_cbor = apply_component_answers(
            &resolved.bytes,
            args.mode.to_flow_mode(),
            &current_config,
            &answers_cbor,
        )
        .with_context(|| format!("apply-answers for {}", component_id))?;
        let wizard_spec = fetch_wizard_spec(&resolved.bytes, args.mode.to_flow_mode())
            .with_context(|| format!("load setup descriptor for {}", component_id))?;
        validate_component_config_output(
            &component_id,
            wizard_spec.descriptor.as_ref(),
            &config_cbor,
        )?;
    }

    write_answers(&answers_json_path, &answers_cbor_path, &answers)?;
    eprintln!("wrote {}", answers_json_path.display());
    eprintln!("wrote {}", answers_cbor_path.display());

    Ok(())
}

fn read_pack_config(pack_yaml: &Path) -> Result<PackConfig> {
    let contents = fs::read_to_string(pack_yaml)
        .with_context(|| format!("failed to read {}", pack_yaml.display()))?;
    let cfg: PackConfig = serde_yaml_bw::from_str(&contents)
        .with_context(|| format!("{} is not a valid pack.yaml", pack_yaml.display()))?;
    Ok(cfg)
}

fn load_pack_qa_spec(
    pack_dir: &Path,
    mode: PackQaMode,
    require: bool,
) -> Result<Option<PackQaSpec>> {
    let pack_cbor = pack_dir.join("pack.cbor");
    if !pack_cbor.exists() {
        if require {
            bail!("pack.cbor not found (pack-level QA requires pack.cbor with metadata)");
        }
        return Ok(None);
    }
    let bytes =
        fs::read(&pack_cbor).with_context(|| format!("failed to read {}", pack_cbor.display()))?;
    let describe: PackDescribe =
        canonical::from_cbor(&bytes).with_context(|| format!("decode {}", pack_cbor.display()))?;

    let Some(value) = describe.metadata.get("greentic.qa") else {
        if require {
            bail!("pack.cbor metadata missing greentic.qa");
        }
        return Ok(None);
    };

    let source: QaSpecSource =
        decode_qa_spec_source(value).context("decode pack-level QA spec source")?;

    let spec = match source {
        QaSpecSource::InlineCbor(bytes) => decode_pack_qa_spec(bytes.as_slice())?,
        QaSpecSource::RefPackPath(path) => {
            let path = pack_dir.join(path);
            let bytes =
                fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            canonical::ensure_canonical(&bytes).context("pack QA spec must be canonical CBOR")?;
            decode_pack_qa_spec(bytes.as_slice())?
        }
        QaSpecSource::RefUri(uri) => {
            bail!("pack QA RefUri not supported yet: {}", uri);
        }
    };

    if spec.mode != mode {
        bail!(
            "pack QA spec mode mismatch: expected {:?}, got {:?}",
            mode,
            spec.mode
        );
    }
    Ok(Some(spec))
}

fn decode_qa_spec_source(value: &ciborium::value::Value) -> Result<QaSpecSource> {
    let bytes = canonical::to_canonical_cbor_allow_floats(value)
        .context("canonicalize QaSpecSource metadata")?;
    let source: QaSpecSource =
        canonical::from_cbor(&bytes).context("decode QaSpecSource metadata")?;
    Ok(source)
}

fn decode_pack_qa_spec(bytes: &[u8]) -> Result<PackQaSpec> {
    canonical::from_cbor(bytes).context("decode PackQaSpec")
}

fn index_component_paths(config: &PackConfig, pack_dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    for component in &config.components {
        let path = if component.wasm.is_absolute() {
            component.wasm.clone()
        } else {
            pack_dir.join(&component.wasm)
        };
        map.insert(component.id.clone(), path);
    }
    map
}

fn select_target_components(
    config: &PackConfig,
    lock: &greentic_pack::pack_lock::PackLockV1,
    args: &QaArgs,
) -> Result<Vec<String>> {
    if args.all_locked && !args.components.is_empty() {
        bail!("--component cannot be combined with --all-locked");
    }

    let mut targets = Vec::new();
    let mut pack_component_ids: BTreeMap<String, ()> = BTreeMap::new();
    for component in &config.components {
        pack_component_ids.insert(component.id.clone(), ());
    }

    if !args.components.is_empty() {
        for component_id in &args.components {
            if !pack_component_ids.contains_key(component_id) {
                bail!(
                    "component {} not found in pack.yaml (use --all-locked to target lock-only entries)",
                    component_id
                );
            }
            targets.push(component_id.clone());
        }
        targets.sort();
        targets.dedup();
        return Ok(targets);
    }

    if args.all_locked {
        targets.extend(lock.components.keys().cloned());
        targets.sort();
        return Ok(targets);
    }

    targets.extend(pack_component_ids.keys().cloned());
    targets.sort();
    Ok(targets)
}

fn resolve_answers_paths(
    pack_dir: &Path,
    answers: Option<&Path>,
    mode: &str,
) -> Result<(PathBuf, PathBuf)> {
    let (json_path, cbor_path) = match answers {
        None => {
            let dir = pack_dir.join("answers");
            (
                dir.join(format!("{mode}.answers.json")),
                dir.join(format!("{mode}.answers.cbor")),
            )
        }
        Some(path) if path.is_dir() => (
            path.join(format!("{mode}.answers.json")),
            path.join(format!("{mode}.answers.cbor")),
        ),
        Some(path) => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if ext.eq_ignore_ascii_case("json") {
                let cbor = path.with_extension("cbor");
                (path.to_path_buf(), cbor)
            } else if ext.eq_ignore_ascii_case("cbor") {
                let json = path.with_extension("json");
                (json, path.to_path_buf())
            } else {
                let json = path.with_extension("answers.json");
                let cbor = path.with_extension("answers.cbor");
                (json, cbor)
            }
        }
    };

    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    Ok((json_path, cbor_path))
}

fn load_answers(path: &Path, reask: bool, mode: &QaModeLabel) -> Result<AnswersDoc> {
    if reask || !path.exists() {
        return Ok(AnswersDoc::new(mode));
    }
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut doc: AnswersDoc = serde_json::from_str(&contents)
        .with_context(|| format!("{} is not valid answers JSON", path.display()))?;
    if doc.mode != mode.as_str() {
        doc.mode = mode.as_str().to_string();
    }
    Ok(doc)
}

fn write_answers(json_path: &Path, cbor_path: &Path, answers: &AnswersDoc) -> Result<()> {
    let json_bytes = to_sorted_json_bytes(answers)?;
    fs::write(json_path, json_bytes).with_context(|| format!("write {}", json_path.display()))?;

    let cbor_bytes =
        canonical::to_canonical_cbor_allow_floats(answers).context("encode answers CBOR")?;
    fs::write(cbor_path, cbor_bytes).with_context(|| format!("write {}", cbor_path.display()))?;
    Ok(())
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

fn load_i18n_bundle(pack_dir: &Path, locale: &str) -> Result<BTreeMap<String, String>> {
    let path = pack_dir
        .join("assets")
        .join("i18n")
        .join(format!("{locale}.json"));
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let mut map = BTreeMap::new();
    if let serde_json::Value::Object(entries) = raw {
        for (key, value) in entries {
            if let Some(value) = value.as_str() {
                map.insert(key, value.to_string());
            }
        }
    }
    Ok(map)
}

fn collect_answers_for_component(
    component_id: &str,
    spec: &ComponentQaSpec,
    existing: Option<&BTreeMap<String, serde_json::Value>>,
    i18n_bundle: &BTreeMap<String, String>,
    non_interactive: bool,
    reask: bool,
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut answers = existing.cloned().unwrap_or_default();

    if !spec.questions.is_empty() {
        println!();
        println!("== {} ==", render_text(&spec.title, i18n_bundle));
        if let Some(desc) = &spec.description {
            println!("{}", render_text(desc, i18n_bundle));
        }
    }

    for question in &spec.questions {
        if !reask && answers.contains_key(&question.id) {
            continue;
        }

        let default = question
            .default
            .as_ref()
            .map(cbor_value_to_json)
            .or_else(|| spec.defaults.get(&question.id).map(cbor_value_to_json));

        let value = if non_interactive {
            if let Some(existing) = answers.get(&question.id) {
                Some(existing.clone())
            } else if let Some(default) = default {
                Some(default)
            } else if question.required {
                return Err(anyhow!(
                    "missing required answer {}.{} (non-interactive)",
                    component_id,
                    question.id
                ));
            } else {
                None
            }
        } else {
            prompt_question(question, i18n_bundle, default.as_ref())?
        };

        if let Some(value) = value {
            answers.insert(question.id.clone(), value);
        }
    }

    Ok(answers)
}

fn collect_answers_for_pack(
    spec: &PackQaSpec,
    existing: Option<&BTreeMap<String, serde_json::Value>>,
    i18n_bundle: &BTreeMap<String, String>,
    non_interactive: bool,
    reask: bool,
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut answers = existing.cloned().unwrap_or_default();

    if !spec.questions.is_empty() {
        println!();
        println!("== {} ==", render_text(&spec.title, i18n_bundle));
        if let Some(desc) = &spec.description {
            println!("{}", render_text(desc, i18n_bundle));
        }
    }

    for question in &spec.questions {
        if !reask && answers.contains_key(&question.id) {
            continue;
        }

        let default = question
            .default
            .as_ref()
            .map(cbor_value_to_json)
            .or_else(|| spec.defaults.get(&question.id).map(cbor_value_to_json));

        let value = if non_interactive {
            if let Some(existing) = answers.get(&question.id) {
                Some(existing.clone())
            } else if let Some(default) = default {
                Some(default)
            } else if question.required {
                return Err(anyhow!(
                    "missing required pack answer {} (non-interactive)",
                    question.id
                ));
            } else {
                None
            }
        } else {
            prompt_pack_question(question, i18n_bundle, default.as_ref())?
        };

        if let Some(value) = value {
            answers.insert(question.id.clone(), value);
        }
    }

    Ok(answers)
}

fn render_text(text: &I18nText, i18n_bundle: &BTreeMap<String, String>) -> String {
    if let Some(value) = i18n_bundle.get(&text.key) {
        return value.clone();
    }
    if let Some(fallback) = &text.fallback {
        return fallback.clone();
    }
    text.key.clone()
}

fn prompt_question(
    question: &Question,
    i18n_bundle: &BTreeMap<String, String>,
    default: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let label = render_text(&question.label, i18n_bundle);
    let help = question
        .help
        .as_ref()
        .map(|help| render_text(help, i18n_bundle));

    loop {
        print_prompt(&label, help.as_deref(), default);
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            if let Some(default) = default {
                return Ok(Some(default.clone()));
            }
            if question.required {
                println!("Answer required.");
                continue;
            }
            return Ok(None);
        }

        let parsed = match &question.kind {
            QuestionKind::Text => serde_json::Value::String(input.to_string()),
            QuestionKind::Number => match input.parse::<f64>() {
                Ok(value) => serde_json::Value::Number(
                    serde_json::Number::from_f64(value)
                        .ok_or_else(|| anyhow!("invalid numeric input"))?,
                ),
                Err(_) => {
                    println!("Expected a number.");
                    continue;
                }
            },
            QuestionKind::Bool => match parse_bool(input) {
                Some(value) => serde_json::Value::Bool(value),
                None => {
                    println!("Expected yes/no.");
                    continue;
                }
            },
            QuestionKind::Choice { options } => match parse_choice(input, options, i18n_bundle) {
                Some(value) => serde_json::Value::String(value),
                None => {
                    println!("Select one of the listed options.");
                    continue;
                }
            },
        };

        return Ok(Some(parsed));
    }
}

fn prompt_pack_question(
    question: &PackQuestion,
    i18n_bundle: &BTreeMap<String, String>,
    default: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    let label = render_text(&question.label, i18n_bundle);
    let help = question
        .help
        .as_ref()
        .map(|help| render_text(help, i18n_bundle));

    loop {
        print_prompt(&label, help.as_deref(), default);
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            if let Some(default) = default {
                return Ok(Some(default.clone()));
            }
            if question.required {
                println!("Answer required.");
                continue;
            }
            return Ok(None);
        }

        let parsed = match &question.kind {
            PackQuestionKind::Text => serde_json::Value::String(input.to_string()),
            PackQuestionKind::Number => match input.parse::<f64>() {
                Ok(value) => serde_json::Value::Number(
                    serde_json::Number::from_f64(value)
                        .ok_or_else(|| anyhow!("invalid numeric input"))?,
                ),
                Err(_) => {
                    println!("Expected a number.");
                    continue;
                }
            },
            PackQuestionKind::Bool => match parse_bool(input) {
                Some(value) => serde_json::Value::Bool(value),
                None => {
                    println!("Expected yes/no.");
                    continue;
                }
            },
            PackQuestionKind::Choice { options } => {
                match parse_pack_choice(input, options, i18n_bundle) {
                    Some(value) => serde_json::Value::String(value),
                    None => {
                        println!("Select one of the listed options.");
                        continue;
                    }
                }
            }
        };

        return Ok(Some(parsed));
    }
}

fn print_prompt(label: &str, help: Option<&str>, default: Option<&serde_json::Value>) {
    if let Some(help) = help {
        println!("{label} ({help})");
    } else {
        println!("{label}");
    }
    if let Some(default) = default {
        println!("default: {}", default);
    }
    print!("> ");
    let _ = io::stdout().flush();
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.to_ascii_lowercase().as_str() {
        "y" | "yes" | "true" | "1" => Some(true),
        "n" | "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_choice(
    input: &str,
    options: &[greentic_types::schemas::component::v0_6_0::qa::ChoiceOption],
    i18n_bundle: &BTreeMap<String, String>,
) -> Option<String> {
    let index = input.parse::<usize>().ok();
    if let Some(idx) = index
        && idx > 0
        && idx <= options.len()
    {
        return Some(options[idx - 1].value.clone());
    }
    options
        .iter()
        .find(|option| option.value == input)
        .map(|option| option.value.clone())
        .or_else(|| {
            options
                .iter()
                .find(|option| render_text(&option.label, i18n_bundle) == input)
                .map(|option| option.value.clone())
        })
}

fn parse_pack_choice(
    input: &str,
    options: &[greentic_types::schemas::pack::v0_6_0::qa::ChoiceOption],
    i18n_bundle: &BTreeMap<String, String>,
) -> Option<String> {
    let index = input.parse::<usize>().ok();
    if let Some(idx) = index
        && idx > 0
        && idx <= options.len()
    {
        return Some(options[idx - 1].value.clone());
    }
    options
        .iter()
        .find(|option| option.value == input)
        .map(|option| option.value.clone())
        .or_else(|| {
            options
                .iter()
                .find(|option| render_text(&option.label, i18n_bundle) == input)
                .map(|option| option.value.clone())
        })
}

fn cbor_value_to_json(value: &ciborium::value::Value) -> serde_json::Value {
    use ciborium::value::Value;
    match value {
        Value::Integer(int) => {
            let raw: i128 = (*int).into();
            if let Ok(value) = i64::try_from(raw) {
                serde_json::Value::Number(value.into())
            } else if let Ok(value) = u64::try_from(raw) {
                serde_json::Value::Number(value.into())
            } else {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(raw as f64)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                )
            }
        }
        Value::Bytes(bytes) => serde_json::Value::String(hex::encode(bytes)),
        Value::Float(value) => serde_json::Value::Number(
            serde_json::Number::from_f64(*value).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Value::Text(value) => serde_json::Value::String(value.clone()),
        Value::Bool(value) => serde_json::Value::Bool(*value),
        Value::Null => serde_json::Value::Null,
        Value::Tag(_, inner) => cbor_value_to_json(inner),
        Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(cbor_value_to_json).collect())
        }
        Value::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (key, value) in entries {
                let key_string = match key {
                    Value::Text(value) => value.clone(),
                    Value::Integer(int) => {
                        let raw: i128 = (*int).into();
                        raw.to_string()
                    }
                    other => format!("{other:?}"),
                };
                map.insert(key_string, cbor_value_to_json(value));
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::Value::Null,
    }
}

struct ResolvedBytes {
    bytes: Vec<u8>,
}

fn resolve_component_bytes(
    dist: &DistClient,
    runtime: &RuntimeContext,
    reference: &str,
    expected_digest: Option<&str>,
) -> Result<ResolvedBytes> {
    if let Some(path) = reference.strip_prefix("file://") {
        let bytes = fs::read(path).with_context(|| format!("read {}", path))?;
        let digest = digest_for_bytes(&bytes);
        if let Some(expected) = expected_digest
            && expected != digest
        {
            bail!(
                "digest mismatch for {} (expected {}, got {})",
                reference,
                expected,
                digest
            );
        }
        return Ok(ResolvedBytes { bytes });
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
    if let Some(expected) = expected_digest
        && expected != digest
    {
        bail!(
            "digest mismatch for {} (expected {}, got {})",
            reference,
            expected,
            digest
        );
    }
    Ok(ResolvedBytes { bytes })
}

fn digest_for_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn block_on<F, T, E>(handle: &Handle, fut: F) -> std::result::Result<T, E>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
{
    tokio::task::block_in_place(|| handle.block_on(fut))
}

fn load_component_qa_spec(bytes: &[u8], mode: FlowWizardMode) -> Result<ComponentQaSpec> {
    let spec = fetch_wizard_spec(bytes, mode).context("fetch wizard spec from component")?;
    decode_component_qa_spec(&spec.qa_spec_cbor, mode).context("decode wizard qa-spec")
}

fn apply_component_answers(
    bytes: &[u8],
    mode: FlowWizardMode,
    current_config: &[u8],
    answers: &[u8],
) -> Result<Vec<u8>> {
    apply_wizard_answers(bytes, WizardAbi::V6, mode, current_config, answers)
        .context("invoke setup.apply_answers")
}

fn validate_component_config_output(
    component_id: &str,
    descriptor: Option<&ComponentDescriptor>,
    config_cbor: &[u8],
) -> Result<()> {
    canonical::ensure_canonical(config_cbor)
        .context("apply-answers output must be canonical CBOR")?;

    let value: ciborium::value::Value =
        ciborium::de::from_reader(config_cbor).context("decode apply-answers output as CBOR")?;

    let schema = match descriptor {
        Some(descriptor) => setup_apply_answers_output_schema(descriptor).with_context(|| {
            format!("decode setup.apply_answers output schema for {component_id}")
        })?,
        None => None,
    };
    let Some(schema) = schema else {
        return Ok(());
    };

    let diags = validate_value_against_schema(&schema, &value);
    let errors: Vec<_> = diags
        .iter()
        .filter(|diag| diag.severity == Severity::Error)
        .collect();
    if errors.is_empty() {
        return Ok(());
    }

    let summary = errors
        .iter()
        .map(|diag| format!("{}: {}", diag.path, diag.message))
        .collect::<Vec<_>>()
        .join("; ");
    bail!(
        "component {} apply-answers output failed schema validation ({} violations): {}",
        component_id,
        errors.len(),
        summary
    );
}

fn setup_apply_answers_output_schema(descriptor: &ComponentDescriptor) -> Result<Option<SchemaIr>> {
    let Some(op) = descriptor
        .ops
        .iter()
        .find(|op| op.name == "setup.apply_answers")
    else {
        return Ok(None);
    };
    match &op.output.schema {
        SchemaSource::InlineCbor(bytes) => {
            let schema: SchemaIr =
                canonical::from_cbor(bytes).context("decode inline output schema")?;
            Ok(Some(schema))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ComponentConfig, FlowKindLabel};
    use clap::ValueEnum;
    use greentic_interfaces_host::component_v0_6::exports::greentic::component::node::{
        ComponentDescriptor, IoSchema, Op, SchemaSource,
    };
    use greentic_pack::pack_lock::PackLockV1;
    use greentic_types::cbor_bytes::CborBytes;
    use greentic_types::schemas::common::schema_ir::{AdditionalProperties, SchemaIr};
    use greentic_types::{ComponentCapabilities, ComponentProfiles};
    use tempfile::TempDir;

    #[test]
    fn answers_paths_default_to_pack_answers_dir() {
        let temp = TempDir::new().expect("temp dir");
        let (json_path, cbor_path) =
            resolve_answers_paths(temp.path(), None, "default").expect("paths");
        assert!(json_path.ends_with("answers/default.answers.json"));
        assert!(cbor_path.ends_with("answers/default.answers.cbor"));
    }

    #[test]
    fn answers_paths_custom_file() {
        let temp = TempDir::new().expect("temp dir");
        let custom = temp.path().join("qa.json");
        let (json_path, cbor_path) =
            resolve_answers_paths(temp.path(), Some(custom.as_path()), "setup").expect("paths");
        assert!(json_path.ends_with("qa.json"));
        assert!(cbor_path.ends_with("qa.cbor"));
    }

    #[test]
    fn sorted_json_is_deterministic() {
        let mut map = serde_json::Map::new();
        map.insert("b".to_string(), serde_json::Value::Number(2.into()));
        map.insert("a".to_string(), serde_json::Value::Number(1.into()));
        let value = serde_json::Value::Object(map);
        let bytes = to_sorted_json_bytes(&value).expect("json bytes");
        let text = String::from_utf8(bytes).expect("utf8");
        let a_idx = text.find("\"a\"").expect("a key");
        let b_idx = text.find("\"b\"").expect("b key");
        assert!(a_idx < b_idx);
    }

    #[test]
    fn select_targets_defaults_to_pack_components() {
        let cfg = PackConfig {
            pack_id: "demo".to_string(),
            version: "0.1.0".to_string(),
            kind: "application".to_string(),
            publisher: "Greentic".to_string(),
            name: None,
            bootstrap: None,
            components: vec![ComponentConfig {
                id: "demo.component".to_string(),
                version: "0.1.0".to_string(),
                world: "greentic:component/stub".to_string(),
                supports: vec![FlowKindLabel::Messaging],
                profiles: ComponentProfiles::default(),
                capabilities: ComponentCapabilities::default(),
                wasm: PathBuf::from("components/demo.wasm"),
                operations: Vec::new(),
                config_schema: None,
                resources: None,
                configurators: None,
            }],
            dependencies: Vec::new(),
            flows: Vec::new(),
            assets: Vec::new(),
            extensions: None,
        };
        let lock = PackLockV1::new(BTreeMap::new());
        let args = QaArgs {
            pack_dir: PathBuf::from("."),
            mode: QaModeLabel::Default,
            answers: None,
            locale: "en".to_string(),
            non_interactive: false,
            reask: false,
            components: Vec::new(),
            all_locked: false,
            pack_only: false,
        };
        let targets = select_target_components(&cfg, &lock, &args).expect("targets");
        assert_eq!(targets, vec!["demo.component".to_string()]);
    }

    fn sample_pack_qa_spec(mode: PackQaMode) -> PackQaSpec {
        PackQaSpec {
            mode,
            title: I18nText::new("pack.qa.title", Some("Pack QA".to_string())),
            description: None,
            questions: vec![PackQuestion {
                id: "region".to_string(),
                label: I18nText::new("pack.qa.region", Some("Region".to_string())),
                help: None,
                error: None,
                kind: PackQuestionKind::Text,
                required: true,
                default: None,
            }],
            defaults: BTreeMap::new(),
        }
    }

    fn write_pack_cbor_with_metadata(
        dir: &Path,
        metadata: BTreeMap<String, ciborium::value::Value>,
    ) -> Result<()> {
        let describe = PackDescribe {
            info: greentic_types::schemas::pack::v0_6_0::PackInfo {
                id: "demo.pack".to_string(),
                version: "0.1.0".to_string(),
                role: "application".to_string(),
                display_name: None,
            },
            provided_capabilities: Vec::new(),
            required_capabilities: Vec::new(),
            units_summary: BTreeMap::new(),
            metadata,
        };
        let bytes =
            canonical::to_canonical_cbor_allow_floats(&describe).context("encode pack.cbor")?;
        fs::write(dir.join("pack.cbor"), bytes).context("write pack.cbor")?;
        Ok(())
    }

    #[test]
    fn pack_qa_inline_cbor_is_loaded() {
        let temp = TempDir::new().expect("temp dir");
        let spec = sample_pack_qa_spec(PackQaMode::Default);
        let spec_bytes = canonical::to_canonical_cbor_allow_floats(&spec).expect("spec bytes");
        let source = QaSpecSource::InlineCbor(CborBytes::new(spec_bytes));
        let source_bytes =
            canonical::to_canonical_cbor_allow_floats(&source).expect("source bytes");
        let source_value: ciborium::value::Value =
            canonical::from_cbor(&source_bytes).expect("source value");

        let mut metadata = BTreeMap::new();
        metadata.insert("greentic.qa".to_string(), source_value);
        write_pack_cbor_with_metadata(temp.path(), metadata).expect("pack.cbor");

        let loaded = load_pack_qa_spec(temp.path(), PackQaMode::Default, true)
            .expect("load")
            .expect("spec");
        assert_eq!(loaded.mode, PackQaMode::Default);
    }

    #[test]
    fn pack_qa_ref_pack_path_is_loaded() {
        let temp = TempDir::new().expect("temp dir");
        let spec = sample_pack_qa_spec(PackQaMode::Default);
        let spec_bytes = canonical::to_canonical_cbor_allow_floats(&spec).expect("spec bytes");
        let qa_path = temp.path().join("qa/pack/default.cbor");
        fs::create_dir_all(qa_path.parent().unwrap()).expect("qa dir");
        fs::write(&qa_path, spec_bytes).expect("write qa spec");

        let source = QaSpecSource::RefPackPath("qa/pack/default.cbor".to_string());
        let source_bytes =
            canonical::to_canonical_cbor_allow_floats(&source).expect("source bytes");
        let source_value: ciborium::value::Value =
            canonical::from_cbor(&source_bytes).expect("source value");

        let mut metadata = BTreeMap::new();
        metadata.insert("greentic.qa".to_string(), source_value);
        write_pack_cbor_with_metadata(temp.path(), metadata).expect("pack.cbor");

        let loaded = load_pack_qa_spec(temp.path(), PackQaMode::Default, true)
            .expect("load")
            .expect("spec");
        assert_eq!(loaded.mode, PackQaMode::Default);
    }

    #[test]
    fn qa_mode_upgrade_alias_normalizes_to_update() {
        let update = QaModeLabel::from_str("update", false).expect("parse update");
        let upgrade = QaModeLabel::from_str("upgrade", false).expect("parse upgrade");
        assert_eq!(update.as_str(), "update");
        assert_eq!(upgrade.as_str(), "update");
        assert_eq!(update.to_spec_mode(), SpecQaMode::Update);
        assert_eq!(upgrade.to_spec_mode(), SpecQaMode::Update);
    }

    #[test]
    fn validation_error_includes_paths_and_structured_violations() {
        let output_schema = SchemaIr::Object {
            properties: BTreeMap::from([("enabled".to_string(), SchemaIr::Bool)]),
            required: vec!["enabled".to_string()],
            additional: AdditionalProperties::Forbid,
        };
        let output_schema_cbor =
            canonical::to_canonical_cbor_allow_floats(&output_schema).expect("schema cbor");
        let describe = ComponentDescriptor {
            name: "demo.component".to_string(),
            version: "0.1.0".to_string(),
            summary: None,
            capabilities: Vec::new(),
            ops: vec![Op {
                name: "setup.apply_answers".to_string(),
                summary: None,
                input: IoSchema {
                    schema: SchemaSource::InlineCbor(vec![0xa0]),
                    content_type: "application/cbor".to_string(),
                    schema_version: None,
                },
                output: IoSchema {
                    schema: SchemaSource::InlineCbor(output_schema_cbor),
                    content_type: "application/cbor".to_string(),
                    schema_version: None,
                },
                examples: Vec::new(),
            }],
            schemas: Vec::new(),
            setup: None,
        };
        let config_cbor = canonical::to_canonical_cbor_allow_floats(&serde_json::json!({}))
            .expect("encode config");
        let err = validate_component_config_output("demo.component", Some(&describe), &config_cbor)
            .expect_err("missing required field must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("$.enabled"), "missing path in: {msg}");
        assert!(msg.contains("schema validation"), "missing validation text in: {msg}");
    }
}
