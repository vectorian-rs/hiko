#![cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use hiko_common::blake3_hex;
use hiko_compile::package::{
    ModuleCatalog, ModuleSources, PackageDefinition, PackageOwner, PinnedSource,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hiko-package-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn deps(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).into()).collect()
}
fn locked(name: &str) -> PackageOwner {
    PackageOwner::locked(
        name,
        "1.0.0",
        &format!("https://example.test/{name}-v1.0.0"),
    )
}
fn project(root: &Scratch, dependencies: &[&str]) -> PackageDefinition {
    root.write("hiko.toml", "[project]\nname = 'display-name'\n");
    PackageDefinition {
        owner: PackageOwner::project(&root.0).unwrap(),
        dependencies: deps(dependencies),
        sources: ModuleSources::Project,
    }
}
fn embedded(
    owner: PackageOwner,
    dependencies: &[&str],
    modules: &[(&str, &str)],
) -> PackageDefinition {
    PackageDefinition {
        owner,
        dependencies: deps(dependencies),
        sources: ModuleSources::Pinned {
            hashes: modules
                .iter()
                .map(|(name, source)| ((*name).into(), blake3_hex(source.as_bytes())))
                .collect(),
            source: PinnedSource::Embedded(
                modules
                    .iter()
                    .map(|(name, source)| ((*name).into(), (*source).into()))
                    .collect(),
            ),
        },
    }
}
fn error<T: std::fmt::Debug>(result: Result<T, hiko_compile::compiler::CompileError>) -> String {
    format!("{:?}", result.unwrap_err())
}

#[test]
fn project_and_two_embedded_libraries_keep_equal_and_nested_names_in_their_owners() {
    let root = Scratch::new();
    root.write("src/Util.hml", "val who = \"project\"");
    root.write("src/Net/Client.hml", "val who = \"project-client\"");
    let definition = project(&root, &["Alpha", "Beta"]);
    let project_owner = definition.owner.clone();
    let alpha = locked("Alpha");
    let beta = locked("Beta");
    let catalog = ModuleCatalog::new(vec![
        definition,
        embedded(
            alpha.clone(),
            &[],
            &[
                ("Util", "val who = \"alpha\""),
                ("Net.Client", "val who = \"alpha-client\""),
            ],
        ),
        embedded(
            beta.clone(),
            &[],
            &[
                ("Util", "val who = \"beta\""),
                ("Net.Client", "val who = \"beta-client\""),
            ],
        ),
    ])
    .unwrap();
    for (owner, expected) in [
        (&project_owner, "project"),
        (&alpha, "alpha"),
        (&beta, "beta"),
    ] {
        for (module, suffix) in [("Util", ""), ("Net.Client", "-client")] {
            let resolved = catalog
                .resolve(Some(owner), &format!("Proj.{module}"))
                .unwrap();
            assert_eq!(resolved.identity().owner(), owner);
            assert_eq!(
                resolved.read_source().unwrap(),
                format!("val who = \"{expected}{suffix}\"")
            );
            if owner != &project_owner {
                let qualified = catalog
                    .resolve(Some(owner), &format!("{}.{module}", owner.name()))
                    .unwrap();
                assert_eq!(qualified.identity(), resolved.identity());
            }
        }
    }
    assert!(error(catalog.resolve(None, "Proj.Util")).contains("requires an owning package"));
    let counterfeit = PackageOwner::locked("Alpha", "1.0.0", "https://other.test/Alpha-v1.0.0");
    assert!(
        error(catalog.resolve(Some(&counterfeit), "Proj.Util")).contains("not in this catalog")
    );
}

#[test]
fn a_library_does_not_inherit_its_consumers_dependencies() {
    let root = Scratch::new();
    let definition = project(&root, &["Alpha", "Beta"]);
    let alpha = locked("Alpha");
    let beta = locked("Beta");
    let catalog = ModuleCatalog::new(vec![
        definition,
        embedded(alpha.clone(), &[], &[("Util", "val who = \"alpha\"")]),
        embedded(beta, &[], &[("Util", "val who = \"beta\"")]),
    ])
    .unwrap();
    assert!(
        error(catalog.resolve(Some(&alpha), "Beta.Util"))
            .contains("package Alpha cannot import Beta.Util: declare Beta as its dependency")
    );
    let permitted = ModuleCatalog::new(vec![
        embedded(alpha.clone(), &["Beta"], &[("Util", "val who = \"alpha\"")]),
        embedded(locked("Beta"), &[], &[("Util", "val who = \"beta\"")]),
    ])
    .unwrap();
    assert_eq!(
        permitted
            .resolve(Some(&alpha), "Beta.Util")
            .unwrap()
            .read_source()
            .unwrap(),
        "val who = \"beta\""
    );
}

