pub fn canonical_wit_root() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );

    let workspace_root = manifest_dir.join("../../wit");
    if has_wit_files(&workspace_root) {
        return workspace_root
            .canonicalize()
            .expect("Failed to locate canonical WIT root");
    }

    let local = manifest_dir.join("wit");
    if has_wit_files(&local) {
        return local
            .canonicalize()
            .expect("Failed to locate canonical WIT root");
    }

    let workspace_sibling = manifest_dir.join("../greentic-interfaces/wit");
    if has_wit_files(&workspace_sibling) {
        return workspace_sibling
            .canonicalize()
            .expect("Failed to locate canonical WIT root");
    }

    panic!("Failed to locate canonical WIT root")
}

fn has_wit_files(root: &std::path::Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("wit") {
                return true;
            }
        }
    }
    false
}
