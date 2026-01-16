use std::fs;

use greentic_pack::pack_lock::{LockedComponent, PackLockV1, read_pack_lock, write_pack_lock};
use tempfile::TempDir;

#[test]
fn pack_lock_roundtrips_with_sorted_components() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("pack.lock.json");

    let lock = PackLockV1::new(vec![
        LockedComponent {
            name: "beta".into(),
            r#ref: "oci://example/beta:1.0.0".into(),
            digest: "sha256:bbbb".into(),
            component_id: None,
        },
        LockedComponent {
            name: "alpha".into(),
            r#ref: "repo://example/alpha?rev=1".into(),
            digest: "sha256:aaaa".into(),
            component_id: None,
        },
    ]);

    write_pack_lock(&path, &lock).expect("write pack.lock");
    let written = fs::read_to_string(&path).expect("read file");

    // Expect deterministic ordering by name (alpha before beta).
    assert!(
        written.find("alpha").unwrap() < written.find("beta").unwrap(),
        "components should be sorted deterministically"
    );

    let roundtrip = read_pack_lock(&path).expect("read pack.lock");
    assert_eq!(roundtrip.schema_version, 1);
    assert_eq!(roundtrip.components.len(), 2);
    assert_eq!(roundtrip.components[0].name, "alpha");
}
