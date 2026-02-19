use std::collections::BTreeSet;
use std::path::Path;

use wit_parser::Resolve;

fn collect_package_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = std::fs::read_dir(root)
        .unwrap_or_else(|_| panic!("missing WIT root at {}", root.display()));
    for entry in entries {
        let entry = entry.expect("read wit entry");
        let path = entry.path();
        if path.is_dir() {
            collect_package_files(&path, out);
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("package.wit") {
            out.push(path);
        }
    }
}

#[test]
fn staged_wit_packages_are_valid() {
    let staged_root = Path::new(env!("WIT_STAGING_DIR"));
    let entries = std::fs::read_dir(staged_root)
        .unwrap_or_else(|_| panic!("missing staged WIT packages in {}", staged_root.display()));

    for entry in entries {
        let entry = entry.expect("read staged entry");
        if !entry.path().is_dir() {
            continue;
        }
        let mut resolve = Resolve::new();
        resolve
            .push_dir(entry.path())
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", entry.path().display()));
    }
}

#[test]
fn crate_local_wit_interfaces_do_not_redefine_imported_types() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wit_root = manifest_dir.join("wit");
    let mut package_files = Vec::new();
    collect_package_files(&wit_root, &mut package_files);
    package_files.sort();
    assert!(
        !package_files.is_empty(),
        "no package.wit files discovered under {}",
        wit_root.display()
    );

    for package_file in package_files {
        let contents = std::fs::read_to_string(&package_file)
            .unwrap_or_else(|_| panic!("failed to read {}", package_file.display()));
        let mut in_interface = false;
        let mut depth: i32 = 0;
        let mut imported = BTreeSet::new();
        let mut local_defs = BTreeSet::new();
        let mut interface_name = String::new();

        for raw_line in contents.lines() {
            let line = raw_line.trim();
            if !in_interface {
                if let Some(rest) = line.strip_prefix("interface ") {
                    interface_name = rest
                        .split_whitespace()
                        .next()
                        .unwrap_or("<unknown>")
                        .trim_end_matches('{')
                        .to_string();
                    in_interface = true;
                    depth += line.matches('{').count() as i32;
                    depth -= line.matches('}').count() as i32;
                    imported.clear();
                    local_defs.clear();
                }
                continue;
            }

            if let Some(rest) = line.strip_prefix("use ")
                && let Some(start) = rest.find('{')
                && let Some(end) = rest[start + 1..].find('}')
            {
                for name in rest[start + 1..start + 1 + end].split(',') {
                    let ty = name.trim();
                    if !ty.is_empty() {
                        imported.insert(ty.to_string());
                    }
                }
            }

            for keyword in ["record", "enum", "variant", "flags", "resource", "type"] {
                if let Some(rest) = line.strip_prefix(&format!("{keyword} ")) {
                    let name = rest
                        .split(|c: char| c.is_whitespace() || c == '{' || c == '=')
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !name.is_empty() {
                        local_defs.insert(name.to_string());
                    }
                }
            }

            depth += line.matches('{').count() as i32;
            depth -= line.matches('}').count() as i32;
            if depth <= 0 {
                let overlap: Vec<String> = imported.intersection(&local_defs).cloned().collect();
                assert!(
                    overlap.is_empty(),
                    "{} interface `{}` redefines imported types: {}",
                    package_file.display(),
                    interface_name,
                    overlap.join(", ")
                );
                in_interface = false;
                depth = 0;
            }
        }
    }
}

#[test]
fn oauth_broker_worlds_include_client() {
    use std::collections::BTreeSet;

    let staged_root = Path::new(env!("WIT_STAGING_DIR"));
    let package_dir = staged_root.join("greentic-oauth-broker-1.0.0");

    assert!(
        package_dir.exists(),
        "staged oauth-broker package missing at {}",
        package_dir.display()
    );

    let mut resolve = Resolve::new();
    let (pkg, _) = resolve
        .push_dir(&package_dir)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", package_dir.display()));

    let worlds: BTreeSet<String> = resolve.packages[pkg]
        .worlds
        .keys()
        .map(|name| name.to_string())
        .collect();

    assert!(
        worlds.contains("broker"),
        "expected existing broker world to remain"
    );
    assert!(
        worlds.contains("broker-client"),
        "expected additive broker-client world to be staged"
    );
}

#[test]
fn component_v0_v6_exports_node_interface() {
    use std::collections::BTreeSet;
    use wit_parser::WorldKey;

    let staged_root = Path::new(env!("WIT_STAGING_DIR"));
    let package_dir = staged_root.join("greentic-component-0.6.0");

    assert!(
        package_dir.exists(),
        "staged component package missing at {}",
        package_dir.display()
    );

    let mut resolve = Resolve::new();
    let (pkg, _) = resolve
        .push_dir(&package_dir)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", package_dir.display()));

    let world_id = resolve.packages[pkg]
        .worlds
        .get("component")
        .copied()
        .expect("missing component world");

    let world = &resolve.worlds[world_id];
    let export_names: BTreeSet<String> = world
        .exports
        .keys()
        .filter_map(|key| match key {
            WorldKey::Name(name) => Some(name.clone()),
            WorldKey::Interface(id) => resolve.interfaces[*id].name.clone(),
        })
        .collect();
    let import_names: BTreeSet<String> = world
        .imports
        .keys()
        .filter_map(|key| match key {
            WorldKey::Name(name) => Some(name.clone()),
            WorldKey::Interface(id) => resolve.interfaces[*id].name.clone(),
        })
        .collect();

    assert!(
        export_names.contains("node"),
        "expected component world to export node"
    );
    assert!(
        import_names.contains("control"),
        "expected component world to import control"
    );
}

#[test]
fn no_legacy_component_v0_v6_wit_mirrors_exist() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let guest =
        manifest_dir.join("../greentic-interfaces-guest/wit/greentic/component@0.6.0/package.wit");
    let wasmtime = manifest_dir
        .join("../greentic-interfaces-wasmtime/wit/greentic/component@0.6.0/package.wit");

    assert!(
        !guest.exists(),
        "legacy guest mirror should not exist at {}",
        guest.display()
    );
    assert!(
        !wasmtime.exists(),
        "legacy wasmtime mirror should not exist at {}",
        wasmtime.display()
    );
}
