use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;
use walkdir::WalkDir;

/// Assert a `greentic-pack` subprocess succeeded, surfacing its exit code,
/// stdout, and stderr on failure. The bare `assert!(output.status.success(),
/// "…")` form reports nothing when the subprocess fails, which makes the
/// occasional environmental CI flake in these binary-driven tests impossible
/// to diagnose after the fact.
fn assert_cli_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Build a `greentic-pack` command with the wizard delegate overrides cleared.
///
/// `GREENTIC_PACK_WIZARD_SELF_EXE`, `GREENTIC_FLOW_BIN`, and `GREENTIC_COMPONENT_BIN`
/// point the wizard at stand-in delegate binaries. They must only ever be set on the
/// child command: setting them on the test process leaks them into every other test
/// spawned concurrently, which then delegates to a stub living in a sibling test's
/// `TempDir` — already deleted by the time it runs.
fn pack_cmd() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"));
    cmd.env_remove("GREENTIC_PACK_WIZARD_SELF_EXE")
        .env_remove("GREENTIC_FLOW_BIN")
        .env_remove("GREENTIC_COMPONENT_BIN");
    cmd
}

#[test]
fn wizard_run_emit_answers_writes_envelope() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("answers.json");
    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--schema-version")
        .arg("1.2.3")
        .arg("--emit-answers")
        .arg(&answers_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard run");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"0\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "wizard run should succeed");

    let doc: Value =
        serde_json::from_slice(&fs::read(&answers_path).expect("read emitted answers"))
            .expect("parse emitted answers");
    assert_eq!(
        doc.get("wizard_id").and_then(Value::as_str),
        Some("greentic-pack.wizard.run")
    );
    assert_eq!(
        doc.get("schema_id").and_then(Value::as_str),
        Some("greentic-pack.wizard.answers")
    );
    assert_eq!(
        doc.get("schema_version").and_then(Value::as_str),
        Some("1.2.3")
    );
    assert!(doc.get("answers").and_then(Value::as_object).is_some());
    assert!(doc.get("locks").and_then(Value::as_object).is_some());
}

#[test]
fn wizard_run_emit_answers_records_extension_operation() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("extension_answers.json");
    let pack_dir = temp.path().join("control-pack");
    let input = format!(
        "3\nfixture://extensions.json\n7\n1\n{}\n\n\ncontrol-entry\n0\n\n\n\n\n0\n0\n\n2\n0\n",
        pack_dir.display()
    );

    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard run");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "wizard run should succeed");

    let doc: Value = serde_json::from_slice(&fs::read(&answers_path).expect("read answers"))
        .expect("parse answers");
    let answers = doc
        .get("answers")
        .and_then(Value::as_object)
        .expect("answers object");
    assert_eq!(
        answers.get("extension_operation").and_then(Value::as_str),
        Some("create_extension_pack")
    );
    assert_eq!(
        answers.get("extension_catalog_ref").and_then(Value::as_str),
        Some("fixture://extensions.json")
    );
    assert_eq!(
        answers.get("extension_type_id").and_then(Value::as_str),
        Some("control")
    );
    assert_eq!(
        answers.get("extension_template_id").and_then(Value::as_str),
        Some("control-basic")
    );
}

#[test]
fn wizard_validate_with_migrate_reemits_document() {
    let temp = TempDir::new().expect("tempdir");
    let input_path = temp.path().join("old_answers.json");
    let output_path = temp.path().join("migrated_answers.json");
    fs::write(
        &input_path,
        r#"{"answers":{"pack_dir":"."},"locks":{"legacy":"x"}}"#,
    )
    .expect("write old answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&input_path)
        .arg("--migrate")
        .arg("--schema-version")
        .arg("2.0.0")
        .arg("--emit-answers")
        .arg(&output_path)
        .output()
        .expect("run wizard validate");
    assert!(output.status.success(), "wizard validate should succeed");

    let doc: Value =
        serde_json::from_slice(&fs::read(&output_path).expect("read migrated answers"))
            .expect("parse migrated answers");
    assert_eq!(
        doc.get("wizard_id").and_then(Value::as_str),
        Some("greentic-pack.wizard.run")
    );
    assert_eq!(
        doc.get("schema_id").and_then(Value::as_str),
        Some("greentic-pack.wizard.answers")
    );
    assert_eq!(
        doc.get("schema_version").and_then(Value::as_str),
        Some("2.0.0")
    );
}

#[test]
fn wizard_apply_answers_runs_pipeline() {
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let answers_path = temp.path().join("apply_answers.json");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{"pack_dir":".","run_doctor":true,"run_build":true,"sign":false},
  "locks":{}
}"#,
    )
    .expect("write answers file");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("doctor --in ."));
    assert!(calls.contains("build --in ."));
}

#[test]
fn wizard_apply_answers_with_sign_runs_sign_step() {
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let answers_path = temp.path().join("apply_sign_answers.json");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{
    "pack_dir":".",
    "run_doctor":true,
    "run_build":true,
    "sign":true,
    "sign_key_path":"./test-sign-key.pem"
  },
  "locks":{}
}"#,
    )
    .expect("write answers file");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("doctor --in ."));
    assert!(calls.contains("build --in ."));
    assert!(calls.contains("sign --pack . --key ./test-sign-key.pem"));
}

#[test]
fn wizard_apply_answers_stage_asset_file_into_pack() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(
        source_dir.join("README-snippet.md"),
        "hello from staged file\n",
    )
    .expect("write source file");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/README-snippet.md",
                        "destination": "assets/docs/readme-snippet.md",
                        "kind": "file"
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/docs/readme-snippet.md"))
            .expect("read staged file"),
        "hello from staged file\n"
    );
}

#[test]
fn wizard_run_answers_stage_asset_file_into_pack() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(source_dir.join("hello.txt"), "run path staging\n").expect("write source file");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/hello.txt",
                        "destination": "assets/hello.txt",
                        "kind": "file"
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = run_wizard_answers("run", &answers_path);
    assert!(
        output.status.success(),
        "wizard run --answers should succeed"
    );
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/hello.txt")).expect("read staged file"),
        "run path staging\n"
    );
}

#[test]
fn wizard_apply_answers_stage_asset_directory_recursively() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external/cards");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(source_dir.join("nested")).expect("create nested source dir");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(source_dir.join("hello.json"), "{\"hello\":true}\n").expect("write root file");
    fs::write(source_dir.join("nested/world.json"), "{\"world\":true}\n")
        .expect("write nested file");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/cards",
                        "destination": "assets/cards",
                        "kind": "directory",
                        "recursive": true
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/cards/hello.json")).expect("read root copy"),
        "{\"hello\":true}\n"
    );
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/cards/nested/world.json"))
            .expect("read nested copy"),
        "{\"world\":true}\n"
    );
}

#[test]
fn wizard_apply_answers_reject_asset_staging_outside_pack_root() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(source_dir.join("hello.txt"), "nope\n").expect("write source file");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/hello.txt",
                        "destination": "../outside.txt",
                        "kind": "file"
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert!(!output.status.success(), "wizard apply should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must not contain '..' segments"));
}

#[test]
fn wizard_apply_answers_reject_missing_asset_source() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/missing.txt",
                        "destination": "assets/missing.txt",
                        "kind": "file"
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert!(!output.status.success(), "wizard apply should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("source does not exist"));
}

#[test]
fn wizard_asset_staging_can_feed_followup_build_without_shell_copying() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("adaptive-pack");
    let external_dir = temp.path().join("external/cards");
    let answers_path = temp.path().join("answers.json");
    copy_tree(
        &workspace_root().join("examples/adaptive-mcp-oauth-demo"),
        &pack_dir,
    );
    fs::create_dir_all(&external_dir).expect("create external dir");

    let source_card = fs::read_to_string(pack_dir.join("assets/oauth_connect_card.json"))
        .expect("read source card");
    fs::write(external_dir.join("hello.json"), &source_card).expect("write external card");
    fs::remove_file(pack_dir.join("assets/oauth_connect_card.json")).expect("remove original card");

    let flow_path = pack_dir.join("flows/adaptive_mcp_oauth_demo.ygtc");
    let flow = fs::read_to_string(&flow_path).expect("read flow");
    fs::write(
        &flow_path,
        flow.replace(
            "asset_path: oauth_connect_card.json",
            "asset_path: cards/hello.json",
        ),
    )
    .expect("write flow");

    let pack_yaml_path = pack_dir.join("pack.yaml");
    let pack_yaml = fs::read_to_string(&pack_yaml_path).expect("read pack yaml");
    fs::write(
        &pack_yaml_path,
        pack_yaml.replace("assets/oauth_connect_card.json", "assets/cards/hello.json"),
    )
    .expect("write pack yaml");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/cards/hello.json",
                        "destination": "assets/cards/hello.json",
                        "kind": "file"
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let apply = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .current_dir(workspace_root())
        .output()
        .expect("run wizard apply");
    assert!(apply.status.success(), "wizard apply should succeed");

    let build = pack_cmd()
        .arg("build")
        .arg("--in")
        .arg(&pack_dir)
        .arg("--log")
        .arg("warn")
        .current_dir(workspace_root())
        .output()
        .expect("run build");
    assert!(
        build.status.success(),
        "build should succeed after asset staging"
    );
    assert!(pack_dir.join("assets/cards/hello.json").exists());
}

