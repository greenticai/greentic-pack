use std::io::Cursor;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use std::{sync::Mutex, sync::OnceLock};

use packc::cli::wizard;
use tempfile::TempDir;

#[test]
fn wizard_boots_and_exits_with_zero() {
    let mut input = Cursor::new(b"0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_with_io(&mut input, &mut output).expect("wizard should exit cleanly");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Main Menu"));
    assert!(rendered.contains("0) Exit"));
}

#[test]
fn wizard_submenu_supports_back_and_main_menu() {
    let mut input = Cursor::new(b"1\n0\n4\nM\n0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_with_io(&mut input, &mut output).expect("wizard navigation should work");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Create application pack"));
    assert!(rendered.contains("Update extension pack"));
    assert!(rendered.contains("0) Back"));
    assert!(rendered.contains("M) Main Menu"));
}

#[test]
fn wizard_i18n_smoke_en_gb_has_no_missing_keys() {
    let mut input = Cursor::new(b"0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard should render en-GB bundle");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(!rendered.contains("??wizard."));
    assert!(rendered.contains("Main Menu"));
}

#[test]
fn wizard_create_extension_catalog_fixture_renders_type_explanations() {
    let mut input = Cursor::new(b"3\nfixture://extensions.json\n1\n0\n0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard cli extension flow should run");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Choose extension type"));
    assert!(rendered.contains("Messaging - Messaging channels and connectors"));
    assert!(rendered.contains("Custom extension - Scaffold only"));
}

#[test]
fn wizard_update_flow_auto_runs_validate_after_delegate_success() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    fs::write(&log_path, "").expect("seed call log");
    let delegate = temp.path().join("greentic-flow");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &delegate,
        &format!(
            "#!/usr/bin/env bash\necho \"flow:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    let old_path = std::env::var("PATH").ok();
    let new_path = match &old_path {
        Some(value) => format!("{}:{value}", temp.path().display()),
        None => temp.path().display().to_string(),
    };

    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let mut input = Cursor::new(b"2\n.\n1\n2\nM\n0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard cli flow should run");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("flow:wizard"));
    assert!(calls.contains("self:doctor --in ."));
    assert!(calls.contains("self:build --in ."));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_flow_delegate_failure_does_not_auto_run_validate() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let delegate = temp.path().join("greentic-flow");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &delegate,
        &format!(
            "#!/usr/bin/env bash\necho \"flow:$*\" >> \"{}\"\nexit 1\n",
            log_path.display()
        ),
    );

    let old_path = std::env::var("PATH").ok();
    let new_path = match &old_path {
        Some(value) => format!("{}:{value}", temp.path().display()),
        None => temp.path().display().to_string(),
    };

    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let mut input = Cursor::new(b"2\n.\n1\n0\nM\n0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard cli flow should run");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("flow:wizard"));
    assert!(!calls.contains("self:doctor --in ."));
    assert!(!calls.contains("self:build --in ."));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

