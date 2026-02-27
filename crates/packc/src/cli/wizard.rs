#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use clap::Args;
use greentic_qa_lib::{WizardDriver, WizardFrontend, WizardRunConfig};
use greentic_types::ExtensionRef;
use greentic_types::WizardStep;
use greentic_types::pack_manifest::ExtensionInline;
use serde_json::{Value, json};

use crate::cli::wizard_catalog::{
    CatalogQuestion, CatalogQuestionKind, ExtensionCatalog, ExtensionTemplate, ExtensionType,
    load_extension_catalog,
};
use crate::cli::wizard_i18n::{WizardI18n, detect_requested_locale};
use crate::cli::wizard_ui;
use crate::runtime::RuntimeContext;

#[derive(Debug, Args, Default)]
pub struct WizardArgs {}

#[derive(Clone, Copy)]
enum MainChoice {
    CreateApplicationPack,
    UpdateApplicationPack,
    CreateExtensionPack,
    UpdateExtensionPack,
    Exit,
}

#[derive(Clone, Copy)]
enum SubmenuAction {
    Back,
    MainMenu,
}

#[derive(Clone, Copy)]
enum RunMode {
    Harness,
    Cli,
}

#[derive(Default)]
struct WizardSession {
    sign_key_path: Option<String>,
}

pub fn handle(
    _args: WizardArgs,
    runtime: &RuntimeContext,
    requested_locale: Option<&str>,
) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    run_with_mode(
        &mut input,
        &mut output,
        requested_locale,
        RunMode::Cli,
        Some(runtime),
    )
}

pub fn run_with_io<R: BufRead, W: Write>(input: &mut R, output: &mut W) -> Result<()> {
    run_with_mode(
        input,
        output,
        detect_requested_locale().as_deref(),
        RunMode::Harness,
        None,
    )
}

pub fn run_with_io_and_locale<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    requested_locale: Option<&str>,
) -> Result<()> {
    run_with_mode(input, output, requested_locale, RunMode::Harness, None)
}

pub fn run_cli_with_io_and_locale<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    requested_locale: Option<&str>,
) -> Result<()> {
    run_with_mode(input, output, requested_locale, RunMode::Cli, None)
}

fn run_with_mode<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    requested_locale: Option<&str>,
    mode: RunMode,
    runtime: Option<&RuntimeContext>,
) -> Result<()> {
    let i18n = WizardI18n::new(requested_locale);
    let mut session = WizardSession::default();

    loop {
        let choice = ask_main_menu(input, output, &i18n)?;
        match choice {
            MainChoice::CreateApplicationPack => match mode {
                RunMode::Harness => {
                    let _ = ask_placeholder_submenu(
                        input,
                        output,
                        &i18n,
                        "wizard.create_application_pack.title",
                    )?;
                }
                RunMode::Cli => {
                    run_create_application_pack(input, output, &i18n, &mut session)?;
                }
            },
            MainChoice::UpdateApplicationPack => match mode {
                RunMode::Harness => {
                    let _ = ask_placeholder_submenu(
                        input,
                        output,
                        &i18n,
                        "wizard.update_application_pack.title",
                    )?;
                }
                RunMode::Cli => {
                    run_update_application_pack(input, output, &i18n, &mut session)?;
                }
            },
            MainChoice::CreateExtensionPack => match mode {
                RunMode::Harness => {
                    let _ = ask_placeholder_submenu(
                        input,
                        output,
                        &i18n,
                        "wizard.create_extension_pack.title",
                    )?;
                }
                RunMode::Cli => {
                    run_create_extension_pack(input, output, &i18n, runtime)?;
                }
            },
            MainChoice::UpdateExtensionPack => match mode {
                RunMode::Harness => {
                    let _ = ask_placeholder_submenu(
                        input,
                        output,
                        &i18n,
                        "wizard.update_extension_pack.title",
                    )?;
                }
                RunMode::Cli => {
                    run_update_extension_pack(input, output, &i18n, &mut session, runtime)?;
                }
            },
            MainChoice::Exit => return Ok(()),
        }
    }
}

fn run_create_extension_pack<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    runtime: Option<&RuntimeContext>,
) -> Result<()> {
    let catalog_ref = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.create_ext.catalog_ref",
        "wizard.create_extension_pack.ask_catalog_ref",
        Some("wizard.create_extension_pack.ask_catalog_ref_help"),
        Some("oci://ghcr.io/greenticai/catalogs/extensions:latest"),
    )?;

    let catalog = match load_extension_catalog(catalog_ref.trim(), runtime) {
        Ok(value) => value,
        Err(err) => {
            wizard_ui::render_line(
                output,
                &format!("{}: {}", i18n.t("wizard.error.catalog_load_failed"), err),
            )?;
            let nav = ask_failure_nav(input, output, i18n)?;
            if matches!(nav, SubmenuAction::MainMenu) {
                return Ok(());
            }
            return Ok(());
        }
    };

    let type_choice = ask_extension_type(input, output, i18n, &catalog)?;
    if type_choice == "0" || type_choice.eq_ignore_ascii_case("m") {
        return Ok(());
    }

    let selected = catalog
        .extension_types
        .iter()
        .find(|item| item.id == type_choice)
        .ok_or_else(|| anyhow!("selected extension type not found"))?;

    let template = match ask_extension_template(input, output, i18n, selected)? {
        Some(template) => template,
        None => return Ok(()),
    };

    wizard_ui::render_line(
        output,
        &format!(
            "{} {} / {}",
            i18n.t("wizard.create_extension_pack.selected_type"),
            selected.id,
            template.id
        ),
    )?;

    let default_dir = format!("./{}-extension", selected.id.replace('/', "-"));
    let pack_dir = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.create_ext.pack_dir",
        "wizard.create_extension_pack.ask_pack_dir",
        Some("wizard.create_extension_pack.ask_pack_dir_help"),
        Some(&default_dir),
    )?;
    let pack_dir_path = PathBuf::from(pack_dir.trim());
    let qa_answers = ask_template_qa_answers(input, output, i18n, &template)?;
    if let Err(err) = apply_template_plan(&template, &pack_dir_path, selected, i18n, &qa_answers) {
        wizard_ui::render_line(
            output,
            &format!("{}: {err}", i18n.t("wizard.error.template_apply_failed")),
        )?;
        let nav = ask_failure_nav(input, output, i18n)?;
        if matches!(nav, SubmenuAction::MainMenu) {
            return Ok(());
        }
        return Ok(());
    }

    let self_exe = wizard_self_exe()?;
    let mut session = WizardSession::default();
    let finalized = run_update_validate_sequence(
        input,
        output,
        i18n,
        &mut session,
        &self_exe,
        &pack_dir_path,
        true,
        "wizard.progress.running_finalize",
    )?;
    if !finalized {
        let _ = ask_failure_nav(input, output, i18n)?;
    }
    Ok(())
}