#[test]
fn wizard_asset_staging_run_and_apply_have_identical_results() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_root = temp.path().join("inputs");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(source_root.join("cards/empty")).expect("create source dirs");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(source_root.join("cards/alpha.json"), "{\"alpha\":1}\n").expect("write alpha");
    fs::write(source_root.join("cards/.keep"), "").expect("write dotfile");
    fs::write(source_root.join("snippet.md"), "parity\n").expect("write snippet");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./inputs/cards",
                "destination": "assets/cards",
                "kind": "directory",
                "recursive": true
            },
            {
                "source": "./inputs/snippet.md",
                "destination": "assets/docs/snippet.md",
                "kind": "file"
            }
        ]),
    );

    let run = run_wizard_answers("run", &answers_path);
    assert!(run.status.success(), "wizard run --answers should succeed");
    let run_snapshot = snapshot_tree(&pack_dir);

    fs::remove_dir_all(&pack_dir).expect("remove pack after run");
    fs::create_dir_all(&pack_dir).expect("recreate pack");

    let apply = run_wizard_answers("apply", &answers_path);
    assert!(apply.status.success(), "wizard apply should succeed");
    let apply_snapshot = snapshot_tree(&pack_dir);

    assert_eq!(run_snapshot, apply_snapshot);
}

#[test]
fn wizard_asset_staging_resolves_relative_sources_from_answers_file_directory() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_dir = temp.path().join("nested/answers");
    let answers_path = answers_dir.join("answers.json");
    fs::create_dir_all(answers_dir.join("source")).expect("create source dir");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(
        answers_dir.join("source/from-answers-dir.txt"),
        "relative source base\n",
    )
    .expect("write source");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./source/from-answers-dir.txt",
                "destination": "assets/from-answers-dir.txt",
                "kind": "file"
            }
        ]),
    );

    let output = pack_cmd()
        .current_dir(workspace_root())
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/from-answers-dir.txt")).expect("read staged"),
        "relative source base\n"
    );
}

#[test]
fn wizard_asset_staging_supports_absolute_source_paths() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_path = temp.path().join("absolute.txt");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(&source_path, "absolute path\n").expect("write absolute source");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": source_path,
                "destination": "assets/absolute.txt",
                "kind": "file"
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/absolute.txt")).expect("read staged"),
        "absolute path\n"
    );
}

#[test]
fn wizard_asset_staging_rejects_normalized_destination_traversal_forms() {
    for destination in [
        "../outside.txt",
        "assets/../../outside.txt",
        "./assets/../outside.txt",
    ] {
        let temp = TempDir::new().expect("tempdir");
        let pack_dir = temp.path().join("pack");
        let source_dir = temp.path().join("external");
        let answers_path = temp.path().join("answers.json");
        fs::create_dir_all(&pack_dir).expect("create pack dir");
        fs::create_dir_all(&source_dir).expect("create source dir");
        fs::write(source_dir.join("hello.txt"), destination).expect("write source");

        write_asset_answers(
            &answers_path,
            &pack_dir,
            json!([
                {
                    "source": "./external/hello.txt",
                    "destination": destination,
                    "kind": "file"
                }
            ]),
        );

        let output = run_wizard_answers("apply", &answers_path);
        assert!(
            !output.status.success(),
            "destination {destination} should fail"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("must not contain '..' segments"),
            "{stderr}"
        );
    }
}

#[test]
fn wizard_asset_staging_rejects_duplicate_destinations_deterministically() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(source_dir.join("one.txt"), "one\n").expect("write one");
    fs::write(source_dir.join("two.txt"), "two\n").expect("write two");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/one.txt",
                "destination": "assets/collision.txt",
                "kind": "file"
            },
            {
                "source": "./external/two.txt",
                "destination": "assets/collision.txt",
                "kind": "file",
                "overwrite": true
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(
        !output.status.success(),
        "duplicate destination should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("conflicts with another asset staging entry"));
    assert!(stderr.contains("assets/collision.txt"));
}

#[test]
fn wizard_asset_staging_rejects_file_to_existing_directory_destination() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(pack_dir.join("assets/cards")).expect("create destination dir");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(source_dir.join("card.json"), "{}\n").expect("write source");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/card.json",
                "destination": "assets/cards",
                "kind": "file"
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(!output.status.success(), "file to dir should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("destination is a directory"));
}

#[test]
fn wizard_asset_staging_rejects_directory_to_existing_file_destination() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external/cards");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::create_dir_all(pack_dir.join("assets")).expect("create assets dir");
    fs::write(pack_dir.join("assets/cards"), "existing file\n").expect("write existing file");
    fs::write(source_dir.join("hello.json"), "{}\n").expect("write source");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/cards",
                "destination": "assets/cards",
                "kind": "directory",
                "recursive": true
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(!output.status.success(), "directory to file should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("destination is a file"));
}

#[test]
fn wizard_asset_staging_overwrites_existing_scaffold_file_when_enabled() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("scaffold-pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(
        source_dir.join("pack.yaml"),
        "pack_id: overwritten.by.staging\n",
    )
    .expect("write source");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "create_pack_scaffold": true,
                "create_pack_id": "scaffold.override",
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/pack.yaml",
                        "destination": "pack.yaml",
                        "kind": "file",
                        "overwrite": true
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = run_wizard_answers("apply", &answers_path);
    assert!(output.status.success(), "overwrite=true should succeed");
    assert_eq!(
        fs::read_to_string(pack_dir.join("pack.yaml")).expect("read staged"),
        "pack_id: overwritten.by.staging\n"
    );
}

#[test]
fn wizard_asset_staging_rejects_existing_scaffold_file_when_overwrite_disabled() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("scaffold-pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&source_dir).expect("create source dir");
    fs::write(
        source_dir.join("pack.yaml"),
        "pack_id: overwritten.by.staging\n",
    )
    .expect("write source");

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "create_pack_scaffold": true,
                "create_pack_id": "scaffold.no-overwrite",
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": [
                    {
                        "source": "./external/pack.yaml",
                        "destination": "pack.yaml",
                        "kind": "file",
                        "overwrite": false
                    }
                ]
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = run_wizard_answers("apply", &answers_path);
    assert!(!output.status.success(), "overwrite=false should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("overwrite=false"));
    assert!(stderr.contains("pack.yaml"));
}

#[test]
fn wizard_asset_staging_reports_missing_source_path() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/missing.txt",
                "destination": "assets/missing.txt",
                "kind": "file"
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(!output.status.success(), "missing source should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("source does not exist"));
    assert!(stderr.contains("external/missing.txt"));
}

#[test]
fn wizard_asset_staging_rejects_kind_mismatches() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_file = temp.path().join("file_mismatch.json");
    let answers_dir = temp.path().join("dir_mismatch.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(source_dir.join("cards")).expect("create dir source");
    fs::write(source_dir.join("single.txt"), "one\n").expect("write file source");

    write_asset_answers(
        &answers_file,
        &pack_dir,
        json!([
            {
                "source": "./external/cards",
                "destination": "assets/cards",
                "kind": "file"
            }
        ]),
    );
    let file_output = run_wizard_answers("apply", &answers_file);
    assert!(!file_output.status.success(), "dir as file should fail");
    assert!(
        String::from_utf8_lossy(&file_output.stderr).contains("kind=file requires a file source")
    );

    write_asset_answers(
        &answers_dir,
        &pack_dir,
        json!([
            {
                "source": "./external/single.txt",
                "destination": "assets/single",
                "kind": "directory",
                "recursive": true
            }
        ]),
    );
    let dir_output = run_wizard_answers("apply", &answers_dir);
    assert!(!dir_output.status.success(), "file as dir should fail");
    assert!(
        String::from_utf8_lossy(&dir_output.stderr)
            .contains("kind=directory requires a directory source")
    );
}

#[test]
fn wizard_asset_staging_preserves_hidden_files_and_empty_directories() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external/tree");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(source_dir.join("nested/empty")).expect("create nested dirs");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::write(source_dir.join(".gitkeep"), "").expect("write gitkeep");
    fs::write(source_dir.join("nested/file.json"), "{}\n").expect("write nested file");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/tree",
                "destination": "assets/tree",
                "kind": "directory",
                "recursive": true
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(output.status.success(), "recursive staging should succeed");
    assert!(
        pack_dir.join("assets/tree/.gitkeep").exists(),
        "dotfile should be copied"
    );
    assert!(
        pack_dir.join("assets/tree/nested/file.json").exists(),
        "nested file should be copied"
    );
    assert!(
        pack_dir.join("assets/tree/nested/empty").is_dir(),
        "empty dir should be preserved"
    );
}