fn test_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn wizard_create_extension_custom_scaffold_creates_expected_files() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let pack_dir = temp.path().join("custom-ext");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    unsafe {
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "3\nfixture://extensions.json\n10\n1\n{}\n\n2\n0\n",
        pack_dir.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard cli extension scaffold should run");

    assert!(pack_dir.join("README.md").exists());
    assert!(pack_dir.join("pack.yaml").exists());
    assert!(pack_dir.join("flows/main.ygtc").exists());
    assert!(pack_dir.join("components").exists());
    assert!(pack_dir.join("i18n").exists());

    let readme = fs::read_to_string(pack_dir.join("README.md")).expect("read README");
    assert!(readme.contains("Canonical key: greentic.provider-extension.v1"));

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("self:doctor --in"));
    assert!(calls.contains("self:build --in"));

    unsafe {
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_create_extension_template_delegate_step_runs_flow_wizard() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");
    let pack_dir = temp.path().join("messaging-ext");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &flow_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"flow:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    let old_path = std::env::var("PATH").ok();
    let new_path = match &old_path {
        Some(value) => format!("{}:{value}", temp.path().display()),
        None => temp.path().display().to_string(),
    };

    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "3\nfixture://extensions.json\n1\n1\n{}\n\n2\n0\n",
        pack_dir.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard create extension delegate should run");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("flow:wizard"));
    assert!(calls.contains("self:doctor --in"));
    assert!(calls.contains("self:build --in"));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_extension_edit_entries_persists_catalog_answers() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let self_exe = temp.path().join("greentic-pack-self");
    let pack_dir = temp.path().join("update-ext");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(
        pack_dir.join("pack.yaml"),
        "pack_id: update.ext\nversion: 0.1.0\nkind: application\npublisher: Greentic\ncomponents: []\nflows: []\ndependencies: []\nassets: []\n",
    )
    .expect("write pack.yaml");

    unsafe {
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "4\n{}\nfixture://extensions.json\n1\n10\nmy-custom-entry\n0\n0\n",
        pack_dir.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard update extension edit should run");

    let persisted = pack_dir.join("extensions/custom-scaffold.json");
    assert!(persisted.exists(), "edited entry file should exist");
    let body = fs::read_to_string(persisted).expect("read persisted entry");
    assert!(body.contains("\"entry_label\": \"my-custom-entry\""));
    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack.yaml");
    assert!(pack_yaml.contains("greentic.wizard.custom-scaffold.v1"));
    assert!(pack_yaml.contains("my-custom-entry"));

    unsafe {
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_extension_flow_runs_validate_pipeline() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    unsafe {
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let mut input = Cursor::new(b"4\n.\nfixture://extensions.json\n4\n2\n0\n0\n".to_vec());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard update extension flow should run");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    let doctor_idx = calls
        .find("self:doctor --in .")
        .expect("doctor call missing");
    let build_idx = calls.find("self:build --in .").expect("build call missing");
    assert!(doctor_idx < build_idx, "doctor should run before build");

    unsafe {
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_app_flow_missing_delegate_binary_shows_error_and_stays_in_menu() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    fs::write(&log_path, "").expect("seed call log");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    let old_path = std::env::var("PATH").ok();
    unsafe {
        std::env::set_var("PATH", "");
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let mut input = Cursor::new(b"2\n.\n1\n0\n0\n0\n".to_vec());
    let mut output = Vec::new();
    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard should continue after missing delegate binary");
    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Failed to run greentic-flow wizard."));

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(!calls.contains("self:doctor --in ."));
    assert!(!calls.contains("self:build --in ."));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_extension_missing_component_delegate_binary_shows_error() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let self_exe = temp.path().join("greentic-pack-self");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");

    let old_path = std::env::var("PATH").ok();
    unsafe {
        std::env::set_var("PATH", "");
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let mut input = Cursor::new(b"4\n.\nfixture://extensions.json\n3\n0\n0\n".to_vec());
    let mut output = Vec::new();
    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard should continue after missing component delegate binary");
    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Failed to run greentic-component wizard."));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_update_app_reprompts_when_pack_dir_is_invalid() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let valid_pack = temp.path().join("valid-pack");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::create_dir_all(&valid_pack).expect("create valid pack dir");
    fs::write(
        valid_pack.join("pack.yaml"),
        "pack_id: valid.pack\nversion: 0.1.0\nkind: application\npublisher: Greentic\ncomponents: []\nflows: []\ndependencies: []\nassets: []\n",
    )
    .expect("write valid pack yaml");

    unsafe {
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "2\n/tmp/does-not-exist\n{}\n3\n2\n0\n",
        valid_pack.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();
    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard should reprompt and continue with valid pack dir");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Invalid pack directory"));

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains(&format!("self:doctor --in {}", valid_pack.display())));
    assert!(calls.contains(&format!("self:build --in {}", valid_pack.display())));

    unsafe {
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_create_extension_run_cli_step_interpolates_qa_answers() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let hook_exe = temp.path().join("greentic-runcli-hook");
    let pack_dir = temp.path().join("runcli-hook-ext");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"self:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &hook_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"hook:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );

    let old_path = std::env::var("PATH").ok();
    let new_path = match &old_path {
        Some(value) => format!("{}:{value}", temp.path().display()),
        None => temp.path().display().to_string(),
    };

    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "3\nfixture://extensions.json\n11\n1\n{}\nmy-hook-name\n2\n0\n",
        pack_dir.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard run_cli interpolation flow should run");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("hook:--label my-hook-name"));
    assert!(calls.contains("self:doctor --in"));
    assert!(calls.contains("self:build --in"));

    unsafe {
        if let Some(value) = old_path {
            std::env::set_var("PATH", value);
        } else {
            std::env::remove_var("PATH");
        }
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}

#[test]
fn wizard_create_extension_run_cli_step_rejects_unresolved_placeholders() {
    let _guard = test_env_lock().lock().expect("env lock");
    let temp = TempDir::new().expect("tempdir");
    let self_exe = temp.path().join("greentic-pack-self");
    let pack_dir = temp.path().join("runcli-invalid-ext");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");
    unsafe {
        std::env::set_var("GREENTIC_PACK_WIZARD_SELF_EXE", self_exe.as_os_str());
    }

    let input_script = format!(
        "3\nfixture://extensions.json\n12\n1\n{}\n0\n0\n",
        pack_dir.display()
    );
    let mut input = Cursor::new(input_script.into_bytes());
    let mut output = Vec::new();

    wizard::run_cli_with_io_and_locale(&mut input, &mut output, Some("en-GB"))
        .expect("wizard should show localized apply error and continue");
    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Failed to apply extension template"));

    unsafe {
        std::env::remove_var("GREENTIC_PACK_WIZARD_SELF_EXE");
    }
}