#[test]
fn local_pinned_sources_use_nested_layout_and_reject_tampering() {
    let root = Scratch::new();
    root.write("modules/Net/Client.hml", "val who = \"alpha\"");
    let owner = locked("Alpha");
    let definition = PackageDefinition {
        owner: owner.clone(),
        dependencies: deps(&[]),
        sources: ModuleSources::Pinned {
            hashes: [("Net.Client".into(), blake3_hex(b"val who = \"alpha\""))].into(),
            source: PinnedSource::Local(root.0.clone()),
        },
    };
    let catalog = ModuleCatalog::new(vec![definition]).unwrap();
    let resolved = catalog.resolve(Some(&owner), "Proj.Net.Client").unwrap();
    assert_eq!(resolved.read_source().unwrap(), "val who = \"alpha\"");
    root.write("modules/Net/Client.hml", "val who = \"changed\"");
    assert!(error(resolved.read_source()).contains("failed BLAKE3 verification"));
    assert_eq!(
        std::fs::read_to_string(root.0.join("modules/Net/Client.hml")).unwrap(),
        "val who = \"changed\""
    );
}

#[cfg(feature = "loader-integrity-blake3")]
#[test]
fn cache_resolution_is_owner_relative_verified_and_read_only() {
    use hiko_compile::compiler::module_cache_path;
    let root = Scratch::new();
    let directory = root.0.join("cache");
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/Alpha-v1.0.0", listener.local_addr().unwrap());
    let owner = PackageOwner::locked("Alpha", "1.0.0", &base);
    let source = "val who = \"alpha-cache\"";
    let hash = blake3_hex(source.as_bytes());
    let path = module_cache_path(&directory, &format!("{base}/modules/Net/Client.hml"), &hash);
    let catalog = ModuleCatalog::new(vec![PackageDefinition {
        owner: owner.clone(),
        dependencies: deps(&[]),
        sources: ModuleSources::Pinned {
            hashes: [("Net.Client".into(), hash)].into(),
            source: PinnedSource::Cache(directory.clone()),
        },
    }])
    .unwrap();
    let missing = catalog.resolve(Some(&owner), "Proj.Net.Client").unwrap();
    assert!(error(missing.read_source()).contains("hiko install"));
    assert!(
        !directory.exists(),
        "missing cache must not be created by resolution"
    );
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(&path, source).unwrap();
    let named = catalog.resolve(Some(&owner), "Alpha.Net.Client").unwrap();
    assert_eq!(named.identity(), missing.identity());
    assert_eq!(named.read_source().unwrap(), source);
    std::fs::write(&path, "corrupt").unwrap();
    assert!(error(named.read_source()).contains("failed BLAKE3 verification"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt");
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn a_sealed_project_keeps_its_identity_without_reading_the_project() {
    let root = Scratch::new();
    let project = project(&root, &[]);
    let owner = project.owner;
    root.write("src/Util.hml", "val who = \"disk\"");
    let catalog = ModuleCatalog::new(vec![embedded(
        owner.clone(),
        &[],
        &[("Util", "val who = \"sealed\"")],
    )])
    .unwrap();
    std::fs::remove_dir_all(&root.0).unwrap();
    let resolved = catalog.resolve(Some(&owner), "Proj.Util").unwrap();
    assert_eq!(resolved.identity().owner(), &owner);
    assert_eq!(resolved.read_source().unwrap(), "val who = \"sealed\"");
}

#[test]
fn embedded_sources_need_complete_pins_and_cannot_fall_back() {
    let root = Scratch::new();
    root.write("modules/Util.hml", "val who = \"disk\"");
    let owner = locked("Alpha");
    let missing = PackageDefinition {
        owner: owner.clone(),
        dependencies: deps(&[]),
        sources: ModuleSources::Pinned {
            hashes: [("Util".into(), blake3_hex(b"val who = \"expected\""))].into(),
            source: PinnedSource::Embedded(BTreeMap::new()),
        },
    };
    assert!(error(ModuleCatalog::new(vec![missing])).contains("exactly its pinned modules"));
    let wrong = PackageDefinition {
        owner: owner.clone(),
        dependencies: deps(&[]),
        sources: ModuleSources::Pinned {
            hashes: [("Util".into(), blake3_hex(b"val who = \"expected\""))].into(),
            source: PinnedSource::Embedded(
                [("Util".into(), "val who = \"changed\"".into())].into(),
            ),
        },
    };
    let catalog = ModuleCatalog::new(vec![wrong]).unwrap();
    assert!(
        error(
            catalog
                .resolve(Some(&owner), "Proj.Util")
                .unwrap()
                .read_source()
        )
        .contains("failed BLAKE3 verification")
    );
    assert!(error(catalog.resolve(Some(&owner), "Proj.Missing")).contains("locked module Missing"));
}

#[test]
fn namespace_validation_rejects_reserved_names_and_package_extension_collisions() {
    for name in ["Proj", "__Builtin", "lower", "Bad/Name"] {
        assert!(
            error(ModuleCatalog::new(vec![embedded(
                locked(name),
                &[],
                &[("Util", "val x = 1")]
            )]))
            .contains("invalid or reserved")
        );
    }
    let ordinary = embedded(locked("Alpha"), &[], &[("Util", "val x = 1")]);
    let extension = PackageDefinition {
        owner: PackageOwner::extension("linked-alpha", "Alpha"),
        dependencies: deps(&[]),
        sources: ModuleSources::Extension {
            modules: [("Util".into(), "val x = 2".into())].into(),
            requires: deps(&[]),
        },
    };
    let collision = error(ModuleCatalog::new(vec![ordinary, extension]));
    assert!(
        collision.contains("collides")
            && collision.contains("linked-alpha")
            && collision.contains("example.test"),
        "{collision}"
    );
    assert!(
        error(ModuleCatalog::new(vec![embedded(
            locked("Alpha"),
            &["Absent"],
            &[("Util", "val x = 1")]
        )]))
        .contains("unavailable dependency Absent")
    );
    for module in [
        "",
        "net.Client",
        "Net..Client",
        "../Util",
        "/Util",
        "Net\\Client",
    ] {
        assert!(
            error(ModuleCatalog::new(vec![embedded(
                locked("Alpha"),
                &[],
                &[(module, "val x = 1")]
            )]))
            .contains("invalid module path")
        );
    }
}

#[test]
fn extensions_keep_exact_library_requirements_including_compiler_modules() {
    let owner = PackageOwner::extension("aws", "Aws");
    let catalog = ModuleCatalog::new(vec![
        PackageDefinition {
            owner: owner.clone(),
            dependencies: deps(&["Std"]),
            sources: ModuleSources::Extension {
                modules: [("S3".into(), "val who = \"aws\"".into())].into(),
                requires: deps(&["Std.Result"]),
            },
        },
        embedded(
            locked("Std"),
            &[],
            &[("Result", "val result = 1"), ("List", "val list = 2")],
        ),
    ])
    .unwrap();
    assert_eq!(
        catalog
            .resolve(Some(&owner), "Proj.S3")
            .unwrap()
            .read_source()
            .unwrap(),
        "val who = \"aws\""
    );
    assert!(catalog.resolve(Some(&owner), "Std.Result").is_ok());
    assert!(error(catalog.resolve(Some(&owner), "Std.List")).contains("requires does not list"));
    assert!(
        error(catalog.resolve(Some(&owner), "__Builtin.Process"))
            .contains("requires does not list")
    );
}

#[test]
fn local_modules_need_exact_case_and_missing_errors_name_the_expected_path() {
    let root = Scratch::new();
    root.write("src/Net/Client.hml", "val who = \"project\"");
    let project = project(&root, &[]);
    let owner = project.owner.clone();
    let catalog = ModuleCatalog::new(vec![project]).unwrap();
    assert!(catalog.resolve(Some(&owner), "Proj.Net.Client").is_ok());
    let case = error(catalog.resolve(Some(&owner), "Proj.Net.CLIENT"));
    assert!(
        case.contains("exact spelling") && case.contains("CLIENT.hml"),
        "{case}"
    );
    let missing = error(catalog.resolve(Some(&owner), "Proj.Missing"));
    assert!(
        missing.contains("module Proj.Missing is missing from package Proj")
            && missing.contains("src/Missing.hml"),
        "{missing}"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_inside_and_outside_a_source_root_are_refused() {
    use std::os::unix::fs::symlink;
    let root = Scratch::new();
    let outside = Scratch::new();
    root.write("src/Real.hml", "val who = \"inside\"");
    root.write("src/Swapped.hml", "val who = \"before\"");
    outside.write("Else.hml", "val who = \"outside\"");
    symlink(root.0.join("src/Real.hml"), root.0.join("src/Alias.hml")).unwrap();
    symlink(outside.0.join("Else.hml"), root.0.join("src/Escape.hml")).unwrap();
    symlink(outside.0.clone(), root.0.join("src/Net")).unwrap();
    let definition = project(&root, &[]);
    let owner = definition.owner.clone();
    let catalog = ModuleCatalog::new(vec![definition]).unwrap();
    let resolved_before_swap = catalog.resolve(Some(&owner), "Proj.Swapped").unwrap();
    std::fs::remove_file(root.0.join("src/Swapped.hml")).unwrap();
    symlink(outside.0.join("Else.hml"), root.0.join("src/Swapped.hml")).unwrap();
    assert!(error(resolved_before_swap.read_source()).contains("symlink"));
    for import in ["Proj.Alias", "Proj.Escape", "Proj.Net.Else"] {
        assert!(
            error(catalog.resolve(Some(&owner), import)).contains("symlink"),
            "{import}"
        );
    }
}

// A shared path is not an authority: foreign catalog owners are rejected above.
// This also keeps opaque entry ownership stable through canonical project links.
#[cfg(unix)]
#[test]
fn project_owner_uses_canonical_identity() {
    let root = Scratch::new();
    let links = Scratch::new();
    std::os::unix::fs::symlink(&root.0, links.0.join("Project")).unwrap();
    assert_eq!(
        PackageOwner::project(&root.0).unwrap(),
        PackageOwner::project(&links.0.join("Project")).unwrap()
    );
}