#[test]
fn wizard_asset_staging_apply_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(source_dir.join("cards")).expect("create source dir");
    fs::write(source_dir.join("cards/hello.json"), "{\"hello\":true}\n").expect("write source");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/cards",
                "destination": "assets/cards",
                "kind": "directory",
                "recursive": true
            }
        ]),
    );

    let first = run_wizard_answers("apply", &answers_path);
    assert!(first.status.success(), "first apply should succeed");
    let first_snapshot = snapshot_tree(&pack_dir);

    let second = run_wizard_answers("apply", &answers_path);
    assert!(second.status.success(), "second apply should succeed");
    let second_snapshot = snapshot_tree(&pack_dir);

    assert_eq!(first_snapshot, second_snapshot);
}

#[test]
fn wizard_asset_staging_partial_failure_is_fail_fast_and_keeps_prior_copies() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let source_dir = temp.path().join("external");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    fs::create_dir_all(source_dir.join("cards")).expect("create source dirs");
    fs::write(source_dir.join("ok.txt"), "ok\n").expect("write ok file");
    fs::write(source_dir.join("cards/hello.json"), "{}\n").expect("write card file");

    write_asset_answers(
        &answers_path,
        &pack_dir,
        json!([
            {
                "source": "./external/ok.txt",
                "destination": "assets/conflict",
                "kind": "file"
            },
            {
                "source": "./external/cards",
                "destination": "assets/conflict/subdir",
                "kind": "directory",
                "recursive": true
            }
        ]),
    );

    let output = run_wizard_answers("apply", &answers_path);
    assert!(!output.status.success(), "apply should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not a directory") || stderr.contains("create staged asset parent"));
    assert_eq!(
        fs::read_to_string(pack_dir.join("assets/conflict")).expect("first file should remain"),
        "ok\n"
    );
    assert!(!pack_dir.join("assets/conflict/subdir/hello.json").exists());
}

#[test]
fn wizard_validate_answers_is_side_effect_free() {
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let answers_path = temp.path().join("validate_answers.json");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{"pack_dir":".","run_doctor":true,"run_build":true,"sign":false},
  "locks":{}
}"#,
    )
    .expect("write answers file");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard validate");
    assert!(output.status.success(), "wizard validate should succeed");
    assert!(
        !log_path.exists()
            || fs::read_to_string(&log_path)
                .expect("read calls")
                .trim()
                .is_empty()
    );
}

#[test]
fn wizard_run_with_answers_executes_apply_flow() {
    let temp = TempDir::new().expect("tempdir");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let answers_path = temp.path().join("run_answers.json");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{"pack_dir":".","run_doctor":true,"run_build":true,"sign":false},
  "locks":{}
}"#,
    )
    .expect("write answers file");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .arg("wizard")
        .arg("run")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard run --answers");
    assert!(
        output.status.success(),
        "wizard run --answers should succeed"
    );

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("doctor --in ."));
    assert!(calls.contains("build --in ."));
}

#[test]
fn wizard_validate_missing_schema_without_migrate_fails() {
    let temp = TempDir::new().expect("tempdir");
    let input_path = temp.path().join("old_answers.json");
    fs::write(&input_path, r#"{"answers":{"pack_dir":"."},"locks":{}}"#)
        .expect("write old answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&input_path)
        .output()
        .expect("run wizard validate");
    assert!(
        !output.status.success(),
        "validate should fail without --migrate"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rerun with --migrate"));
}

#[test]
fn wizard_validate_schema_mismatch_without_migrate_fails() {
    let temp = TempDir::new().expect("tempdir");
    let input_path = temp.path().join("schema_mismatch_answers.json");
    fs::write(
        &input_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"0.9.0",
  "locale":"en-GB",
  "answers":{"pack_dir":"."},
  "locks":{}
}"#,
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&input_path)
        .arg("--schema-version")
        .arg("1.0.0")
        .output()
        .expect("run validate");
    assert!(
        !output.status.success(),
        "validate should fail on schema mismatch without --migrate"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not match target"));
}

#[test]
fn wizard_validate_schema_mismatch_with_migrate_succeeds() {
    let temp = TempDir::new().expect("tempdir");
    let input_path = temp.path().join("schema_mismatch_answers.json");
    let output_path = temp.path().join("schema_mismatch_migrated.json");
    fs::write(
        &input_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"0.9.0",
  "locale":"en-GB",
  "answers":{"pack_dir":"."},
  "locks":{"k":"v"}
}"#,
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&input_path)
        .arg("--schema-version")
        .arg("1.0.0")
        .arg("--migrate")
        .arg("--emit-answers")
        .arg(&output_path)
        .output()
        .expect("run validate");
    assert!(
        output.status.success(),
        "validate should succeed with migrate on schema mismatch"
    );
    let doc: Value = serde_json::from_slice(&fs::read(&output_path).expect("read migrated"))
        .expect("parse migrated");
    assert_eq!(
        doc.get("schema_version").and_then(Value::as_str),
        Some("1.0.0")
    );
    assert_eq!(
        doc.get("locks")
            .and_then(Value::as_object)
            .and_then(|o| o.get("k"))
            .and_then(Value::as_str),
        Some("v")
    );
}

#[test]
fn wizard_validate_rejects_wrong_wizard_id() {
    let temp = TempDir::new().expect("tempdir");
    let input_path = temp.path().join("wrong_wizard_answers.json");
    fs::write(
        &input_path,
        r#"{
  "wizard_id":"other.wizard",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{"pack_dir":"."},
  "locks":{}
}"#,
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("validate")
        .arg("--answers")
        .arg(&input_path)
        .output()
        .expect("run validate");
    assert!(
        !output.status.success(),
        "validate should fail for unsupported wizard_id"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported wizard_id"));
}

#[test]
fn wizard_run_with_invalid_answers_file_writes_localized_error_and_continues() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("invalid_answers.json");
    fs::write(
        &answers_path,
        r#"{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{"pack_dir":".","run_build":"yes"},
  "locks":{}
}"#,
    )
    .expect("write invalid answers file");

    let mut child = pack_cmd()
        .arg("wizard")
        .arg("--answers")
        .arg(&answers_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard --answers");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"0\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");

    assert!(
        output.status.success(),
        "wizard should continue interactively after invalid answers"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Wizard answers error: answers.run_build must be a boolean"));
    assert!(stdout.contains("Main Menu"));
    assert!(stdout.contains("0) Exit"));
    assert!(
        String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "wizard should keep the error in wizard output, not stderr"
    );
}

#[test]
fn wizard_run_dry_run_records_choices_without_side_effects() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("dry_run_answers.json");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    write_script(
        &flow_exe,
        r#"#!/usr/bin/env bash
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--emit-answers" ]; then
    out="$2"
    shift 2
    continue
  fi
  shift
done
if [ -n "$out" ]; then
  printf '{"flow":"ok"}' > "$out"
fi
exit 0
"#,
    );
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--qa-answers-out" ]; then
    out="$2"
    shift 2
    continue
  fi
  shift
done
if [ -n "$out" ]; then
  printf '{"component":"ok"}' > "$out"
fi
exit 0
"#,
    );

    let input = format!(
        "2\n{}\n1\n2\n2\n2\n3\n2\n4\n./demo-sign.pem\nM\n0\n",
        pack_dir.display()
    );
    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard run dry-run");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(
        output.status.success(),
        "wizard run --dry-run should succeed"
    );

    let doc: Value = serde_json::from_slice(&fs::read(&answers_path).expect("read answers"))
        .expect("parse answers");
    let answers = doc
        .get("answers")
        .and_then(Value::as_object)
        .expect("answers object");
    assert_eq!(
        answers.get("pack_dir").and_then(Value::as_str),
        Some(pack_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        answers.get("run_delegate_flow").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers
            .get("run_delegate_component")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers.get("run_doctor").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers.get("run_build").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(answers.get("sign").and_then(Value::as_bool), Some(true));
    assert_eq!(
        answers.get("sign_key_path").and_then(Value::as_str),
        Some("./demo-sign.pem")
    );
    assert_eq!(answers.get("dry_run").and_then(Value::as_bool), Some(true));
    assert_eq!(
        answers.get("mode").and_then(Value::as_str),
        Some("interactive-dry-run")
    );
    assert_eq!(
        answers
            .get("flow_wizard_answers")
            .and_then(Value::as_object)
            .and_then(|v| v.get("flow"))
            .and_then(Value::as_str),
        Some("ok")
    );
    assert_eq!(
        answers
            .get("component_wizard_answers")
            .and_then(Value::as_object)
            .and_then(|v| v.get("component"))
            .and_then(Value::as_str),
        Some("ok")
    );
    let selected = answers
        .get("selected_actions")
        .and_then(Value::as_array)
        .expect("selected_actions array");
    let selected_values = selected
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(selected_values.contains(&"main.update_application_pack"));
    assert!(selected_values.contains(&"update_application_pack.edit_flows"));
    assert!(selected_values.contains(&"update_application_pack.add_edit_components"));
    assert!(selected_values.contains(&"update_application_pack.run_update_validate"));
    assert!(selected_values.contains(&"update_application_pack.sign"));
}

#[test]
fn wizard_apply_replays_recorded_delegate_and_pipeline_actions() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

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
    write_script(
        &component_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"component:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "run_delegate_flow":true,
    "run_delegate_component":true,
    "run_doctor":true,
    "run_build":true,
    "sign":true,
    "sign_key_path":"./demo-sign.pem"
  }},
  "locks":{{}}
}}"#,
            pack_dir.display()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("flow:wizard ."));
    assert!(calls.contains("component:wizard"));
    assert!(calls.contains("self:doctor --in"));
    assert!(calls.contains("self:build --in"));
    assert!(calls.contains("self:sign --pack"));
}

