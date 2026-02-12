use std::fs;
use std::path::PathBuf;

use greentic_types::cbor::canonical;
use greentic_types::schemas::pack::v0_6_0::PackDescribe;
use packc::cli::wizard::{NewAppArgs, NewExtensionArgs, WizardCommand, handle as wizard_handle};
use packc::runtime;
use tempfile::TempDir;

fn read_bytes(path: PathBuf) -> Vec<u8> {
    fs::read(path).expect("read file")
}

#[test]
fn wizard_new_app_writes_pack_cbor_and_i18n() {
    let temp = TempDir::new().expect("temp dir");
    let out = temp.path().join("app-pack");
    let args = NewAppArgs {
        pack_id: "dev.local.app".to_string(),
        out: out.clone(),
        locale: "en".to_string(),
        name: Some("Demo App".to_string()),
    };

    let runtime = runtime::resolve_runtime(None, None, true, None).expect("runtime");
    wizard_handle(WizardCommand::NewApp(args), &runtime).expect("wizard new-app");

    let pack_bytes = read_bytes(out.join("pack.cbor"));
    let describe: PackDescribe = canonical::from_cbor(&pack_bytes).expect("decode pack.cbor");
    assert_eq!(describe.info.id, "dev.local.app");
    assert_eq!(describe.info.role, "application");
    assert!(describe.info.display_name.is_some());

    let i18n_path = out.join("assets").join("i18n").join("en.json");
    let i18n_json = fs::read_to_string(i18n_path).expect("read i18n");
    assert!(i18n_json.contains("pack.name"));
    assert!(i18n_json.contains("Demo App"));
}

#[test]
fn wizard_new_extension_is_deterministic() {
    let temp = TempDir::new().expect("temp dir");
    let out_a = temp.path().join("ext-pack-a");
    let out_b = temp.path().join("ext-pack-b");

    let args_a = NewExtensionArgs {
        pack_id: "dev.local.ext".to_string(),
        kind: "provider".to_string(),
        out: out_a.clone(),
        locale: "en".to_string(),
        name: None,
    };
    let args_b = NewExtensionArgs {
        pack_id: "dev.local.ext".to_string(),
        kind: "provider".to_string(),
        out: out_b.clone(),
        locale: "en".to_string(),
        name: None,
    };

    let runtime = runtime::resolve_runtime(None, None, true, None).expect("runtime");
    wizard_handle(WizardCommand::NewExtension(args_a), &runtime).expect("wizard new-extension a");
    wizard_handle(WizardCommand::NewExtension(args_b), &runtime).expect("wizard new-extension b");

    assert!(out_a.join("extensions").join("provider").exists());
    assert!(out_b.join("extensions").join("provider").exists());

    let pack_a = read_bytes(out_a.join("pack.cbor"));
    let pack_b = read_bytes(out_b.join("pack.cbor"));
    assert_eq!(pack_a, pack_b, "pack.cbor should be deterministic");

    let i18n_a = read_bytes(out_a.join("assets").join("i18n").join("en.json"));
    let i18n_b = read_bytes(out_b.join("assets").join("i18n").join("en.json"));
    assert_eq!(i18n_a, i18n_b, "i18n bundle should be deterministic");
}