fn ask_extension_type<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    catalog: &ExtensionCatalog,
) -> Result<String> {
    let mut choices = catalog
        .extension_types
        .iter()
        .enumerate()
        .map(|(idx, ext)| {
            (
                (idx + 1).to_string(),
                format!(
                    "{} - {}",
                    ext.display_name(i18n),
                    ext.display_description(i18n)
                ),
                ext.id.clone(),
            )
        })
        .collect::<Vec<_>>();

    let mut menu_choices = choices
        .iter()
        .map(|(menu_id, label, _)| (menu_id.clone(), label.clone()))
        .collect::<Vec<_>>();
    menu_choices.push(("0".to_string(), i18n.t("wizard.nav.back")));
    menu_choices.push(("M".to_string(), i18n.t("wizard.nav.main_menu")));

    let choice = ask_enum_custom_labels_owned(
        input,
        output,
        i18n,
        "pack.wizard.create_ext.type",
        "wizard.create_extension_pack.type_menu.title",
        Some("wizard.create_extension_pack.type_menu.description"),
        &menu_choices,
        "M",
    )?;

    if choice == "0" || choice.eq_ignore_ascii_case("m") {
        return Ok(choice);
    }

    let selected = choices
        .iter_mut()
        .find(|(menu_id, _, _)| menu_id == &choice)
        .map(|(_, _, id)| id.clone())
        .ok_or_else(|| anyhow!("invalid extension type selection"))?;
    Ok(selected)
}

fn ask_extension_template<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    extension_type: &ExtensionType,
) -> Result<Option<ExtensionTemplate>> {
    if extension_type.templates.is_empty() {
        return Err(anyhow!("extension type has no templates"));
    }

    let choices = extension_type
        .templates
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            (
                (idx + 1).to_string(),
                format!(
                    "{} - {}",
                    item.display_name(i18n),
                    item.display_description(i18n)
                ),
                item,
            )
        })
        .collect::<Vec<_>>();

    let mut menu_choices = choices
        .iter()
        .map(|(menu_id, label, _)| (menu_id.clone(), label.clone()))
        .collect::<Vec<_>>();
    menu_choices.push(("0".to_string(), i18n.t("wizard.nav.back")));
    menu_choices.push(("M".to_string(), i18n.t("wizard.nav.main_menu")));

    let choice = ask_enum_custom_labels_owned(
        input,
        output,
        i18n,
        "pack.wizard.create_ext.template",
        "wizard.create_extension_pack.template_menu.title",
        Some("wizard.create_extension_pack.template_menu.description"),
        &menu_choices,
        "M",
    )?;

    if choice == "0" || choice.eq_ignore_ascii_case("m") {
        return Ok(None);
    }

    let selected = choices
        .iter()
        .find(|(menu_id, _, _)| menu_id == &choice)
        .map(|(_, _, template)| (*template).clone())
        .ok_or_else(|| anyhow!("invalid extension template selection"))?;
    Ok(Some(selected))
}

fn apply_template_plan(
    template: &ExtensionTemplate,
    pack_dir: &Path,
    extension_type: &ExtensionType,
    i18n: &WizardI18n,
    qa_answers: &BTreeMap<String, String>,
) -> Result<()> {
    fs::create_dir_all(pack_dir)
        .with_context(|| format!("create extension pack dir {}", pack_dir.display()))?;
    for step in &template.plan {
        match step {
            WizardStep::EnsureDir { paths } => {
                for rel in paths {
                    let target = pack_dir.join(rel);
                    fs::create_dir_all(&target)
                        .with_context(|| format!("create directory {}", target.display()))?;
                }
            }
            WizardStep::WriteFiles { files } => {
                for (rel, content) in files {
                    let target = pack_dir.join(rel);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).with_context(|| {
                            format!("create parent directory {}", parent.display())
                        })?;
                    }
                    let rendered = render_template_content(
                        content,
                        extension_type,
                        template,
                        i18n,
                        qa_answers,
                    );
                    fs::write(&target, rendered)
                        .with_context(|| format!("write file {}", target.display()))?;
                }
            }
            WizardStep::RunCli { command, args } => {
                let (rendered_command, rendered_args) = render_run_cli_invocation(
                    command,
                    args,
                    extension_type,
                    template,
                    i18n,
                    qa_answers,
                )?;
                let argv = rendered_args.iter().map(String::as_str).collect::<Vec<_>>();
                let ok = run_process(Path::new(&rendered_command), &argv, Some(pack_dir))
                    .unwrap_or(false);
                if !ok {
                    return Err(anyhow!(
                        "template run_cli step failed: {} {:?}",
                        rendered_command,
                        rendered_args
                    ));
                }
            }
            WizardStep::Delegate { target, .. } => {
                let ok = match target {
                    greentic_types::WizardTarget::Flow => {
                        run_delegate("greentic-flow", &["wizard"], pack_dir)
                    }
                    greentic_types::WizardTarget::Component => {
                        run_delegate("greentic-component", &["wizard"], pack_dir)
                    }
                    _ => false,
                };
                if !ok {
                    return Err(anyhow!(
                        "template delegate step failed for target {:?}",
                        target
                    ));
                }
            }
        }
    }
    Ok(())
}