#[test]
fn wizard_apply_with_nested_answers_can_scaffold_and_replay_subwizards() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("new-pack");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");
    let answers_path = temp.path().join("answers.json");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\n\
echo \"self:$*\" >> \"{}\"\n\
if [ \"$1\" = \"new\" ] && [ \"$2\" = \"--dir\" ]; then\n\
  mkdir -p \"$3\"\n\
fi\n\
exit 0\n",
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
    write_script(
        &component_exe,
        &format!(
            "#!/usr/bin/env bash\necho \"component:$*\" >> \"{}\"\nexit 0\n",
            log_path.display()
        ),
    );
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "create_pack_scaffold":true,
    "create_pack_id":"my-pack",
    "run_delegate_flow":true,
    "run_delegate_component":true,
    "selected_actions":[
      "main.update_application_pack",
      "update_application_pack.edit_flows"
    ],
    "flow_wizard_answers":{{
      "schema_id":"greentic-flow.wizard.plan",
      "schema_version":"2.0.0",
      "actions":[
        {{
          "action":"add-flow",
          "flow":"flows/demo.ygtc"
        }},
        {{
          "action":"add-step",
          "flow":"flows/demo.ygtc",
          "component":"components/demo.wasm",
          "mode":"setup",
          "answers":{{"tenant":"acme"}}
        }}
      ]
    }},
    "component_wizard_answers":{{"component":"ok"}},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");
    assert!(pack_dir.is_dir(), "apply should scaffold pack directory");

    let calls = fs::read_to_string(&log_path).expect("read call log");
    assert!(calls.contains("self:new --dir"));
    assert!(calls.contains("flow:wizard . --answers"));
    assert!(calls.contains("component:wizard --project-root . --execution execute --qa-answers"));
    assert!(calls.contains("self:update --in"));
    assert!(calls.contains("self:doctor --in"));
    assert!(calls.contains("self:build --in"));
    let update_idx = calls.find("self:update --in").expect("update call");
    let doctor_idx = calls.find("self:doctor --in").expect("doctor call");
    let resolve_idx = calls.find("self:resolve --in").expect("resolve call");
    let build_idx = calls.find("self:build --in").expect("build call");
    assert!(
        update_idx < doctor_idx,
        "update should happen before doctor"
    );
    assert!(
        doctor_idx < resolve_idx,
        "doctor should happen before resolve"
    );
    assert!(
        resolve_idx < build_idx,
        "resolve should happen before build"
    );
}

#[test]
fn wizard_apply_control_extension_answers_is_deterministic() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("control-pack");
    let answers_path = temp.path().join("control_answers.json");
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "extension_operation":"create_extension_pack",
    "extension_catalog_ref":"fixture://extensions.json",
    "extension_type_id":"control",
    "extension_template_id":"control-basic",
    "extension_template_qa_answers":{{
      "display_name":"Routing ingress control chain",
      "pack_id":"routing.ingress.control.chain"
    }},
    "extension_edit_answers":{{
      "entry_label":"control",
      "create_offer":"false",
      "offer_id":"control-offer",
      "cap_id":"greentic.cap.control.chain.v1",
      "component_ref":"controller",
      "op":"apply",
      "version":"v1",
      "priority":"0",
      "requires_setup":"false",
      "qa_ref":"qa/control-setup.json",
      "hook_op_names":""
    }},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display()
        ),
    )
    .expect("write answers");

    let first = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run first apply");
    assert!(first.status.success(), "first apply should succeed");

    let pack_yaml_before = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    let extension_before =
        fs::read_to_string(pack_dir.join("extensions/control.json")).expect("read extension json");
    assert!(pack_dir.join("flows").is_dir());
    assert!(pack_dir.join("components").is_dir());
    assert!(pack_dir.join("i18n").is_dir());
    assert!(pack_dir.join("assets").is_dir());
    assert!(pack_dir.join("qa").is_dir());
    assert!(pack_dir.join("assets/README.md").exists());
    assert!(pack_dir.join("qa/README.md").exists());
    assert!(pack_yaml_before.contains("greentic.ext.capabilities.v1"));
    assert!(pack_yaml_before.contains("offers: []"));

    let second = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run second apply");
    assert!(second.status.success(), "second apply should succeed");

    let pack_yaml_after = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    let extension_after =
        fs::read_to_string(pack_dir.join("extensions/control.json")).expect("read extension json");
    assert_eq!(
        pack_yaml_before, pack_yaml_after,
        "pack.yaml should be idempotent"
    );
    assert_eq!(
        extension_before, extension_after,
        "extensions/control.json should be idempotent"
    );
}

#[test]
fn wizard_apply_runtime_capability_answers_scaffolds_replayable_pack() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("runtime-pack");
    let answers_path = temp.path().join("runtime_answers.json");
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "extension_operation":"create_extension_pack",
    "extension_catalog_ref":"{}",
    "extension_type_id":"runtime-capability",
    "extension_template_id":"runtime-capability-basic",
    "extension_template_qa_answers":{{
      "display_name":"Runtime capability extension",
      "pack_id":"runtime.capability.extension"
    }},
    "extension_edit_answers":{{
      "entry_label":"runtime",
      "create_offer":"true",
      "offer_id":"runtime-offer",
      "cap_id":"greentic.cap.runtime.execution.v1",
      "component_ref":"runtime",
      "op":"run",
      "version":"v1",
      "priority":"0",
      "requires_setup":"false",
      "qa_ref":"qa/runtime-setup.json",
      "hook_op_names":""
    }},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display(),
            default_catalog_ref()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run apply");
    assert!(output.status.success(), "runtime apply should succeed");

    assert!(pack_dir.join("assets/runtime/README.md").exists());
    assert!(pack_dir.join("assets/examples/runtime-input.json").exists());
    assert!(pack_dir.join("components/runtime/component.wasm").exists());
    assert!(pack_dir.join("extensions/runtime-capability.json").exists());

    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    assert!(pack_yaml.contains("greentic.ext.capabilities.v1"));
    assert!(pack_yaml.contains("runtime-offer"));
    assert!(pack_yaml.contains("id: runtime"));
}

#[test]
fn wizard_apply_contract_answers_scaffolds_contract_assets() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("contract-pack");
    let answers_path = temp.path().join("contract_answers.json");
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "extension_operation":"create_extension_pack",
    "extension_catalog_ref":"{}",
    "extension_type_id":"contract",
    "extension_template_id":"contract-basic",
    "extension_template_qa_answers":{{
      "display_name":"Contract extension",
      "pack_id":"contract.extension"
    }},
    "extension_edit_answers":{{
      "entry_label":"contract",
      "create_offer":"false",
      "offer_id":"contract-offer",
      "cap_id":"greentic.cap.contract.bundle.v1",
      "component_ref":"contract-hook",
      "op":"validate",
      "version":"v1",
      "priority":"0",
      "requires_setup":"false",
      "qa_ref":"qa/contract-setup.json",
      "hook_op_names":""
    }},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display(),
            default_catalog_ref()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run apply");
    assert!(output.status.success(), "contract apply should succeed");

    assert!(pack_dir.join("assets/contracts/transitions.yaml").exists());
    assert!(pack_dir.join("assets/contracts/rules.yaml").exists());
    assert!(
        pack_dir
            .join("assets/examples/contract.example.json")
            .exists()
    );
    assert!(
        pack_dir
            .join("components/contract-hook/component.wasm")
            .exists()
    );
    assert!(pack_dir.join("extensions/contract.json").exists());

    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    assert!(pack_yaml.contains("offers: []"));
    assert!(pack_yaml.contains("id: contract-hook"));
}

