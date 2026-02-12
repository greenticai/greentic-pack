use std::collections::BTreeMap;
use std::fs;

use greentic_pack::pack_lock::{
    LockedComponent, LockedOperation, PackLockV1, read_pack_lock, write_pack_lock,
};
use tempfile::TempDir;

#[test]
fn pack_lock_roundtrips_with_sorted_components() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("pack.lock.cbor");

    let mut components = BTreeMap::new();
    components.insert(
        "beta.component".to_string(),
        LockedComponent {
            component_id: "beta.component".to_string(),
            r#ref: Some("oci://example/beta:1.0.0".into()),
            abi_version: "0.6.0".to_string(),
            resolved_digest: "sha256:bbbb".into(),
            describe_hash: "b".repeat(64),
            operations: vec![LockedOperation {
                operation_id: "run".into(),
                schema_hash: "c".repeat(64),
            }],
            world: Some("greentic:component/component@0.6.0".into()),
            component_version: Some("1.0.0".into()),
            role: Some("tool".into()),
        },
    );
    components.insert(
        "alpha.component".to_string(),
        LockedComponent {
            component_id: "alpha.component".to_string(),
            r#ref: Some("repo://example/alpha?rev=1".into()),
            abi_version: "0.6.0".to_string(),
            resolved_digest: "sha256:aaaa".into(),
            describe_hash: "a".repeat(64),
            operations: vec![
                LockedOperation {
                    operation_id: "alpha".into(),
                    schema_hash: "e".repeat(64),
                },
                LockedOperation {
                    operation_id: "zeta".into(),
                    schema_hash: "d".repeat(64),
                },
            ],
            world: None,
            component_version: None,
            role: None,
        },
    );
    let lock = PackLockV1::new(components);

    write_pack_lock(&path, &lock).expect("write pack.lock");
    let first = fs::read(&path).expect("read file");

    let roundtrip = read_pack_lock(&path).expect("read pack.lock");
    assert_eq!(roundtrip.version, 1);
    assert_eq!(roundtrip.components.len(), 2);
    let alpha = roundtrip.components.get("alpha.component").expect("alpha");
    assert_eq!(alpha.operations[0].operation_id, "alpha");

    write_pack_lock(&path, &roundtrip).expect("write pack.lock again");
    let second = fs::read(&path).expect("read file again");
    assert_eq!(first, second, "lock bytes should be deterministic");
}