fn render_template_content(
    content: &str,
    extension_type: &ExtensionType,
    template: &ExtensionTemplate,
    i18n: &WizardI18n,
    qa_answers: &BTreeMap<String, String>,
) -> String {
    render_template_string(content, extension_type, template, i18n, qa_answers)
}

fn render_template_string(
    raw: &str,
    extension_type: &ExtensionType,
    template: &ExtensionTemplate,
    i18n: &WizardI18n,
    qa_answers: &BTreeMap<String, String>,
) -> String {
    let mut rendered = raw
        .replace("{{extension_type_id}}", &extension_type.id)
        .replace(
            "{{extension_type_name}}",
            &extension_type.display_name(i18n),
        )
        .replace("{{template_id}}", &template.id)
        .replace("{{template_name}}", &template.display_name(i18n))
        .replace(
            "{{canonical_extension_key}}",
            "greentic.provider-extension.v1",
        )
        .replace(
            "{{not_implemented}}",
            &i18n.t("wizard.shared.not_implemented"),
        );
    for (key, value) in qa_answers {
        rendered = rendered.replace(&format!("{{{{qa.{key}}}}}"), value);
    }
    rendered
}

fn render_run_cli_invocation(
    command: &str,
    args: &[String],
    extension_type: &ExtensionType,
    template: &ExtensionTemplate,
    i18n: &WizardI18n,
    qa_answers: &BTreeMap<String, String>,
) -> Result<(String, Vec<String>)> {
    let rendered_command =
        render_template_string(command, extension_type, template, i18n, qa_answers);
    validate_run_cli_token(&rendered_command, "command", true)?;

    let mut rendered_args = Vec::with_capacity(args.len());
    for (idx, arg) in args.iter().enumerate() {
        let rendered = render_template_string(arg, extension_type, template, i18n, qa_answers);
        validate_run_cli_token(&rendered, &format!("arg[{idx}]"), false)?;
        rendered_args.push(rendered);
    }
    Ok((rendered_command, rendered_args))
}

fn validate_run_cli_token(value: &str, field: &str, require_single_word: bool) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!(
            "template run_cli {field} resolved to an empty value"
        ));
    }
    if value.contains("{{") || value.contains("}}") {
        return Err(anyhow!(
            "template run_cli {field} contains unresolved placeholders: {value}"
        ));
    }
    if value
        .chars()
        .any(|ch| ch == '\0' || ch == '\n' || ch == '\r' || ch.is_control())
    {
        return Err(anyhow!(
            "template run_cli {field} contains control characters"
        ));
    }
    if require_single_word && value.chars().any(char::is_whitespace) {
        return Err(anyhow!(
            "template run_cli {field} must not contain whitespace"
        ));
    }
    Ok(())
}

fn ask_template_qa_answers<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    template: &ExtensionTemplate,
) -> Result<BTreeMap<String, String>> {
    let mut answers = BTreeMap::new();
    for question in &template.qa_questions {
        let value = ask_catalog_question(
            input,
            output,
            i18n,
            &format!("pack.wizard.create_ext.qa.{}", question.id),
            question,
        )?;
        answers.insert(question.id.clone(), value);
    }
    Ok(answers)
}

fn ask_extension_edit_answers<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    extension_type: &ExtensionType,
) -> Result<BTreeMap<String, String>> {
    let mut answers = BTreeMap::new();
    for question in &extension_type.edit_questions {
        let value = ask_catalog_question(
            input,
            output,
            i18n,
            &format!(
                "pack.wizard.update_ext.edit.{}.{}",
                extension_type.id, question.id
            ),
            question,
        )?;
        answers.insert(question.id.clone(), value);
    }
    Ok(answers)
}

fn ask_catalog_question<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    form_id: &str,
    question: &CatalogQuestion,
) -> Result<String> {
    match question.kind {
        CatalogQuestionKind::Enum => {
            let choices = question
                .choices
                .iter()
                .enumerate()
                .map(|(idx, choice)| ((idx + 1).to_string(), choice.clone()))
                .collect::<Vec<_>>();
            let mut menu = choices
                .iter()
                .map(|(id, label)| (id.clone(), label.clone()))
                .collect::<Vec<_>>();
            menu.push(("0".to_string(), i18n.t("wizard.nav.back")));
            let default_idx = question
                .default
                .as_deref()
                .and_then(|value| {
                    choices
                        .iter()
                        .find(|(_, label)| label == value)
                        .map(|(idx, _)| idx.as_str())
                })
                .unwrap_or("1");
            let selected = ask_enum_custom_labels_owned(
                input,
                output,
                i18n,
                form_id,
                &question.title_key,
                question.description_key.as_deref(),
                &menu,
                default_idx,
            )?;
            if selected == "0" {
                return Ok(question.default.clone().unwrap_or_default());
            }
            choices
                .iter()
                .find(|(idx, _)| idx == &selected)
                .map(|(_, label)| label.clone())
                .ok_or_else(|| anyhow!("invalid enum selection for {}", question.id))
        }
        CatalogQuestionKind::Boolean => {
            let selected = ask_enum(
                input,
                output,
                i18n,
                form_id,
                &question.title_key,
                question.description_key.as_deref(),
                &[
                    ("1", "wizard.bool.true"),
                    ("2", "wizard.bool.false"),
                    ("0", "wizard.nav.back"),
                ],
                if question.default.as_deref() == Some("false") {
                    "2"
                } else {
                    "1"
                },
            )?;
            match selected.as_str() {
                "1" => Ok("true".to_string()),
                "2" => Ok("false".to_string()),
                "0" => Ok(question
                    .default
                    .clone()
                    .unwrap_or_else(|| "false".to_string())),
                _ => Err(anyhow!("invalid boolean selection")),
            }
        }
        CatalogQuestionKind::Integer => loop {
            let value = ask_text(
                input,
                output,
                i18n,
                form_id,
                &question.title_key,
                question.description_key.as_deref(),
                question.default.as_deref(),
            )?;
            if value.trim().parse::<i64>().is_ok() {
                break Ok(value);
            }
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
        },
        CatalogQuestionKind::String => ask_text(
            input,
            output,
            i18n,
            form_id,
            &question.title_key,
            question.description_key.as_deref(),
            question.default.as_deref(),
        ),
    }
}

