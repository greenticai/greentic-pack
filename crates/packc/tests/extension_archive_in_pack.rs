//! Integration: an agent's tool extension travels INSIDE the built `.gtpack`.
//!
//! packc already opened each declared `.gtxpack` to generate the credential
//! setup form, so a cloud operator was asked for a tool's API key for a tool
//! that was never shipped — the runner scanned an empty extensions directory
//! and dropped it with a warning. These tests assert on the produced archive's
//! bytes, not on any function having been called: a pack that does not carry
//! the extension is the exact defect, however the copy is implemented.

use std::io::{Read, Write};
use std::path::Path;

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Build a minimal `.gtxpack` whose `describe.json` declares one tool with a
/// secret requirement — the same shape the setup-form generator reads.
fn make_gtxpack(dir: &Path, file_name: &str) -> std::path::PathBuf {
    let describe = r#"{"contributions":{"tools":[
      {"name":"tavily_search","secret_requirements":[
        {"key":"tavily/api_key","required":true,"description":"Tavily web-search API key.","format":"text"}]}
    ]}}"#;
    let path = dir.join(file_name);
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        zip.start_file("describe.json", zip::write::FileOptions::<()>::default())
            .unwrap();
        zip.write_all(describe.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    std::fs::write(&path, buf).unwrap();
    path
}

fn read_zip_bytes(gtpack: &Path, name: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(gtpack).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut content = Vec::new();
    entry.read_to_end(&mut content).ok()?;
    Some(content)
}

fn zip_entry_names(gtpack: &Path) -> Vec<String> {
    let bytes = std::fs::read(gtpack).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect()
}

/// Write a pack source whose single agent binds `tool_ext_id`, declaring
/// `declared_ext_id` in `pack.extensions.json` against `gtxpack_path`.
fn write_pack_source(pack: &Path, declared_ext_id: &str, gtxpack_path: &Path) {
    write_file(
        &pack.join("pack.yaml"),
        &format!(
            r#"pack_id: demo
version: 0.1.0
kind: application
publisher: Test
components: []
dependencies: []
flows: []
agents:
  a:
    agent_id: a
    llm:
      provider: deepseek
      model: deepseek-chat
      credential_ref: deepseek
    tools:
      - extension_id: {declared_ext_id}
        tool_name: tavily_search
"#
        ),
    );

    let extensions_json = serde_json::json!({
        "version": 1,
        "extensions": [{
            "id": declared_ext_id,
            "role": "tool",
            "source": {
                "kind": "file",
                "ref": format!("file://{}", gtxpack_path.display())
            }
        }]
    });
    write_file(
        &pack.join("pack.extensions.json"),
        &extensions_json.to_string(),
    );
}

fn run_build(pack: &Path, gtpack_out: &Path) -> std::process::Output {
    std::process::Command::new(assert_cmd::cargo::cargo_bin!("greentic-pack"))
        .current_dir(pack)
        .args([
            "build",
            "--in",
            pack.to_str().unwrap(),
            "--no-update",
            "--gtpack-out",
            gtpack_out.to_str().unwrap(),
        ])
        .output()
        .expect("run packc build")
}

#[test]
fn the_built_gtpack_carries_the_tool_extension_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");
    let gtx = make_gtxpack(tmp.path(), "greentic.tavily.gtxpack");
    write_pack_source(&pack, "greentic.tavily", &gtx);

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let carried =
        read_zip_bytes(&gtpack_out, "extensions/greentic.tavily.gtxpack").unwrap_or_else(|| {
            panic!(
                "the .gtpack must carry the extension archive; entries were {:?}",
                zip_entry_names(&gtpack_out)
            )
        });
    let source = std::fs::read(&gtx).unwrap();
    assert_eq!(
        carried, source,
        "the carried archive must be the declared .gtxpack byte-for-byte"
    );

    // The archive is a real, readable `.gtxpack`, not merely bytes of the right
    // length: a consumer must be able to open it and find describe.json.
    let mut inner = zip::ZipArchive::new(std::io::Cursor::new(carried)).expect("carried .gtxpack");
    let mut describe = String::new();
    inner
        .by_name("describe.json")
        .expect("carried .gtxpack still has describe.json")
        .read_to_string(&mut describe)
        .unwrap();
    assert!(describe.contains("tavily_search"), "got {describe}");
}

/// The credential form and the shipped archive must move together: a pack that
/// asks for a tool's secret and does not carry the tool is the whole defect.
#[test]
fn the_credential_form_and_the_carried_archive_ship_together() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");
    let gtx = make_gtxpack(tmp.path(), "greentic.tavily.gtxpack");
    write_pack_source(&pack, "greentic.tavily", &gtx);

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);
    assert!(output.status.success(), "packc build failed");

    let requirements = read_zip_bytes(&gtpack_out, "assets/secret-requirements.json")
        .expect("secret-requirements.json must ship");
    assert!(
        String::from_utf8_lossy(&requirements).contains("tavily/api_key"),
        "the operator is asked for the tool credential"
    );
    assert!(
        read_zip_bytes(&gtpack_out, "extensions/greentic.tavily.gtxpack").is_some(),
        "…so the tool itself must ship in the same pack"
    );
}