#[test]
fn wizard_apply_ops_answers_scaffolds_ops_bundle() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("ops-pack");
    let answers_path = temp.path().join("ops_answers.json");
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "extension_operation":"create_extension_pack",
    "extension_catalog_ref":"{}",
    "extension_type_id":"ops",
    "extension_template_id":"ops-basic",
    "extension_template_qa_answers":{{
      "display_name":"Ops extension",
      "pack_id":"ops.extension"
    }},
    "extension_edit_answers":{{
      "entry_label":"ops",
      "create_offer":"true",
      "offer_id":"ops-offer",
      "cap_id":"greentic.cap.ops.execution.v1",
      "component_ref":"ops-provider",
      "op":"execute",
      "version":"v1",
      "priority":"0",
      "requires_setup":"false",
      "qa_ref":"qa/ops-setup.json",
      "hook_op_names":"before,after"
    }},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display(),
            default_catalog_ref()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run apply");
    assert!(output.status.success(), "ops apply should succeed");

    assert!(pack_dir.join("assets/ops/ops.yaml").exists());
    assert!(pack_dir.join("assets/examples/ops-input.json").exists());
    assert!(
        pack_dir
            .join("components/ops-provider/component.wasm")
            .exists()
    );
    assert!(pack_dir.join("extensions/ops.json").exists());

    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    assert!(pack_yaml.contains("ops-offer"));
    assert!(pack_yaml.contains("id: ops-provider"));
}

#[test]
fn wizard_run_dry_run_then_apply_deployer_destroy_answers_succeeds() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("pack-wizard-sample.json");
    let pack_dir = temp.path().join("deploy-test");
    let input = format!(
        "3\nn\n11\n1\n{}\n\n\ndeployer\ngreentic.deployer.example.v1\ndeployer\ngenerate,plan,apply,destroy,status,rollback\n2\n0\n",
        pack_dir.display()
    );

    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard run");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "wizard run should succeed");

    let apply = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert!(apply.status.success(), "wizard apply should succeed");

    let extension =
        fs::read_to_string(pack_dir.join("extensions/deployer.json")).expect("read extension");
    assert!(extension.contains("\"destroy\""));
    assert!(!extension.contains("\"remove\""));
    assert!(pack_dir.join("flows/destroy.ygtc").exists());
    assert!(!pack_dir.join("flows/remove.ygtc").exists());
}

#[test]
fn wizard_apply_deployer_legacy_remove_answers_fail() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("legacy-deployer-pack");
    let answers_path = temp.path().join("legacy_deployer_answers.json");
    fs::write(
        &answers_path,
        format!(
            r#"{{
  "wizard_id":"greentic-pack.wizard.run",
  "schema_id":"greentic-pack.wizard.answers",
  "schema_version":"1.0.0",
  "locale":"en-GB",
  "answers":{{
    "pack_dir":"{}",
    "extension_operation":"create_extension_pack",
    "extension_catalog_ref":"{}",
    "extension_type_id":"deployer",
    "extension_template_id":"deployer-basic",
    "extension_template_qa_answers":{{
      "display_name":"Generic deployer extension",
      "pack_id":"deployer.extension"
    }},
    "extension_edit_answers":{{
      "entry_label":"deployer",
      "contract_id":"greentic.deployer.example.v1",
      "component_ref":"deployer",
      "supported_ops":"generate,plan,apply,remove,status,rollback"
    }},
    "run_doctor":true,
    "run_build":true,
    "sign":false
  }},
  "locks":{{}}
}}"#,
            pack_dir.display(),
            default_catalog_ref()
        ),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .output()
        .expect("run wizard apply");
    assert!(
        !output.status.success(),
        "legacy remove answers should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("flows/remove.ygtc"));
    let extension =
        fs::read_to_string(pack_dir.join("extensions/deployer.json")).expect("read extension");
    assert!(extension.contains("\"remove\""));
    assert!(!extension.contains("\"destroy\""));
}

#[test]
fn wizard_dry_run_flow_child_exit_returns_gracefully_to_pack_menu() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");

    write_script(
        &self_exe,
        "#!/usr/bin/env bash\nif [ \"$1\" = \"new\" ] && [ \"$2\" = \"--dir\" ]; then mkdir -p \"$3\"; fi\nexit 0\n",
    );
    write_script(&flow_exe, "#!/usr/bin/env bash\nexit 1\n");

    let input = format!(
        "1\nmy-pack\n{}\n1\n0\n0\n",
        temp.path().join("pack").display()
    );
    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "wizard should return gracefully");
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(rendered.contains("child wizard exited") || rendered.contains("child-wizard"));
}

#[test]
fn wizard_dry_run_component_child_exit_returns_gracefully_to_pack_menu() {
    let temp = TempDir::new().expect("tempdir");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let component_exe = temp.path().join("greentic-component");

    write_script(
        &self_exe,
        "#!/usr/bin/env bash\nif [ \"$1\" = \"new\" ] && [ \"$2\" = \"--dir\" ]; then mkdir -p \"$3\"; fi\nexit 0\n",
    );
    write_script(&component_exe, "#!/usr/bin/env bash\nexit 1\n");

    let input = format!(
        "1\nmy-pack\n{}\n2\n0\n0\n",
        temp.path().join("pack").display()
    );
    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wizard");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "wizard should return gracefully");
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(rendered.contains("child wizard exited") || rendered.contains("child-wizard"));
}

#[test]
fn wizard_dry_run_export_and_replay_constructs_pack_flow_component() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("target-pack");
    let answers_path = temp.path().join("dry_run_answers.json");
    let log_path = temp.path().join("calls.log");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");

    write_script(
        &self_exe,
        &format!(
            "#!/usr/bin/env bash\n\
echo \"self:$*\" >> \"{}\"\n\
if [ \"$1\" = \"new\" ] && [ \"$2\" = \"--dir\" ]; then mkdir -p \"$3\"; fi\n\
exit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &flow_exe,
        &format!(
            "#!/usr/bin/env bash\n\
echo \"flow:$*\" >> \"{}\"\n\
emit=\"\"\n\
answers=\"\"\n\
while [ \"$#\" -gt 0 ]; do\n\
  if [ \"$1\" = \"--emit-answers\" ]; then emit=\"$2\"; shift 2; continue; fi\n\
  if [ \"$1\" = \"--answers\" ]; then answers=\"$2\"; shift 2; continue; fi\n\
  shift\n\
done\n\
if [ -n \"$emit\" ]; then printf '{{\"flow\":\"dry-run\"}}' > \"$emit\"; fi\n\
if [ -n \"$answers\" ]; then touch \"$PWD/flow.replayed\"; fi\n\
exit 0\n",
            log_path.display()
        ),
    );
    write_script(
        &component_exe,
        &format!(
            "#!/usr/bin/env bash\n\
echo \"component:$*\" >> \"{}\"\n\
out=\"\"\n\
in=\"\"\n\
while [ \"$#\" -gt 0 ]; do\n\
  if [ \"$1\" = \"--qa-answers-out\" ]; then out=\"$2\"; shift 2; continue; fi\n\
  if [ \"$1\" = \"--qa-answers\" ]; then in=\"$2\"; shift 2; continue; fi\n\
  shift\n\
done\n\
if [ -n \"$out\" ]; then printf '{{\"component\":\"dry-run\"}}' > \"$out\"; fi\n\
if [ -n \"$in\" ]; then touch \"$PWD/component.replayed\"; fi\n\
exit 0\n",
            log_path.display()
        ),
    );

    let input = format!("1\nmy-pack\n{}\n1\n2\n3\n2\n0\n", pack_dir.display());
    let mut child = pack_cmd()
        .arg("wizard")
        .arg("run")
        .arg("--dry-run")
        .arg("--emit-answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn dry-run wizard");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait output");
    assert!(output.status.success(), "dry-run should succeed");

    let doc: Value = serde_json::from_slice(&fs::read(&answers_path).expect("read answers"))
        .expect("parse answers");
    let answers = doc
        .get("answers")
        .and_then(Value::as_object)
        .expect("answers object");
    assert_eq!(
        answers.get("create_pack_id").and_then(Value::as_str),
        Some("my-pack")
    );
    assert_eq!(
        answers.get("create_pack_scaffold").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers.get("run_delegate_flow").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers
            .get("run_delegate_component")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers.get("run_doctor").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers.get("run_build").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        answers
            .get("flow_wizard_answers")
            .and_then(Value::as_object)
            .and_then(|v| v.get("flow"))
            .and_then(Value::as_str),
        Some("dry-run")
    );
    assert_eq!(
        answers
            .get("component_wizard_answers")
            .and_then(Value::as_object)
            .and_then(|v| v.get("component"))
            .and_then(Value::as_str),
        Some("dry-run")
    );

    let apply_output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run apply");
    assert!(apply_output.status.success(), "apply should succeed");
    assert!(pack_dir.is_dir(), "pack should be created from answers");
    assert!(pack_dir.join("flow.replayed").exists());
    assert!(pack_dir.join("component.replayed").exists());
    let calls = fs::read_to_string(&log_path).expect("read calls");
    assert!(calls.contains("flow:wizard . --emit-answers"));
    assert!(calls.contains("flow:wizard . --answers"));
}

