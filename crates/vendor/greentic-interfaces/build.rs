use std::collections::{BTreeMap, HashSet};
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use wit_bindgen_core::Files;
use wit_bindgen_core::WorldGenerator;
use wit_bindgen_core::wit_parser::Resolve;
use wit_bindgen_rust::Opts;

const CANONICAL_INTERFACES_TYPES_REF: &str = "greentic:interfaces-types@0.1.0";
const CANONICAL_INTERFACES_TYPES_CANDIDATES: [&str; 2] =
    ["types.wit", "greentic/interfaces-types@0.1.0/package.wit"];

fn main() -> Result<(), Box<dyn Error>> {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch == "wasm32" {
        // Host bindings are not generated for wasm targets.
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let staged_root = manifest_dir.join("target").join("wit-staging");
    // Keep staging stable for `src/wit_all.rs` relative paths and avoid
    // concurrent build-script races by not deleting the shared directory.
    fs::create_dir_all(&staged_root)?;

    let wit_root = resolve_wit_root(&manifest_dir)?;
    let mut package_candidates = BTreeMap::new();
    discover_packages(&wit_root, &mut package_candidates)?;
    verify_interfaces_types_duplicates(&wit_root, &package_candidates)?;
    let catalog = PackageCatalog::new(&wit_root, package_candidates)?;

    for (_, package_path) in catalog.iter() {
        stage_package(package_path, &staged_root, &catalog)?;
    }

    let bindings_dir = generate_rust_bindings(&staged_root, &out_dir)?;
    ensure_tenant_ctx_contains_i18n(&bindings_dir)?;

    println!("cargo:rustc-env=WIT_STAGING_DIR={}", staged_root.display());
    println!(
        "cargo:rustc-env=GREENTIC_INTERFACES_BINDINGS={}",
        bindings_dir.display()
    );

    Ok(())
}

fn resolve_wit_root(manifest_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(explicit) = env::var("GREENTIC_INTERFACES_WIT_ROOT") {
        let explicit_root = PathBuf::from(explicit);
        if explicit_root.is_dir() {
            return Ok(explicit_root);
        }
        return Err(format!(
            "GREENTIC_INTERFACES_WIT_ROOT is set but not a directory: {}",
            explicit_root.display()
        )
        .into());
    }

    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("wit");
        if candidate.join("types.wit").is_file() {
            return Ok(candidate);
        }
    }

    let local_wit = manifest_dir.join("wit");
    if local_wit.is_dir() {
        return Ok(local_wit);
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "unable to locate WIT root; tried {} and workspace-relative wit/ from {}",
            local_wit.display(),
            manifest_dir.display()
        ),
    )
    .into())
}

fn stage_package(
    src_path: &Path,
    staged_root: &Path,
    catalog: &PackageCatalog,
) -> Result<(), Box<dyn Error>> {
    let package_ref = read_package_ref(src_path)?;
    let dest_dir = staged_root.join(sanitize(&package_ref));
    fs::create_dir_all(&dest_dir)?;
    fs::copy(src_path, dest_dir.join("package.wit"))?;
    println!("cargo:rerun-if-changed={}", src_path.display());

    stage_dependencies(&dest_dir, src_path, catalog)?;
    Ok(())
}

fn stage_dependencies(
    parent_dir: &Path,
    source_path: &Path,
    catalog: &PackageCatalog,
) -> Result<(), Box<dyn Error>> {
    let deps = parse_deps(source_path)?;
    if deps.is_empty() {
        return Ok(());
    }
    if env::var("DEBUG_STAGE_DEPS").is_ok() {
        eprintln!(
            "[debug] staging deps for {} -> {:?}",
            source_path.display(),
            deps
        );
    }

    let deps_dir = parent_dir.join("deps");
    fs::create_dir_all(&deps_dir)?;

    for dep in deps {
        let dep_src = catalog.resolve(&dep)?;
        let dep_dest = deps_dir.join(sanitize(&dep));
        fs::create_dir_all(&dep_dest)?;
        fs::copy(dep_src, dep_dest.join("package.wit"))?;
        if env::var("DEBUG_STAGE_DEPS").is_ok() {
            println!("cargo:warning=staging dependency {dep}");
        }
        println!("cargo:rerun-if-changed={}", dep_src.display());

        stage_dependencies(&dep_dest, dep_src, catalog)?;
    }

    Ok(())
}

fn read_package_ref(path: &Path) -> Result<String, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("package ") {
            return Ok(rest.trim_end_matches(';').trim().to_string());
        }
    }
    Err(format!("unable to locate package declaration in {}", path.display()).into())
}