fn persist_extension_edit_answers(
    pack_dir: &Path,
    extension_type: &ExtensionType,
    answers: &BTreeMap<String, String>,
) -> Result<()> {
    let dir = pack_dir.join("extensions");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{}.json", extension_type.id));
    let payload = json!({
        "extension_type": extension_type.id,
        "answers": answers,
    });
    let bytes =
        serde_json::to_vec_pretty(&payload).context("serialize extension edit answers payload")?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    merge_extension_answers_into_pack_yaml(pack_dir, extension_type, answers)?;
    Ok(())
}

fn merge_extension_answers_into_pack_yaml(
    pack_dir: &Path,
    extension_type: &ExtensionType,
    answers: &BTreeMap<String, String>,
) -> Result<()> {
    let pack_yaml = pack_dir.join("pack.yaml");
    if !pack_yaml.exists() {
        return Ok(());
    }

    let mut cfg = crate::config::load_pack_config(pack_dir)?;
    let key = format!("greentic.wizard.{}.v1", extension_type.id);
    let inline_payload = json!({
        "extension_type": extension_type.id,
        "answers": answers,
    });

    let mut extensions = cfg.extensions.unwrap_or_default();
    extensions.insert(
        key.clone(),
        ExtensionRef {
            kind: key,
            version: "v1".to_string(),
            location: None,
            digest: None,
            inline: Some(ExtensionInline::Other(inline_payload)),
        },
    );
    cfg.extensions = Some(extensions);

    let serialized = serde_yaml_bw::to_string(&cfg).context("serialize updated pack.yaml")?;
    fs::write(&pack_yaml, serialized).with_context(|| format!("write {}", pack_yaml.display()))?;
    Ok(())
}

fn ask_main_menu<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
) -> Result<MainChoice> {
    let choice = ask_enum(
        input,
        output,
        i18n,
        "pack.wizard.main",
        "wizard.main.title",
        Some("wizard.main.description"),
        &[
            ("1", "wizard.main.option.create_application_pack"),
            ("2", "wizard.main.option.update_application_pack"),
            ("3", "wizard.main.option.create_extension_pack"),
            ("4", "wizard.main.option.update_extension_pack"),
            ("0", "wizard.main.option.exit"),
        ],
        "0",
    )?;
    MainChoice::from_choice(&choice)
}

fn ask_placeholder_submenu<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    title_key: &str,
) -> Result<SubmenuAction> {
    let choice = ask_enum(
        input,
        output,
        i18n,
        "pack.wizard.placeholder",
        title_key,
        Some("wizard.shared.not_implemented"),
        &[("0", "wizard.nav.back"), ("M", "wizard.nav.main_menu")],
        "M",
    )?;
    SubmenuAction::from_choice(&choice)
}