#[test]
fn wizard_schema_embeds_pack_flow_and_component_contracts() {
    let temp = TempDir::new().expect("tempdir");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");

    write_script(
        &flow_exe,
        r#"#!/usr/bin/env bash
printf '%s\n' "$*" > /dev/null
cat <<'JSON'
{"schema_id":"greentic-flow.wizard.plan","schema_version":"2.0.0","type":"object","properties":{"actions":{"type":"array"}}}
JSON
exit 0
"#,
    );
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
mode=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--mode" ]; then mode="$2"; shift 2; continue; fi
  shift
done
printf '{"title":"component-%s","type":"object","properties":{"answers":{"type":"object","properties":{"mode":{"const":"%s"}}}}}\n' "$mode" "$mode"
"#,
    );

    let output = pack_cmd()
        .arg("wizard")
        .arg("--schema")
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run wizard --schema");
    assert!(output.status.success(), "wizard --schema should succeed");

    let schema: Value = serde_json::from_slice(&output.stdout).expect("parse wizard schema");
    assert_eq!(
        schema
            .get("properties")
            .and_then(|v| v.get("schema_id"))
            .and_then(|v| v.get("const"))
            .and_then(Value::as_str),
        Some("greentic-pack.wizard.answers")
    );
    assert_eq!(
        schema
            .get("properties")
            .and_then(|v| v.get("answers"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("flow_wizard_answers"))
            .and_then(|v| v.get("anyOf"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        schema
            .get("properties")
            .and_then(|v| v.get("answers"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("asset_staging"))
            .and_then(|v| v.get("items"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.get("enum"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        schema
            .get("properties")
            .and_then(|v| v.get("answers"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("asset_staging"))
            .and_then(|v| v.get("items"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("overwrite"))
            .and_then(|v| v.get("default"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_flow_wizard_runtime_schema"))
            .and_then(|v| v.get("schema_id"))
            .and_then(Value::as_str),
        Some("greentic-flow.wizard.plan")
    );
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_component_wizard_create"))
            .and_then(|v| v.get("title"))
            .and_then(Value::as_str),
        Some("component-create")
    );
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_component_wizard_doctor"))
            .and_then(|v| v.get("title"))
            .and_then(Value::as_str),
        Some("component-doctor")
    );
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_component_wizard_qa_envelope"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("schema"))
            .and_then(|v| v.get("const"))
            .and_then(Value::as_str),
        Some("component-wizard-run/v1")
    );
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_component_wizard_simple_fields"))
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("component_name"))
            .and_then(|v| v.get("type"))
            .and_then(Value::as_str),
        Some("string")
    );
    let step_comment = schema
        .get("$defs")
        .and_then(|v| v.get("greentic_flow_step_answers"))
        .and_then(|v| v.get("$comment"))
        .and_then(Value::as_str)
        .expect("flow step comment");
    assert!(step_comment.contains("component-schema <file/oci/repo/store>.wasm"));
    assert!(step_comment.contains("default|setup|update|remove"));
}

#[test]
fn wizard_schema_with_answers_uses_pack_specific_flow_schema_and_dev_overrides() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    let flow_dev_exe = temp.path().join("greentic-flow-dev");
    let component_dev_exe = temp.path().join("greentic-component-dev");
    let flow_log = temp.path().join("flow.log");
    let component_log = temp.path().join("component.log");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_script(
        &flow_dev_exe,
        &format!(
            r#"#!/usr/bin/env bash
echo "$*" > "{}"
if [ "$1" != "wizard" ] || [ "$2" != "--schema" ] || [ "$3" != "{}" ] || [ "$4" != "--answers" ] || [ ! -f "$5" ]; then
  exit 1
fi
cat <<'JSON'
{{"schema_id":"greentic-flow.wizard.plan","schema_version":"2.0.0","title":"pack-specific-flow-schema","properties":{{"actions":{{"type":"array","prefixItems":[{{"action":"from-pack"}}]}}}}}}
JSON
exit 0
"#,
            flow_log.display(),
            pack_dir.display()
        ),
    );
    write_script(
        &component_dev_exe,
        &format!(
            r#"#!/usr/bin/env bash
echo "$*" >> "{}"
mode=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--mode" ]; then mode="$2"; shift 2; continue; fi
  shift
done
printf '{{"title":"component-%s","type":"object"}}\n' "$mode"
"#,
            component_log.display()
        ),
    );

    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_delegate_flow": true,
                "run_delegate_component": false,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "flow_wizard_answers": {
                    "schema_id": "greentic-flow.wizard.plan",
                    "schema_version": "2.0.0",
                    "actions": [
                        {
                            "action": "add-step",
                            "flow": "flows/order_tracking_flow.ygtc",
                            "component": "components/order-status.wasm",
                            "mode": "setup",
                            "answers": {
                                "tenant": "acme"
                            }
                        }
                    ]
                }
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("--schema")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_FLOW_DEV_BIN", &flow_dev_exe)
        .env("GREENTIC_COMPONENT_DEV_BIN", &component_dev_exe)
        .output()
        .expect("run wizard --schema --answers");
    assert!(output.status.success(), "wizard --schema should succeed");

    let schema: Value = serde_json::from_slice(&output.stdout).expect("parse wizard schema");
    assert_eq!(
        schema
            .get("$defs")
            .and_then(|v| v.get("greentic_flow_wizard_runtime_schema"))
            .and_then(|v| v.get("title"))
            .and_then(Value::as_str),
        Some("pack-specific-flow-schema")
    );
    let flow_args = fs::read_to_string(&flow_log).expect("read flow log");
    assert!(flow_args.contains("wizard --schema"));
    assert!(flow_args.contains(&pack_dir.display().to_string()));
    assert!(flow_args.contains("--answers"));
    let component_args = fs::read_to_string(&component_log).expect("read component log");
    assert!(component_args.contains("wizard --schema --mode create"));
}

#[test]
fn wizard_apply_replays_nested_flow_step_and_component_answers_without_dropping_them() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let flow_exe = temp.path().join("greentic-flow");
    let component_exe = temp.path().join("greentic-component");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");
    write_script(
        &flow_exe,
        r#"#!/usr/bin/env bash
answers=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--answers" ]; then answers="$2"; shift 2; continue; fi
  shift
done
if [ -n "$answers" ]; then cp "$answers" "$PWD/flow.replayed.json"; fi
exit 0
"#,
    );
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
answers=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--qa-answers" ]; then answers="$2"; shift 2; continue; fi
  shift
done
if [ -n "$answers" ]; then cp "$answers" "$PWD/component.replayed.json"; fi
exit 0
"#,
    );
    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_delegate_flow": true,
                "run_delegate_component": true,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "flow_wizard_answers": {
                    "schema_id": "greentic-flow.wizard.plan",
                    "schema_version": "2.0.0",
                    "actions": [
                        {
                            "action": "add-step",
                            "flow": "flows/order_tracking_flow.ygtc",
                            "component": "components/order-status.wasm",
                            "mode": "setup",
                            "answers": {
                                "tenant": "acme",
                                "card": {
                                    "template": "adaptive-card"
                                }
                            }
                        },
                        {
                            "action": "update-step",
                            "flow": "flows/order_tracking_flow.ygtc",
                            "step_id": "router",
                            "component": "components/router.wasm",
                            "mode": "default",
                            "operation": "handle_message",
                            "answers": {
                                "prompt": "route"
                            },
                            "routing": [
                                {
                                    "condition": "response.action == \"create_research_plan\"",
                                    "to": "research_planner"
                                },
                                {
                                    "condition": "response.action == \"start_research_analysis\"",
                                    "to": "research_analyst"
                                },
                                {
                                    "out": true
                                }
                            ],
                            "in_map": {
                                "payload": {
                                    "question": "{{entry.input.metadata.user_question}}"
                                }
                            },
                            "out_map": {
                                "planner_output": "{{response.plan}}"
                            },
                            "err_map": {
                                "error": "{{error.message}}"
                            }
                        }
                    ]
                },
                "component_wizard_answers": {
                    "wizard_id": "greentic-component.wizard.run",
                    "schema_id": "greentic-component.wizard.run",
                    "schema_version": "1.0.0",
                    "answers": {
                        "mode": "create",
                        "fields": {
                            "component_name": "order-status"
                        }
                    },
                    "locks": {}
                }
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_FLOW_BIN", &flow_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");

    let replayed_flow: Value = serde_json::from_slice(
        &fs::read(pack_dir.join("flow.replayed.json")).expect("read replayed flow answers"),
    )
    .expect("parse replayed flow answers");
    assert_eq!(
        replayed_flow
            .get("actions")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("answers"))
            .and_then(|v| v.get("card"))
            .and_then(|v| v.get("template"))
            .and_then(Value::as_str),
        Some("adaptive-card")
    );
    let update_action = replayed_flow
        .get("actions")
        .and_then(Value::as_array)
        .and_then(|items| items.get(1))
        .expect("second replayed flow action");
    let routes = update_action
        .get("routing")
        .and_then(Value::as_array)
        .expect("routing array");
    assert_eq!(
        routes
            .first()
            .and_then(|route| route.get("condition"))
            .and_then(Value::as_str),
        Some("response.action == \"create_research_plan\"")
    );
    assert_eq!(
        routes
            .get(1)
            .and_then(|route| route.get("condition"))
            .and_then(Value::as_str),
        Some("response.action == \"start_research_analysis\"")
    );
    assert_eq!(
        update_action.get("operation").and_then(Value::as_str),
        Some("handle_message")
    );
    assert_eq!(
        update_action
            .get("in_map")
            .and_then(|v| v.get("payload"))
            .and_then(|v| v.get("question"))
            .and_then(Value::as_str),
        Some("{{entry.input.metadata.user_question}}")
    );
    assert_eq!(
        update_action
            .get("out_map")
            .and_then(|v| v.get("planner_output"))
            .and_then(Value::as_str),
        Some("{{response.plan}}")
    );
    assert_eq!(
        update_action
            .get("err_map")
            .and_then(|v| v.get("error"))
            .and_then(Value::as_str),
        Some("{{error.message}}")
    );
    let replayed_component: Value = serde_json::from_slice(
        &fs::read(pack_dir.join("component.replayed.json"))
            .expect("read replayed component answers"),
    )
    .expect("parse replayed component answers");
    assert_eq!(
        replayed_component
            .get("answers")
            .and_then(|v| v.get("fields"))
            .and_then(|v| v.get("component_name"))
            .and_then(Value::as_str),
        Some("order-status")
    );
}