fn parse_deps(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let mut deps = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim_start();
        let rest = if let Some(rest) = trimmed.strip_prefix("use ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            rest
        } else {
            continue;
        };

        let token = rest.split_whitespace().next().unwrap_or("");
        let token = token.trim_end_matches(';');
        let token = token.split(".{").next().unwrap_or(token);
        let token = token.split('{').next().unwrap_or(token);

        let (pkg_part, version_part) = match token.split_once('@') {
            Some(parts) => parts,
            None => continue,
        };

        let base_pkg = pkg_part.split('/').next().unwrap_or(pkg_part);
        let mut version = String::new();
        for ch in version_part.chars() {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                version.push(ch);
            } else {
                break;
            }
        }
        while version.ends_with('.') {
            version.pop();
        }
        if version.is_empty() {
            continue;
        }

        let dep_ref = format!("{base_pkg}@{}", normalize_wit_version(&version));
        if !deps.contains(&dep_ref) {
            deps.push(dep_ref);
        }
    }

    Ok(deps)
}

fn normalize_wit_version(version: &str) -> String {
    version
        .split('.')
        .map(|segment| {
            if segment.chars().all(|ch| ch.is_ascii_digit()) {
                let trimmed = segment.trim_start_matches('0');
                if trimmed.is_empty() {
                    "0".to_string()
                } else {
                    trimmed.to_string()
                }
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn sanitize(package_ref: &str) -> String {
    package_ref.replace([':', '@', '/'], "-")
}

fn generate_rust_bindings(staged_root: &Path, out_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let bindings_dir = out_dir.join("bindings");
    reset_directory(&bindings_dir)?;

    let mut package_paths = Vec::new();
    let mut inserted = HashSet::new();

    for entry in fs::read_dir(staged_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let package_path = path.join("package.wit");
        if !package_path.exists() {
            continue;
        }

        let package_ref = read_package_ref(&package_path)?;
        if !inserted.insert(package_ref) {
            continue;
        }

        package_paths.push(path);
    }

    if package_paths.is_empty() {
        return Err("no WIT worlds discovered to generate bindings for".into());
    }

    package_paths.sort();

    let opts = Opts {
        generate_all: true,
        generate_unused_types: true,
        ..Default::default()
    };

    let mut default_module = None;
    let mut mod_rs = String::new();

    for path in package_paths {
        let mut resolve = Resolve::new();
        let (pkg, _) = resolve.push_dir(&path)?;
        let package_name = resolve.packages[pkg].name.clone();

        let mut worlds: Vec<_> = resolve.packages[pkg]
            .worlds
            .iter()
            .map(|(name, id)| (name.to_string(), *id))
            .collect();
        worlds.sort_by(|(a_name, _), (b_name, _)| a_name.cmp(b_name));

        for (world_name, world_id) in worlds {
            let module_name = module_name(&package_name, &world_name);
            let mut files = Files::default();
            let mut generator = opts.clone().build();
            generator.generate(&mut resolve, world_id, &mut files)?;

            let mut combined = Vec::new();
            for (_, contents) in files.iter() {
                combined.extend_from_slice(contents);
            }
            fs::write(bindings_dir.join(format!("{module_name}.rs")), combined)?;
            mod_rs.push_str(&format!(
                "pub mod {module_name} {{ include!(concat!(env!(\"GREENTIC_INTERFACES_BINDINGS\"), \"/{module_name}.rs\")); }}\n"
            ));

            if package_name.namespace == "greentic"
                && package_name.name == "interfaces-pack"
                && matches!(&package_name.version, Some(ver) if ver.major == 0 && ver.minor == 1)
                && world_name == "component"
            {
                default_module = Some(module_name.clone());
            }
        }
    }

    if let Some(default) = default_module {
        mod_rs.push_str(&format!("pub use {default}::*;\n"));
    }

    fs::write(bindings_dir.join("mod.rs"), mod_rs)?;

    Ok(bindings_dir)
}

fn module_name(name: &wit_bindgen_core::wit_parser::PackageName, world: &str) -> String {
    let formatted = format!("{name}-{world}");
    sanitize(&formatted).replace(['-', '.'], "_")
}

fn reset_directory(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn ensure_tenant_ctx_contains_i18n(bindings_dir: &Path) -> Result<(), Box<dyn Error>> {
    let mut missing = Vec::new();

    for entry in fs::read_dir(bindings_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if entry.file_name() == "mod.rs" {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let contents = fs::read_to_string(entry.path())?;
        let mut offset = 0;
        while let Some(pos) = contents[offset..].find("struct TenantCtx") {
            let start = offset + pos;
            let brace_offset = match contents[start..].find('{') {
                Some(brace) => start + brace,
                None => break,
            };

            let mut depth = 0;
            let mut end = None;
            for (i, ch) in contents[brace_offset..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(brace_offset + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if let Some(struct_end) = end {
                let body = &contents[brace_offset..=struct_end];
                if !body.contains("i18n_id") {
                    let module_name = entry
                        .path()
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or_default()
                        .to_string();
                    missing.push(format!(
                        "{} (module: {})",
                        entry.path().display(),
                        module_name
                    ));
                }
                offset = struct_end + 1;
                continue;
            }

            break;
        }
    }

    if !missing.is_empty() {
        panic!(
            "Bindings out of date: the following modules define TenantCtx without i18n_id:\n  {}\nBindings dir: {}. Regenerate WIT bindings so TenantCtx includes i18n_id for all variants.",
            missing.join("\n  "),
            bindings_dir.display()
        );
    }

    Ok(())
}

fn discover_packages(
    root: &Path,
    out: &mut BTreeMap<String, Vec<PathBuf>>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let package_file = path.join("package.wit");
            if package_file.exists() {
                let package_ref = read_package_ref(&package_file)?;
                out.entry(package_ref)
                    .or_default()
                    .push(package_file.clone());
            }
            discover_packages(&path, out)?;
        } else if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("wit") {
            let package_ref = read_package_ref(&path)?;
            out.entry(package_ref).or_default().push(path);
        }
    }
    Ok(())
}

fn canonical_interfaces_types_path(wit_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    for relative_path in CANONICAL_INTERFACES_TYPES_CANDIDATES {
        let candidate = wit_root.join(relative_path);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "none of the canonical WIT sources for {} exist under {} (tried: {})",
        CANONICAL_INTERFACES_TYPES_REF,
        wit_root.display(),
        CANONICAL_INTERFACES_TYPES_CANDIDATES.join(", ")
    )
    .into())
}

fn verify_interfaces_types_duplicates(
    wit_root: &Path,
    candidates: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<(), Box<dyn Error>> {
    let canonical_ref = CANONICAL_INTERFACES_TYPES_REF;
    let canonical_path = canonical_interfaces_types_path(wit_root)?;

    let mut entries: Vec<PathBuf> = candidates
        .get(canonical_ref)
        .ok_or_else(|| format!("package {canonical_ref} was not discovered"))?
        .clone();
    entries.sort();
    entries.dedup();

    if !entries.iter().any(|path| path == canonical_path.as_path()) {
        return Err(format!(
            "canonical WIT source {} for {} was not discovered; only found:\n  {}",
            canonical_path.display(),
            canonical_ref,
            entries
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        )
        .into());
    }

    let canonical_bytes = fs::read(&canonical_path)?;
    let mut mismatches = Vec::new();
    for path in entries.iter().filter(|path| **path != canonical_path) {
        let contents = fs::read(path)?;
        if contents != canonical_bytes {
            mismatches.push(path);
        }
    }

    let strict_canonical = canonical_path.file_name() == Some(OsStr::new("types.wit"));
    if strict_canonical && !mismatches.is_empty() {
        return Err(format!(
            "duplicates of {} diverge from canonical {}:\n  {}",
            canonical_ref,
            canonical_path.display(),
            mismatches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        )
        .into());
    }
    if !strict_canonical && !mismatches.is_empty() {
        println!(
            "cargo:warning=Found {} non-identical copies of {} while using packaged/local wit root (canonical: {}). Continuing without strict duplicate-byte enforcement.",
            mismatches.len(),
            canonical_ref,
            canonical_path.display()
        );
    }

    if entries.len() > 1 {
        let duplicates = entries
            .iter()
            .filter(|path| **path != canonical_path)
            .map(|path| format!("{}", path.display()))
            .collect::<Vec<_>>()
            .join("\n  ");
        println!(
            "cargo:warning=Found {} copies of {} (canonical: {}). Duplicates are byte-identical and will resolve to the canonical file. Copies:\n  {}",
            entries.len(),
            canonical_ref,
            canonical_path.display(),
            duplicates
        );
    }

    Ok(())
}

fn select_preferred_package_path(
    wit_root: &Path,
    package_ref: &str,
    candidates: &[PathBuf],
) -> Result<PathBuf, Box<dyn Error>> {
    if candidates.is_empty() {
        return Err(format!("no WIT sources found for {package_ref}").into());
    }

    if package_ref == CANONICAL_INTERFACES_TYPES_REF {
        let canonical_path = canonical_interfaces_types_path(wit_root)?;
        if !candidates.iter().any(|path| path == &canonical_path) {
            return Err(format!(
                "canonical WIT source {} for {package_ref} was not discovered; found {} entries",
                canonical_path.display(),
                candidates.len()
            )
            .into());
        }
        return Ok(canonical_path);
    }

    let mut sorted = candidates.to_vec();
    sorted.sort_by(|a, b| {
        let a_depth = a.components().count();
        let b_depth = b.components().count();
        a_depth.cmp(&b_depth).then_with(|| a.cmp(b))
    });

    sorted
        .first()
        .cloned()
        .ok_or_else(|| format!("unable to select a WIT source for {package_ref}").into())
}

struct PackageCatalog {
    selection: BTreeMap<String, PathBuf>,
}

impl PackageCatalog {
    fn new(
        wit_root: &Path,
        candidates: BTreeMap<String, Vec<PathBuf>>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut selection = BTreeMap::new();
        for (package_ref, paths) in candidates {
            let chosen = select_preferred_package_path(wit_root, &package_ref, &paths)?;
            selection.insert(package_ref, chosen);
        }
        Ok(Self { selection })
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &PathBuf)> + '_ {
        self.selection.iter()
    }

    fn resolve(&self, package_ref: &str) -> Result<&PathBuf, Box<dyn Error>> {
        self.selection
            .get(package_ref)
            .ok_or_else(|| format!("missing WIT source for {package_ref}").into())
    }
}
