use std::path::PathBuf;

use packc::cli::inspect::{InspectArgs, InspectFormat, handle as inspect_handle};
use packc::new::{NewArgs, handle as new_handle};
use packc::runtime;
use tempfile::TempDir;

#[test]
fn offline_fixture_e2e_new_doctor() {
    let temp = TempDir::new().expect("temp dir");
    let pack_dir = temp.path().join("demo-pack");

    let runtime = runtime::resolve_runtime(None, None, true, None).expect("runtime");
    let rt = tokio::runtime::Runtime::new().expect("tokio");

    let new_args = NewArgs {
        dir: pack_dir.clone(),
        pack_id: "dev.local.demo".to_string(),
    };
    rt.block_on(new_handle(new_args, false, &runtime))
        .expect("new pack");

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

    let result = rt.block_on(inspect_handle(inspect_args, true, &runtime));
    assert!(result.is_ok(), "doctor should succeed for scaffold pack");
}