fn run_create_application_pack<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
) -> Result<()> {
    let pack_id = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.create_app.pack_id",
        "wizard.create_application_pack.ask_pack_id",
        None,
        None,
    )?;

    let pack_dir_default = format!("./{pack_id}");
    let pack_dir = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.create_app.pack_dir",
        "wizard.create_application_pack.ask_pack_dir",
        Some("wizard.create_application_pack.ask_pack_dir_help"),
        Some(&pack_dir_default),
    )?;

    let pack_dir_path = PathBuf::from(pack_dir.trim());
    let self_exe = wizard_self_exe()?;

    let scaffold_ok = run_process(
        &self_exe,
        &[
            "new",
            "--dir",
            &pack_dir_path.display().to_string(),
            &pack_id,
        ],
        None,
    )?;
    if !scaffold_ok {
        wizard_ui::render_line(output, &i18n.t("wizard.error.create_app_failed"))?;
        let nav = ask_failure_nav(input, output, i18n)?;
        if matches!(nav, SubmenuAction::MainMenu) {
            return Ok(());
        }
        return Ok(());
    }

    loop {
        let setup_choice = ask_enum(
            input,
            output,
            i18n,
            "pack.wizard.create_app.setup",
            "wizard.create_application_pack.setup.title",
            Some("wizard.create_application_pack.setup.description"),
            &[
                (
                    "1",
                    "wizard.create_application_pack.setup.option.edit_flows",
                ),
                (
                    "2",
                    "wizard.create_application_pack.setup.option.add_edit_components",
                ),
                ("3", "wizard.create_application_pack.setup.option.finalize"),
                ("0", "wizard.nav.back"),
                ("M", "wizard.nav.main_menu"),
            ],
            "M",
        )?;

        match setup_choice.as_str() {
            "1" => {
                let delegate_ok = run_delegate("greentic-flow", &["wizard"], &pack_dir_path);
                if !delegate_ok {
                    wizard_ui::render_line(output, &i18n.t("wizard.error.delegate_flow_failed"))?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "2" => {
                let delegate_ok = run_delegate("greentic-component", &["wizard"], &pack_dir_path);
                if !delegate_ok {
                    wizard_ui::render_line(
                        output,
                        &i18n.t("wizard.error.delegate_component_failed"),
                    )?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "3" => {
                if finalize_create_app(input, output, i18n, session, &self_exe, &pack_dir_path)? {
                    return Ok(());
                }
            }
            "0" | "M" | "m" => return Ok(()),
            _ => {
                wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            }
        }
    }
}

fn finalize_create_app<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
    self_exe: &Path,
    pack_dir_path: &Path,
) -> Result<bool> {
    run_update_validate_sequence(
        input,
        output,
        i18n,
        session,
        self_exe,
        pack_dir_path,
        true,
        "wizard.progress.running_finalize",
    )
}

fn run_update_application_pack<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
) -> Result<()> {
    let pack_dir_path = ask_existing_pack_dir(
        input,
        output,
        i18n,
        "pack.wizard.update_app.pack_dir",
        "wizard.update_application_pack.ask_pack_dir",
        Some("wizard.update_application_pack.ask_pack_dir_help"),
        Some("."),
    )?;
    let self_exe = wizard_self_exe()?;

    loop {
        let choice = ask_enum(
            input,
            output,
            i18n,
            "pack.wizard.update_app.menu",
            "wizard.update_application_pack.menu.title",
            Some("wizard.update_application_pack.menu.description"),
            &[
                ("1", "wizard.update_application_pack.menu.option.edit_flows"),
                (
                    "2",
                    "wizard.update_application_pack.menu.option.add_edit_components",
                ),
                (
                    "3",
                    "wizard.update_application_pack.menu.option.run_update_validate",
                ),
                ("4", "wizard.update_application_pack.menu.option.sign"),
                ("0", "wizard.nav.back"),
                ("M", "wizard.nav.main_menu"),
            ],
            "M",
        )?;

        match choice.as_str() {
            "1" => {
                let delegate_ok = run_delegate("greentic-flow", &["wizard"], &pack_dir_path);
                if delegate_ok {
                    let _ = run_update_validate_sequence(
                        input,
                        output,
                        i18n,
                        session,
                        &self_exe,
                        &pack_dir_path,
                        true,
                        "wizard.progress.auto_run_update_validate",
                    )?;
                } else {
                    wizard_ui::render_line(output, &i18n.t("wizard.error.delegate_flow_failed"))?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "2" => {
                let delegate_ok = run_delegate("greentic-component", &["wizard"], &pack_dir_path);
                if delegate_ok {
                    let _ = run_update_validate_sequence(
                        input,
                        output,
                        i18n,
                        session,
                        &self_exe,
                        &pack_dir_path,
                        true,
                        "wizard.progress.auto_run_update_validate",
                    )?;
                } else {
                    wizard_ui::render_line(
                        output,
                        &i18n.t("wizard.error.delegate_component_failed"),
                    )?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "3" => {
                let _ = run_update_validate_sequence(
                    input,
                    output,
                    i18n,
                    session,
                    &self_exe,
                    &pack_dir_path,
                    true,
                    "wizard.progress.running_update_validate",
                )?;
            }
            "4" => {
                let _ = run_sign_for_pack(input, output, i18n, session, &self_exe, &pack_dir_path)?;
            }
            "0" | "M" | "m" => return Ok(()),
            _ => {
                wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            }
        }
    }
}

fn run_update_extension_pack<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
    runtime: Option<&RuntimeContext>,
) -> Result<()> {
    let pack_dir_path = ask_existing_pack_dir(
        input,
        output,
        i18n,
        "pack.wizard.update_ext.pack_dir",
        "wizard.update_extension_pack.ask_pack_dir",
        Some("wizard.update_extension_pack.ask_pack_dir_help"),
        Some("."),
    )?;
    let catalog_ref = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.update_ext.catalog_ref",
        "wizard.update_extension_pack.ask_catalog_ref",
        Some("wizard.update_extension_pack.ask_catalog_ref_help"),
        Some("oci://ghcr.io/greenticai/catalogs/extensions:latest"),
    )?;

    let catalog = match load_extension_catalog(catalog_ref.trim(), runtime) {
        Ok(value) => value,
        Err(err) => {
            wizard_ui::render_line(
                output,
                &format!("{}: {}", i18n.t("wizard.error.catalog_load_failed"), err),
            )?;
            let nav = ask_failure_nav(input, output, i18n)?;
            if matches!(nav, SubmenuAction::MainMenu) {
                return Ok(());
            }
            return Ok(());
        }
    };

    let self_exe = wizard_self_exe()?;

    loop {
        let choice = ask_enum(
            input,
            output,
            i18n,
            "pack.wizard.update_ext.menu",
            "wizard.update_extension_pack.menu.title",
            Some("wizard.update_extension_pack.menu.description"),
            &[
                ("1", "wizard.update_extension_pack.menu.option.edit_entries"),
                ("2", "wizard.update_extension_pack.menu.option.edit_flows"),
                (
                    "3",
                    "wizard.update_extension_pack.menu.option.add_edit_components",
                ),
                (
                    "4",
                    "wizard.update_extension_pack.menu.option.run_update_validate",
                ),
                ("5", "wizard.update_extension_pack.menu.option.sign"),
                ("0", "wizard.nav.back"),
                ("M", "wizard.nav.main_menu"),
            ],
            "M",
        )?;

        match choice.as_str() {
            "1" => {
                let type_choice = ask_extension_type(input, output, i18n, &catalog)?;
                if type_choice == "0" || type_choice.eq_ignore_ascii_case("m") {
                    continue;
                }
                let selected = catalog
                    .extension_types
                    .iter()
                    .find(|item| item.id == type_choice)
                    .ok_or_else(|| anyhow!("selected extension type not found"))?;
                let answers = ask_extension_edit_answers(input, output, i18n, selected)?;
                persist_extension_edit_answers(&pack_dir_path, selected, &answers)?;
                wizard_ui::render_line(
                    output,
                    &format!(
                        "{} {}",
                        i18n.t("wizard.update_extension_pack.edited_entry"),
                        type_choice
                    ),
                )?;
            }
            "2" => {
                let delegate_ok = run_delegate("greentic-flow", &["wizard"], &pack_dir_path);
                if !delegate_ok {
                    wizard_ui::render_line(output, &i18n.t("wizard.error.delegate_flow_failed"))?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "3" => {
                let delegate_ok = run_delegate("greentic-component", &["wizard"], &pack_dir_path);
                if !delegate_ok {
                    wizard_ui::render_line(
                        output,
                        &i18n.t("wizard.error.delegate_component_failed"),
                    )?;
                    if matches!(
                        ask_failure_nav(input, output, i18n)?,
                        SubmenuAction::MainMenu
                    ) {
                        return Ok(());
                    }
                }
            }
            "4" => {
                let _ = run_update_validate_sequence(
                    input,
                    output,
                    i18n,
                    session,
                    &self_exe,
                    &pack_dir_path,
                    true,
                    "wizard.progress.running_update_validate",
                )?;
            }
            "5" => {
                let _ = run_sign_for_pack(input, output, i18n, session, &self_exe, &pack_dir_path)?;
            }
            "0" | "M" | "m" => return Ok(()),
            _ => {
                wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            }
        }
    }
}

fn run_update_validate_sequence<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
    self_exe: &Path,
    pack_dir_path: &Path,
    prompt_sign_after: bool,
    progress_key: &str,
) -> Result<bool> {
    wizard_ui::render_line(output, &i18n.t(progress_key))?;
    wizard_ui::render_line(output, &i18n.t("wizard.progress.running_doctor"))?;
    let doctor_ok = run_process(
        self_exe,
        &["doctor", "--in", &pack_dir_path.display().to_string()],
        None,
    )?;
    if !doctor_ok {
        wizard_ui::render_line(output, &i18n.t("wizard.error.finalize_doctor_failed"))?;
        return Ok(false);
    }

    wizard_ui::render_line(output, &i18n.t("wizard.progress.running_build"))?;
    let build_ok = run_process(
        self_exe,
        &["build", "--in", &pack_dir_path.display().to_string()],
        None,
    )?;
    if !build_ok {
        wizard_ui::render_line(output, &i18n.t("wizard.error.finalize_build_failed"))?;
        return Ok(false);
    }

    if prompt_sign_after {
        run_sign_prompt_after_finalize(input, output, i18n, session, self_exe, pack_dir_path)
    } else {
        Ok(true)
    }
}

fn run_sign_prompt_after_finalize<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
    self_exe: &Path,
    pack_dir_path: &Path,
) -> Result<bool> {
    let sign_choice = ask_enum(
        input,
        output,
        i18n,
        "pack.wizard.sign_prompt",
        "wizard.sign.after_finalize.title",
        Some("wizard.sign.after_finalize.description"),
        &[
            ("1", "wizard.sign.after_finalize.option.sign_now"),
            ("2", "wizard.sign.after_finalize.option.skip"),
            ("0", "wizard.nav.back"),
            ("M", "wizard.nav.main_menu"),
        ],
        "2",
    )?;

    match sign_choice.as_str() {
        "2" => Ok(true),
        "M" | "m" => Ok(true),
        "0" => Ok(false),
        "1" => run_sign_for_pack(input, output, i18n, session, self_exe, pack_dir_path),
        _ => {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            Ok(false)
        }
    }
}

fn run_sign_for_pack<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    session: &mut WizardSession,
    self_exe: &Path,
    pack_dir_path: &Path,
) -> Result<bool> {
    let key_path = ask_text(
        input,
        output,
        i18n,
        "pack.wizard.sign_key_path",
        "wizard.sign.ask_key_path",
        None,
        session.sign_key_path.as_deref(),
    )?;
    let sign_ok = run_process(
        self_exe,
        &[
            "sign",
            "--pack",
            &pack_dir_path.display().to_string(),
            "--key",
            &key_path,
        ],
        None,
    )?;
    if !sign_ok {
        wizard_ui::render_line(output, &i18n.t("wizard.error.sign_failed"))?;
        return Ok(false);
    }
    session.sign_key_path = Some(key_path);
    Ok(true)
}

fn ask_failure_nav<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
) -> Result<SubmenuAction> {
    let choice = ask_enum(
        input,
        output,
        i18n,
        "pack.wizard.failure_nav",
        "wizard.failure_nav.title",
        Some("wizard.failure_nav.description"),
        &[("0", "wizard.nav.back"), ("M", "wizard.nav.main_menu")],
        "0",
    )?;
    SubmenuAction::from_choice(&choice)
}

fn ask_enum<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    form_id: &str,
    title_key: &str,
    description_key: Option<&str>,
    choices: &[(&str, &str)],
    default_on_eof: &str,
) -> Result<String> {
    let mut question = json!({
        "id": "choice",
        "type": "enum",
        "title": i18n.t(title_key),
        "title_i18n": {"key": title_key},
        "required": true,
        "choices": choices.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
    });
    if let Some(description_key) = description_key {
        question["description"] = Value::String(i18n.t(description_key));
        question["description_i18n"] = json!({"key": description_key});
    }

    let spec = json!({
        "id": form_id,
        "title": i18n.t(title_key),
        "version": "1.0.0",
        "description": description_key.map(|key| i18n.t(key)).unwrap_or_default(),
        "progress_policy": {
            "skip_answered": true,
            "autofill_defaults": false,
            "treat_default_as_answered": false,
        },
        "questions": [question],
    });
    let config = WizardRunConfig {
        spec_json: serde_json::to_string(&spec).context("serialize enum QA spec")?,
        initial_answers_json: None,
        frontend: WizardFrontend::Text,
        i18n: i18n.qa_i18n_config(),
        verbose: false,
    };

    let mut driver = WizardDriver::new(config).context("initialize QA enum driver")?;
    loop {
        let payload_raw = driver
            .next_payload_json()
            .context("render QA enum payload")?;
        let payload: Value = serde_json::from_str(&payload_raw).context("parse QA enum payload")?;
        if let Some(text) = payload.get("text").and_then(Value::as_str) {
            render_driver_text(output, text)?;
        }

        if driver.is_complete() {
            break;
        }

        for (value, key) in choices {
            wizard_ui::render_line(output, &format!("{value}) {}", i18n.t(key)))?;
        }

        wizard_ui::render_prompt(output, &i18n.t("wizard.prompt"))?;
        let Some(line) = read_trimmed_line(input)? else {
            return Ok(default_on_eof.to_string());
        };
        let candidate = if line.eq_ignore_ascii_case("m") {
            "M".to_string()
        } else {
            line
        };
        if !choices
            .iter()
            .map(|(value, _)| *value)
            .any(|value| value.eq_ignore_ascii_case(&candidate))
        {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            continue;
        }

        let submit = driver
            .submit_patch_json(&json!({"choice": candidate}).to_string())
            .context("submit QA enum answer")?;
        if submit.status == "error" {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
        }
    }

    let result = driver.finish().context("finish QA enum")?;
    result
        .answer_set
        .answers
        .get("choice")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("missing enum answer"))
}