/// An extension id reaches this code from stored configuration. A traversal in
/// one must land under `extensions/` like any other, never outside the archive.
#[test]
fn a_hostile_extension_id_stays_inside_the_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");
    let gtx = make_gtxpack(tmp.path(), "hostile.gtxpack");
    write_pack_source(&pack, "../../../../etc/evil", &gtx);

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let carried: Vec<String> = zip_entry_names(&gtpack_out)
        .into_iter()
        .filter(|name| name.ends_with(".gtxpack"))
        .collect();
    assert_eq!(
        carried.len(),
        1,
        "exactly one extension archive must ship; got {carried:?}"
    );
    let name = &carried[0];
    // The real property: the entry is exactly two ordinary path components,
    // `extensions/` plus one file name. No separator, no `..`, no leading dot —
    // so nothing can address anything outside the archive.
    let components: Vec<_> = Path::new(name.as_str()).components().collect();
    assert!(
        matches!(
            components.as_slice(),
            [
                std::path::Component::Normal(dir),
                std::path::Component::Normal(file)
            ] if *dir == std::ffi::OsStr::new("extensions")
                && !file.to_string_lossy().starts_with('.')
        ),
        "a hostile id must not produce a traversing entry; got {name}"
    );
}

/// A pack with no extension-backed tools must not gain an `extensions/` entry.
#[test]
fn a_pack_with_no_extension_tools_carries_no_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");

    write_file(
        &pack.join("pack.yaml"),
        r#"pack_id: demo
version: 0.1.0
kind: application
publisher: Test
components: []
dependencies: []
flows: []
agents:
  a:
    agent_id: a
    llm:
      provider: deepseek
      model: deepseek-chat
      credential_ref: deepseek
    tools:
      - extension_id: "flow:get_weather"
        tool_name: get_weather
"#,
    );

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);
    assert!(
        output.status.success(),
        "packc build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let carried: Vec<String> = zip_entry_names(&gtpack_out)
        .into_iter()
        .filter(|name| name.starts_with("extensions/"))
        .collect();
    assert!(
        carried.is_empty(),
        "expected no extension archives; got {carried:?}"
    );
}

/// A declared extension whose archive cannot be read is FATAL, not a warning:
/// a green build producing a pack missing an extension is exactly the silence
/// this change exists to remove.
#[test]
fn an_unreadable_extension_dependency_fails_the_build() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");
    let missing = tmp.path().join("absent.gtxpack");
    write_pack_source(&pack, "greentic.tavily", &missing);

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);
    assert!(
        !output.status.success(),
        "an unreadable extension dependency must fail the build, not warn"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("greentic.tavily"),
        "the failure must name the extension; got {stderr}"
    );
    assert!(
        !gtpack_out.exists(),
        "no .gtpack may be produced when an extension could not be acquired"
    );
}

/// The duplicate check at the write site is reachable WITHOUT any hash
/// collision, and this is the case that reaches it.
///
/// `is_reserved_extra_file` excludes `.gtpack` but not `.gtxpack`, and
/// `map_extra_files` passes any `/`-containing logical path through verbatim.
/// So a file the author happens to leave at `<pack>/extensions/<id>.gtxpack`
/// is carried into the archive as an extra — and extras are written before the
/// resolved extension archives, so it claims the entry first.
///
/// Silently skipping there would ship a pack whose `extensions/<id>.gtxpack` is
/// some unrelated file while the setup form still asks for that tool's
/// credential: the exact defect this feature exists to close, one layer up. The
/// build must refuse and name both sides.
#[test]
fn a_source_file_squatting_the_entry_fails_the_build_loudly() {
    let tmp = tempfile::tempdir().unwrap();
    let pack = tmp.path().join("pack");
    let gtx = make_gtxpack(tmp.path(), "greentic.tavily.gtxpack");
    write_pack_source(&pack, "greentic.tavily", &gtx);

    // A DIFFERENT file, sitting on the entry the resolved archive will want.
    write_file(
        &pack.join("extensions/greentic.tavily.gtxpack"),
        "not the extension the agent declared",
    );

    let gtpack_out = tmp.path().join("demo.gtpack");
    let output = run_build(&pack, &gtpack_out);

    assert!(
        !output.status.success(),
        "a squatted extension entry must fail the build:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("greentic.tavily"),
        "the failure must name the extension; got {stderr}"
    );
    assert!(
        stderr.contains("extensions/greentic.tavily.gtxpack"),
        "the failure must name the contested entry; got {stderr}"
    );
}