#[test]
fn wizard_apply_wraps_simple_component_wizard_answers_for_qa_replay() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let component_exe = temp.path().join("greentic-component");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
answers=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--qa-answers" ]; then answers="$2"; shift 2; continue; fi
  shift
done
if [ -n "$answers" ]; then cp "$answers" "$PWD/component.replayed.json"; fi
exit 0
"#,
    );
    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_delegate_component": true,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "component_wizard_answers": {
                    "component_name": "component-rust-qa",
                    "output_dir": "components/component-rust-qa",
                    "abi_version": "0.6.0",
                    "http_client": false,
                    "messaging_inbound": false,
                    "messaging_outbound": false,
                    "secrets_enabled": false,
                    "secret_keys": []
                }
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run wizard apply");
    assert_cli_success(&output, "wizard apply should succeed");

    let replayed_component: Value = serde_json::from_slice(
        &fs::read(pack_dir.join("component.replayed.json"))
            .expect("read replayed component answers"),
    )
    .expect("parse replayed component answers");
    assert_eq!(
        replayed_component.get("schema").and_then(Value::as_str),
        Some("component-wizard-run/v1")
    );
    assert_eq!(
        replayed_component.get("mode").and_then(Value::as_str),
        Some("create")
    );
    assert_eq!(
        replayed_component
            .get("fields")
            .and_then(|v| v.get("component_name"))
            .and_then(Value::as_str),
        Some("component-rust-qa")
    );
}

#[test]
fn wizard_apply_component_delegate_failure_prints_replay_json() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let component_exe = temp.path().join("greentic-component");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_script(&self_exe, "#!/usr/bin/env bash\nexit 0\n");
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
echo "missing field schema" >&2
exit 1
"#,
    );
    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_delegate_component": true,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "component_wizard_answers": {
                    "schema": "component-wizard-run/v1",
                    "mode": "create",
                    "fields": {
                        "component_name": "component-rust-qa"
                    }
                }
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run wizard apply");
    assert!(!output.status.success(), "wizard apply should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing field schema"));
    assert!(stderr.contains("component_wizard_answers JSON passed to greentic-component"));
    assert!(stderr.contains("\"schema\": \"component-wizard-run/v1\""));
    assert!(stderr.contains("\"component_name\": \"component-rust-qa\""));
}

#[test]
fn wizard_apply_rejects_custom_component_operation_names_before_delegate() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    let self_exe = temp.path().join("greentic-pack-self");
    let component_exe = temp.path().join("greentic-component");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_script(
        &self_exe,
        "#!/usr/bin/env bash\nif [ \"$1\" = \"new\" ] && [ \"$2\" = \"--dir\" ]; then mkdir -p \"$3\"; fi\nexit 0\n",
    );
    write_script(
        &component_exe,
        r#"#!/usr/bin/env bash
touch "$PWD/component-delegate-called"
exit 0
"#,
    );
    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "create_pack_scaffold": true,
                "create_pack_id": "custom-ops-pack",
                "run_delegate_component": true,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "component_wizard_answers": {
                    "schema": "component-wizard-run/v1",
                    "mode": "create",
                    "fields": {
                        "component_name": "component-rust-qa",
                        "output_dir": "components/component-rust-qa",
                        "operation_names": ["answer-question"],
                        "abi_version": "0.6.0"
                    }
                }
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&answers_path)
        .env("GREENTIC_PACK_WIZARD_SELF_EXE", &self_exe)
        .env("GREENTIC_COMPONENT_BIN", &component_exe)
        .output()
        .expect("run wizard apply");
    assert!(!output.status.success(), "wizard apply should fail");
    assert!(
        !pack_dir.join("component-delegate-called").exists(),
        "component delegate should not run after unsupported operation_names"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("operation_names"));
    assert!(stderr.contains("not supported"));
    assert!(stderr.contains("greentic-component wizard add-operation"));
}

#[test]
fn wizard_apply_selected_actions_can_infer_update_extension_operation() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let create_answers = temp.path().join("create.json");
    let update_answers = temp.path().join("update.json");

    fs::write(
        &create_answers,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "create_pack_scaffold": true,
                "create_pack_id": "selected-actions-extension-op",
                "selected_actions": ["main.create_application_pack", "create_application_pack.start", "main.exit"],
                "run_doctor": false,
                "run_build": false,
                "sign": false
            },
            "locks": {}
        }))
        .expect("serialize create answers"),
    )
    .expect("write create answers");
    let create_output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&create_answers)
        .output()
        .expect("run create apply");
    assert!(
        create_output.status.success(),
        "create apply should succeed"
    );

    fs::write(
        &update_answers,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "selected_actions": ["main.update_extension_pack", "update_extension_pack.edit_entries", "main.exit"],
                "extension_catalog_ref": default_catalog_ref(),
                "extension_type_id": "messaging",
                "extension_edit_answers": {
                    "entry_label": "messaging",
                    "create_offer": "false",
                    "offer_id": "messaging-offer",
                    "cap_id": "greentic.cap.messaging.provider.v1",
                    "component_ref": "provider",
                    "op": "send",
                    "version": "v1",
                    "priority": "0",
                    "requires_setup": "false",
                    "qa_ref": "qa/setup.json",
                    "hook_op_names": ""
                },
                "run_doctor": false,
                "run_build": false,
                "sign": false
            },
            "locks": {}
        }))
        .expect("serialize update answers"),
    )
    .expect("write update answers");
    let update_output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&update_answers)
        .output()
        .expect("run update apply");
    assert!(
        update_output.status.success(),
        "update apply should succeed"
    );
    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    assert!(pack_yaml.contains("greentic.ext.capabilities.v1"));
}

#[test]
fn wizard_apply_messaging_webchat_gui_writes_provider_extension_entry() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let create_answers = temp.path().join("create.json");
    let update_answers = temp.path().join("update.json");

    fs::write(
        &create_answers,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "create_pack_scaffold": true,
                "create_pack_id": "messaging-webchat-gui-provider",
                "selected_actions": ["main.create_application_pack", "create_application_pack.start", "main.exit"],
                "run_doctor": false,
                "run_build": false,
                "sign": false
            },
            "locks": {}
        }))
        .expect("serialize create answers"),
    )
    .expect("write create answers");
    let create_output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&create_answers)
        .output()
        .expect("run create apply");
    assert!(
        create_output.status.success(),
        "create apply should succeed"
    );

    fs::write(
        &update_answers,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "selected_actions": ["main.update_extension_pack", "update_extension_pack.edit_entries", "main.exit"],
                "extension_operation": "update_extension_pack",
                "extension_catalog_ref": default_catalog_ref(),
                "extension_type_id": "messaging-webchat-gui",
                "extension_edit_answers": {
                    "entry_label": "messaging-webchat-gui"
                },
                "run_doctor": false,
                "run_build": false,
                "sign": false
            },
            "locks": {}
        }))
        .expect("serialize update answers"),
    )
    .expect("write update answers");
    let update_output = pack_cmd()
        .arg("wizard")
        .arg("apply")
        .arg("--answers")
        .arg(&update_answers)
        .output()
        .expect("run update apply");
    assert!(
        update_output.status.success(),
        "update apply should succeed"
    );
    let pack_yaml = fs::read_to_string(pack_dir.join("pack.yaml")).expect("read pack yaml");
    assert!(pack_yaml.contains("greentic.provider-extension.v1"));
    assert!(pack_yaml.contains("provider_type: messaging-webchat-gui"));
}