fn ask_enum_custom_labels_owned<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    form_id: &str,
    title_key: &str,
    description_key: Option<&str>,
    choices: &[(String, String)],
    default_on_eof: &str,
) -> Result<String> {
    let mut question = json!({
        "id": "choice",
        "type": "enum",
        "title": i18n.t(title_key),
        "title_i18n": {"key": title_key},
        "required": true,
        "choices": choices.iter().map(|(v, _)| v).collect::<Vec<_>>(),
    });
    if let Some(description_key) = description_key {
        question["description"] = Value::String(i18n.t(description_key));
        question["description_i18n"] = json!({"key": description_key});
    }

    let spec = json!({
        "id": form_id,
        "title": i18n.t(title_key),
        "version": "1.0.0",
        "description": description_key.map(|key| i18n.t(key)).unwrap_or_default(),
        "progress_policy": {
            "skip_answered": true,
            "autofill_defaults": false,
            "treat_default_as_answered": false,
        },
        "questions": [question],
    });
    let config = WizardRunConfig {
        spec_json: serde_json::to_string(&spec).context("serialize custom enum QA spec")?,
        initial_answers_json: None,
        frontend: WizardFrontend::Text,
        i18n: i18n.qa_i18n_config(),
        verbose: false,
    };

    let mut driver = WizardDriver::new(config).context("initialize QA custom enum driver")?;
    loop {
        let payload_raw = driver
            .next_payload_json()
            .context("render QA custom enum payload")?;
        let payload: Value =
            serde_json::from_str(&payload_raw).context("parse QA custom enum payload")?;
        if let Some(text) = payload.get("text").and_then(Value::as_str) {
            render_driver_text(output, text)?;
        }

        if driver.is_complete() {
            break;
        }

        for (value, label) in choices {
            wizard_ui::render_line(output, &format!("{value}) {label}"))?;
        }

        wizard_ui::render_prompt(output, &i18n.t("wizard.prompt"))?;
        let Some(line) = read_trimmed_line(input)? else {
            return Ok(default_on_eof.to_string());
        };
        let candidate = if line.eq_ignore_ascii_case("m") {
            "M".to_string()
        } else {
            line
        };
        if !choices
            .iter()
            .map(|(value, _)| value.as_str())
            .any(|value| value.eq_ignore_ascii_case(&candidate))
        {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
            continue;
        }

        let submit = driver
            .submit_patch_json(&json!({"choice": candidate}).to_string())
            .context("submit QA custom enum answer")?;
        if submit.status == "error" {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
        }
    }

    let result = driver.finish().context("finish QA custom enum")?;
    result
        .answer_set
        .answers
        .get("choice")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("missing custom enum answer"))
}

