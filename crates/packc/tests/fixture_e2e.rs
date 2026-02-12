use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use greentic_types::cbor::canonical;
use greentic_types::i18n_text::I18nText;
use greentic_types::qa::QaSpecSource;
use greentic_types::schemas::pack::v0_6_0::PackDescribe;
use greentic_types::schemas::pack::v0_6_0::qa::{
    PackQaSpec, QaMode as PackQaMode, Question, QuestionKind,
};
use packc::cli::inspect::{InspectArgs, InspectFormat, handle as inspect_handle};
use packc::cli::qa::{QaArgs, QaModeLabel, handle as qa_handle};
use packc::cli::wizard::{AddComponentArgs, NewAppArgs, WizardCommand, handle as wizard_handle};
use packc::runtime;
use tempfile::TempDir;

fn write_pack_qa_spec(pack_dir: &Path) {
    let qa_dir = pack_dir.join("qa/pack");
    fs::create_dir_all(&qa_dir).expect("qa dir");

    let mut defaults = BTreeMap::new();
    defaults.insert(
        "region".to_string(),
        ciborium::value::Value::Text("us".to_string()),
    );

    let spec = PackQaSpec {
        mode: PackQaMode::Default,
        title: I18nText::new("pack.qa.title", Some("Pack QA".to_string())),
        description: None,
        questions: vec![Question {
            id: "region".to_string(),
            label: I18nText::new("pack.qa.region", Some("Region".to_string())),
            help: None,
            error: None,
            kind: QuestionKind::Text,
            required: true,
            default: None,
        }],
        defaults,
    };
    let bytes = canonical::to_canonical_cbor_allow_floats(&spec).expect("spec bytes");
    fs::write(qa_dir.join("default.cbor"), bytes).expect("write pack QA");

    let pack_cbor = pack_dir.join("pack.cbor");
    let bytes = fs::read(&pack_cbor).expect("read pack.cbor");
    let mut describe: PackDescribe = canonical::from_cbor(&bytes).expect("decode pack.cbor");

    let source = QaSpecSource::RefPackPath("qa/pack/default.cbor".to_string());
    let source_bytes = canonical::to_canonical_cbor_allow_floats(&source).expect("source bytes");
    let source_value: ciborium::value::Value =
        canonical::from_cbor(&source_bytes).expect("source value");

    describe
        .metadata
        .insert("greentic.qa".to_string(), source_value);
    let updated = canonical::to_canonical_cbor_allow_floats(&describe).expect("encode pack.cbor");
    fs::write(&pack_cbor, updated).expect("write pack.cbor");
}

fn write_pack_yaml(pack_dir: &Path, pack_id: &str) {
    let contents = format!(
        r#"pack_id: {pack_id}
version: 0.1.0
kind: application
publisher: Greentic

components: []
dependencies: []
flows: []
assets: []
"#
    );
    fs::write(pack_dir.join("pack.yaml"), contents).expect("write pack.yaml");
}

#[test]
fn offline_fixture_e2e_wizard_qa_doctor() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let runtime = runtime::resolve_runtime(None, None, true, None).expect("runtime");

    let new_args = NewAppArgs {
        pack_id: "dev.local.demo".to_string(),
        out: pack_dir.clone(),
        locale: "en".to_string(),
        name: Some("Demo".to_string()),
    };
    wizard_handle(WizardCommand::NewApp(new_args), &runtime).expect("wizard new-app");
    write_pack_yaml(&pack_dir, "dev.local.demo");

    write_pack_qa_spec(&pack_dir);

    let qa_args = QaArgs {
        pack_dir: pack_dir.clone(),
        mode: QaModeLabel::Default,
        answers: None,
        locale: "en".to_string(),
        non_interactive: true,
        reask: false,
        components: Vec::new(),
        all_locked: false,
        pack_only: true,
    };
    qa_handle(qa_args, &runtime).expect("pack QA");

    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/registry");
    unsafe {
        std::env::set_var("GREENTIC_PACK_FIXTURE_DIR", fixture_root);
    }

    let add_args = AddComponentArgs {
        reference: "fixture://dev.local.component".to_string(),
        pack_dir: pack_dir.clone(),
        use_describe_cache: true,
        force: false,
        dry_run: false,
    };
    wizard_handle(WizardCommand::AddComponent(add_args), &runtime).expect("wizard add-component");

    let inspect_args = InspectArgs {
        path: None,
        pack: None,
        input: Some(pack_dir.clone()),
        archive: false,
        source: true,
        allow_oci_tags: false,
        flow_doctor: true,
        component_doctor: true,
        format: InspectFormat::Json,
        validate: true,
        no_validate: false,
        validators_root: PathBuf::from(".greentic/validators"),
        validator_pack: Vec::new(),
        validator_wasm: Vec::new(),
        validator_allow: vec!["oci://".to_string()],
        validator_cache_dir: PathBuf::from(".greentic/cache/validators"),
        validator_policy: packc::validator::ValidatorPolicy::Optional,
        online: false,
        use_describe_cache: true,
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio");
    let result = rt.block_on(inspect_handle(inspect_args, true, &runtime));
    assert!(
        result.is_err(),
        "doctor should report errors for fixture wasm"
    );
}