fn write_extension_dependency_answers(
    answers_path: &Path,
    pack_dir: &Path,
    extension_dependencies: Value,
) {
    fs::write(
        answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "extension_dependencies": extension_dependencies
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");
}

fn http_store_dependency() -> Value {
    // Exactly packc's `ExtensionDependency` serde shape. A version-tag store ref
    // (vs a `@sha256:` digest) is only valid with `allow_tags: true`, which the
    // resolver's own fixtures use too.
    json!({
        "id": "greentic.http",
        "role": "capability",
        "source": {
            "kind": "store",
            "ref": "store://greentic.http@1.2.0",
            "allow_tags": true
        }
    })
}

fn read_extensions_json(pack_dir: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(pack_dir.join("pack.extensions.json")).expect("read pack.extensions.json"),
    )
    .expect("parse pack.extensions.json")
}

#[test]
fn wizard_apply_writes_extension_dependencies_file_from_answers() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_extension_dependency_answers(&answers_path, &pack_dir, json!([http_store_dependency()]));

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");

    let doc = read_extensions_json(&pack_dir);
    assert_eq!(doc.get("version").and_then(Value::as_u64), Some(1));
    let extensions = doc
        .get("extensions")
        .and_then(Value::as_array)
        .expect("extensions array");
    assert_eq!(extensions.len(), 1, "exactly one dependency expected");
    let dep = &extensions[0];
    assert_eq!(dep.get("id").and_then(Value::as_str), Some("greentic.http"));
    assert_eq!(dep.get("role").and_then(Value::as_str), Some("capability"));
    let source = dep
        .get("source")
        .and_then(Value::as_object)
        .expect("source");
    assert_eq!(source.get("kind").and_then(Value::as_str), Some("store"));
    assert_eq!(
        source.get("ref").and_then(Value::as_str),
        Some("store://greentic.http@1.2.0")
    );
    assert_eq!(
        source.get("allow_tags").and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn wizard_apply_extension_dependencies_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    write_extension_dependency_answers(&answers_path, &pack_dir, json!([http_store_dependency()]));

    let first = run_wizard_answers("apply", &answers_path);
    assert!(first.status.success(), "first apply should succeed");
    let second = run_wizard_answers("apply", &answers_path);
    assert!(second.status.success(), "second apply should succeed");

    let extensions = read_extensions_json(&pack_dir)
        .get("extensions")
        .and_then(Value::as_array)
        .expect("extensions array")
        .clone();
    assert_eq!(
        extensions.len(),
        1,
        "applying the same dependency twice must not duplicate it"
    );
}

#[test]
fn wizard_apply_extension_dependencies_preserves_existing_manual_entries() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    // A hand-authored pack.extensions.json with an unrelated entry.
    fs::write(
        pack_dir.join("pack.extensions.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "extensions": [
                {
                    "id": "greentic.deployer.v1",
                    "role": "deployer",
                    "source": {
                        "kind": "file",
                        "ref": "file:///tmp/deployer.json",
                        "allow_tags": false
                    }
                }
            ]
        }))
        .expect("serialize existing file"),
    )
    .expect("write existing pack.extensions.json");

    write_extension_dependency_answers(&answers_path, &pack_dir, json!([http_store_dependency()]));

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");

    let extensions = read_extensions_json(&pack_dir)
        .get("extensions")
        .and_then(Value::as_array)
        .expect("extensions array")
        .clone();
    let ids: Vec<&str> = extensions
        .iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    assert!(
        ids.contains(&"greentic.deployer.v1"),
        "manual entry must be preserved: {ids:?}"
    );
    assert!(
        ids.contains(&"greentic.http"),
        "supplied entry must be added: {ids:?}"
    );
    assert_eq!(extensions.len(), 2, "exactly two entries expected: {ids:?}");
}

#[test]
fn wizard_apply_empty_extension_dependencies_leaves_existing_file_untouched() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    let original = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "extensions": [
            {
                "id": "greentic.deployer.v1",
                "role": "deployer",
                "source": {
                    "kind": "file",
                    "ref": "file:///tmp/deployer.json",
                    "allow_tags": false
                }
            }
        ]
    }))
    .expect("serialize existing file");
    fs::write(pack_dir.join("pack.extensions.json"), &original)
        .expect("write existing pack.extensions.json");

    write_extension_dependency_answers(&answers_path, &pack_dir, json!([]));

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");

    let after = fs::read(pack_dir.join("pack.extensions.json")).expect("read after");
    assert_eq!(
        after, original,
        "empty extension_dependencies must not clobber an existing file"
    );
}

#[test]
fn wizard_apply_absent_extension_dependencies_creates_no_file() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    // No extension_dependencies key at all.
    fs::write(
        &answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");
    assert!(
        !pack_dir.join("pack.extensions.json").exists(),
        "absent extension_dependencies must not create a file"
    );
}

#[test]
fn wizard_apply_extension_dependencies_supplied_entry_wins_on_conflict() {
    let temp = TempDir::new().expect("tempdir");
    let pack_dir = temp.path().join("pack");
    let answers_path = temp.path().join("answers.json");
    fs::create_dir_all(&pack_dir).expect("create pack dir");

    // Pre-existing entry with the SAME id but a different source ref.
    fs::write(
        pack_dir.join("pack.extensions.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "extensions": [
                {
                    "id": "greentic.http",
                    "role": "capability",
                    "source": {
                        "kind": "store",
                        "ref": "store://greentic.http@1.0.0",
                        "allow_tags": true
                    }
                }
            ]
        }))
        .expect("serialize existing file"),
    )
    .expect("write existing pack.extensions.json");

    write_extension_dependency_answers(&answers_path, &pack_dir, json!([http_store_dependency()]));

    let output = run_wizard_answers("apply", &answers_path);
    assert_cli_success(&output, "wizard apply should succeed");

    let extensions = read_extensions_json(&pack_dir)
        .get("extensions")
        .and_then(Value::as_array)
        .expect("extensions array")
        .clone();
    assert_eq!(extensions.len(), 1, "conflict must dedupe by id");
    assert_eq!(
        extensions[0]
            .get("source")
            .and_then(|s| s.get("ref"))
            .and_then(Value::as_str),
        Some("store://greentic.http@1.2.0"),
        "supplied dependency must win over conflicting existing entry"
    );
}

fn write_script(path: &std::path::Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn copy_tree(src: &std::path::Path, dest: &std::path::Path) {
    fs::create_dir_all(dest).expect("create destination dir");
    for entry in WalkDir::new(src).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_symlink() {
            continue;
        }
        let rel = entry.path().strip_prefix(src).expect("relative path");
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).expect("create dir");
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

fn write_asset_answers(answers_path: &Path, pack_dir: &Path, asset_staging: Value) {
    if let Some(parent) = answers_path.parent() {
        fs::create_dir_all(parent).expect("create answers parent");
    }
    fs::write(
        answers_path,
        serde_json::to_vec_pretty(&json!({
            "wizard_id": "greentic-pack.wizard.run",
            "schema_id": "greentic-pack.wizard.answers",
            "schema_version": "1.0.0",
            "locale": "en-GB",
            "answers": {
                "pack_dir": pack_dir,
                "run_doctor": false,
                "run_build": false,
                "sign": false,
                "asset_staging": asset_staging
            },
            "locks": {}
        }))
        .expect("serialize answers"),
    )
    .expect("write answers");
}

fn run_wizard_answers(command: &str, answers_path: &Path) -> std::process::Output {
    pack_cmd()
        .current_dir(workspace_root())
        .arg("wizard")
        .arg(command)
        .arg("--answers")
        .arg(answers_path)
        .output()
        .expect("run wizard command")
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
    let mut snapshot = BTreeMap::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let rel = entry.path().strip_prefix(root).expect("relative path");
        if rel.as_os_str().is_empty() {
            continue;
        }
        let key = rel.display().to_string();
        if entry.file_type().is_dir() {
            snapshot.insert(key, "dir".to_string());
        } else {
            let content = fs::read(entry.path()).expect("read snapshot file");
            snapshot.insert(key, format!("file:{content:?}"));
        }
    }
    snapshot
}

fn default_catalog_ref() -> String {
    format!(
        "file://{}",
        workspace_root()
            .join("docs")
            .join("extensions_capability_packs.catalog.v1.json")
            .display()
    )
}