fn ask_text<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    form_id: &str,
    title_key: &str,
    description_key: Option<&str>,
    default_value: Option<&str>,
) -> Result<String> {
    let mut question = json!({
        "id": "value",
        "type": "string",
        "title": i18n.t(title_key),
        "title_i18n": {"key": title_key},
        "required": true,
    });
    if let Some(description_key) = description_key {
        question["description"] = Value::String(i18n.t(description_key));
        question["description_i18n"] = json!({"key": description_key});
    }
    if let Some(default_value) = default_value {
        question["default_value"] = Value::String(default_value.to_string());
    }

    let spec = json!({
        "id": form_id,
        "title": i18n.t(title_key),
        "version": "1.0.0",
        "description": description_key.map(|key| i18n.t(key)).unwrap_or_default(),
        "progress_policy": {
            "skip_answered": true,
            "autofill_defaults": false,
            "treat_default_as_answered": false,
        },
        "questions": [question],
    });
    let config = WizardRunConfig {
        spec_json: serde_json::to_string(&spec).context("serialize text QA spec")?,
        initial_answers_json: None,
        frontend: WizardFrontend::Text,
        i18n: i18n.qa_i18n_config(),
        verbose: false,
    };

    let mut driver = WizardDriver::new(config).context("initialize QA text driver")?;
    loop {
        let payload_raw = driver
            .next_payload_json()
            .context("render QA text payload")?;
        let payload: Value = serde_json::from_str(&payload_raw).context("parse QA text payload")?;
        if let Some(text) = payload.get("text").and_then(Value::as_str) {
            render_driver_text(output, text)?;
        }

        if driver.is_complete() {
            break;
        }

        wizard_ui::render_prompt(output, &i18n.t("wizard.prompt"))?;
        let Some(line) = read_trimmed_line(input)? else {
            if let Some(default) = default_value {
                return Ok(default.to_string());
            }
            return Err(anyhow!("missing text input"));
        };

        let answer = if line.trim().is_empty() {
            default_value.unwrap_or_default().to_string()
        } else {
            line
        };
        let submit = driver
            .submit_patch_json(&json!({"value": answer}).to_string())
            .context("submit QA text answer")?;
        if submit.status == "error" {
            wizard_ui::render_line(output, &i18n.t("wizard.error.invalid_selection"))?;
        }
    }

    let result = driver.finish().context("finish QA text")?;
    result
        .answer_set
        .answers
        .get("value")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("missing text answer"))
}

fn ask_existing_pack_dir<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    i18n: &WizardI18n,
    form_id: &str,
    title_key: &str,
    description_key: Option<&str>,
    default_value: Option<&str>,
) -> Result<PathBuf> {
    loop {
        let pack_dir = ask_text(
            input,
            output,
            i18n,
            form_id,
            title_key,
            description_key,
            default_value,
        )?;
        let candidate = PathBuf::from(pack_dir.trim());
        if candidate.is_dir() {
            return Ok(candidate);
        }
        wizard_ui::render_line(
            output,
            &format!(
                "{}: {}",
                i18n.t("wizard.error.invalid_pack_dir"),
                candidate.display()
            ),
        )?;
    }
}

fn run_process(binary: &Path, args: &[&str], cwd: Option<&Path>) -> Result<bool> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", binary.display()))?;
    Ok(status.success())
}

fn run_delegate(binary: &str, args: &[&str], cwd: &Path) -> bool {
    if let Some(current_exe) = std::env::current_exe().ok()
        && let Some(exe_dir) = current_exe.parent()
    {
        let local_bin = exe_dir.join(binary);
        if local_bin.exists() {
            return run_process(&local_bin, args, Some(cwd)).unwrap_or(false);
        }
    }

    Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn wizard_self_exe() -> Result<PathBuf> {
    if let Ok(path) = env::var("GREENTIC_PACK_WIZARD_SELF_EXE") {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Ok(candidate);
        }
        return Err(anyhow!(
            "GREENTIC_PACK_WIZARD_SELF_EXE does not exist: {}",
            candidate.display()
        ));
    }
    std::env::current_exe().context("resolve current executable")
}

fn read_trimmed_line<R: BufRead>(input: &mut R) -> Result<Option<String>> {
    let mut line = String::new();
    let read = input.read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

fn render_driver_text<W: Write>(output: &mut W, text: &str) -> Result<()> {
    let filtered = filter_driver_boilerplate(text);
    if filtered.trim().is_empty() {
        return Ok(());
    }
    wizard_ui::render_text(output, &filtered)?;
    if !filtered.ends_with('\n') {
        wizard_ui::render_text(output, "\n")?;
    }
    Ok(())
}

fn filter_driver_boilerplate(text: &str) -> String {
    let mut kept = Vec::new();
    let mut skipping_visible_block = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(title) = trimmed.strip_prefix("Title:") {
            let title = title.trim();
            if !title.is_empty() {
                kept.push(title);
            }
            continue;
        }
        if trimmed.starts_with("Description:") || trimmed.starts_with("Required:") {
            continue;
        }
        if trimmed == "All visible questions are answered." {
            continue;
        }
        if trimmed.starts_with("Form:")
            || trimmed.starts_with("Status:")
            || trimmed.starts_with("Help:")
            || trimmed.starts_with("Next question:")
        {
            skipping_visible_block = false;
            continue;
        }
        if trimmed.starts_with("Visible questions:") {
            skipping_visible_block = true;
            continue;
        }
        if skipping_visible_block {
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                continue;
            }
            if trimmed.is_empty() {
                continue;
            }
            skipping_visible_block = false;
        }
        kept.push(line);
    }
    let joined = kept.join("\n");
    joined.trim_matches('\n').to_string()
}

impl SubmenuAction {
    fn from_choice(choice: &str) -> Result<Self> {
        if choice == "0" {
            return Ok(Self::Back);
        }
        if choice.eq_ignore_ascii_case("m") {
            return Ok(Self::MainMenu);
        }
        Err(anyhow!("invalid submenu selection `{choice}`"))
    }
}

impl MainChoice {
    fn from_choice(choice: &str) -> Result<Self> {
        match choice {
            "1" => Ok(Self::CreateApplicationPack),
            "2" => Ok(Self::UpdateApplicationPack),
            "3" => Ok(Self::CreateExtensionPack),
            "4" => Ok(Self::UpdateExtensionPack),
            "0" => Ok(Self::Exit),
            _ => Err(anyhow!("invalid main selection `{choice}`")),
        }
    }
}
