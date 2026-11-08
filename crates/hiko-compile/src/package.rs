//! Package ownership and resolution prepared by #210.
//!
//! The language parser and manifest/lockfile writers do not use this catalog
//! until #186. Definitions arrive from the operator's metadata or linked
//! extensions; imported source never supplies its own identity or permissions.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "loader-integrity-blake3")]
use crate::compiler::module_cache_path;
#[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
use crate::compiler::verify_blake3;
use crate::compiler::{CompileError, checked_builtin_module, normalize_blake3};

/// Provenance, independent of a module's cache path or contents.
/// Possessing an identity does not grant compiler-owned builtin authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageOwner(Arc<OwnerIdentity>);

#[derive(Debug, PartialEq, Eq, Hash)]
enum OwnerIdentity {
    Project(PathBuf),
    Locked {
        name: String,
        version: String,
        base_url: String,
    },
    Extension {
        provider: String,
        package: String,
    },
    EmbeddedPackage(String),
    EmbeddedProject,
    Builtin,
}

impl PackageOwner {
    /// A local package rooted at an existing canonical directory.
    pub fn project(root: &Path) -> Result<Self, CompileError> {
        let root = std::fs::canonicalize(root).map_err(|e| {
            CompileError::codegen(format!(
                "cannot resolve package root '{}': {e}",
                root.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(CompileError::codegen(format!(
                "package root '{}' is not a directory",
                root.display()
            )));
        }
        Ok(Self::project_root(root))
    }

    pub(crate) fn project_root(root: PathBuf) -> Self {
        Self(Arc::new(OwnerIdentity::Project(root)))
    }

    /// Identity from a validated lock entry. The catalog validates definitions,
    /// including reserved names, before permitting any resolution.
    pub fn locked(name: &str, version: &str, base_url: &str) -> Self {
        Self(Arc::new(OwnerIdentity::Locked {
            name: name.into(),
            version: version.into(),
            base_url: base_url.into(),
        }))
    }

    pub fn extension(provider: &str, package: &str) -> Self {
        Self(Arc::new(OwnerIdentity::Extension {
            provider: provider.into(),
            package: package.into(),
        }))
    }

    // Current sealed artifacts carry package names but not lock identities.
    // The future catalog instead retains the full locked identity with embedded
    // sources. No format inference or filesystem fallback is introduced here.
    pub(crate) fn embedded(package: Option<&str>) -> Self {
        Self(Arc::new(match package {
            Some(name) => OwnerIdentity::EmbeddedPackage(name.into()),
            None => OwnerIdentity::EmbeddedProject,
        }))
    }

    pub(crate) fn builtin() -> Self {
        Self(Arc::new(OwnerIdentity::Builtin))
    }

    pub fn name(&self) -> &str {
        match self.0.as_ref() {
            OwnerIdentity::Project(_) | OwnerIdentity::EmbeddedProject => "Proj",
            OwnerIdentity::Locked { name, .. } | OwnerIdentity::EmbeddedPackage(name) => name,
            OwnerIdentity::Extension { package, .. } => package,
            OwnerIdentity::Builtin => "__Builtin",
        }
    }
}

/// Logical identity shared by an owner's qualified and Proj imports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleIdentity {
    owner: PackageOwner,
    module: String,
}

impl ModuleIdentity {
    pub fn owner(&self) -> &PackageOwner {
        &self.owner
    }
    pub fn module(&self) -> &str {
        &self.module
    }
}

/// Installed bytes are located explicitly; one origin never falls back to another.
#[derive(Debug)]
pub enum PinnedSource {
    /// Package root containing modules/.
    Local(PathBuf),
    /// Installed cache, using the existing URL/hash cache key.
    Cache(PathBuf),
    /// Module-relative name to source bytes, supplied by the sealed artifact.
    Embedded(BTreeMap<String, String>),
}

#[derive(Debug)]
pub enum ModuleSources {
    /// Mutable source under the project owner's src/.
    Project,
    Pinned {
        hashes: BTreeMap<String, String>,
        source: PinnedSource,
    },
    /// Linked host source; exact library imports retain ExtensionRequires semantics.
    Extension {
        modules: BTreeMap<String, String>,
        requires: BTreeSet<String>,
    },
}

/// Input from the reviewed catalog, never from a module being compiled.
/// Dependencies name installed packages/extensions, not version constraints.
#[derive(Debug)]
pub struct PackageDefinition {
    pub owner: PackageOwner,
    pub dependencies: BTreeSet<String>,
    pub sources: ModuleSources,
}

/// One validated namespace, with per-owner dependency authority.
#[derive(Debug)]
pub struct ModuleCatalog {
    owners: HashMap<PackageOwner, PackageDefinition>,
    names: BTreeMap<String, PackageOwner>,
}

impl ModuleCatalog {
    pub fn new(definitions: Vec<PackageDefinition>) -> Result<Self, CompileError> {
        let mut catalog = Self {
            owners: HashMap::new(),
            names: BTreeMap::new(),
        };
        for mut definition in definitions {
            let owner = &definition.owner;
            match (owner.0.as_ref(), &mut definition.sources) {
                (OwnerIdentity::Project(_), ModuleSources::Project) => {}
                (
                    OwnerIdentity::Project(_),
                    ModuleSources::Pinned {
                        source: PinnedSource::Embedded(_),
                        ..
                    },
                )
                | (OwnerIdentity::Locked { .. }, ModuleSources::Pinned { .. })
                | (OwnerIdentity::Extension { .. }, ModuleSources::Extension { .. }) => {}
                _ => {
                    return Err(CompileError::codegen(format!(
                        "invalid source kind for package {:?}",
                        owner
                    )));
                }
            }
            if !matches!(owner.0.as_ref(), OwnerIdentity::Project(_)) {
                validate_package(owner.name())?;
            }
            if let OwnerIdentity::Locked {
                version, base_url, ..
            } = owner.0.as_ref()
                && (version.is_empty() || base_url.is_empty())
            {
                return Err(CompileError::codegen(format!(
                    "package {} needs a locked version and base URL",
                    owner.name()
                )));
            }
            if let ModuleSources::Pinned { hashes, source } = &mut definition.sources {
                for (module, hash) in hashes.iter_mut() {
                    module_file(module)?;
                    *hash = normalized_hash(hash)?;
                }
                if let PinnedSource::Embedded(modules) = source
                    && !modules.keys().eq(hashes.keys())
                {
                    return Err(CompileError::codegen(format!(
                        "embedded package {} must contain exactly its pinned modules",
                        owner.name()
                    )));
                }
            }
            if let ModuleSources::Extension { modules, requires } = &definition.sources {
                for module in modules.keys() {
                    module_file(module)?;
                }
                for import in requires {
                    let (package, _) = import_path(import)?;
                    if package != "__Builtin"
                        && package != owner.name()
                        && !definition.dependencies.contains(package)
                    {
                        return Err(denied(owner, import, package));
                    }
                }
            }
            if let Some(previous) = catalog.names.insert(owner.name().into(), owner.clone()) {
                return Err(CompileError::codegen(format!(
                    "package name {} collides between {:?} and {:?}",
                    owner.name(),
                    previous,
                    owner
                )));
            }
            catalog.owners.insert(owner.clone(), definition);
        }
        for owner in catalog.names.values() {
            let definition = &catalog.owners[owner];
            for dependency in &definition.dependencies {
                validate_package(dependency)?;
                if !catalog.names.contains_key(dependency) {
                    return Err(CompileError::codegen(format!(
                        "package {} declares unavailable dependency {dependency}; declare and install it in hiko.toml or provide its extension",
                        definition.owner.name()
                    )));
                }
            }
        }
        Ok(catalog)
    }

    /// Resolve relative to the supplied source owner, never the consuming project.
    /// The caller keeps this identity with inferred and compiled source.
    pub fn resolve(
        &self,
        owner: Option<&PackageOwner>,
        import: &str,
    ) -> Result<ResolvedModule<'_>, CompileError> {
        let (package, module) = import_path(import)?;
        let owner = owner.ok_or_else(|| {
            CompileError::codegen(format!(
                "import {import} requires an owning package; create hiko.toml"
            ))
        })?;
        let importer = self.owners.get(owner).ok_or_else(|| {
            CompileError::codegen(format!(
                "importing package {:?} is not in this catalog",
                owner
            ))
        })?;
        if package == "__Builtin" {
            check_requires(importer, import)?;
            let builtin = checked_builtin_module(import, module)?;
            return Ok(ResolvedModule {
                identity: ModuleIdentity {
                    owner: PackageOwner::builtin(),
                    module: module.into(),
                },
                target: ModuleTarget::Text {
                    source: builtin.source,
                    hash: None,
                },
            });
        }
        let target = if package == "Proj" {
            owner
        } else {
            self.names
                .get(package)
                .ok_or_else(|| denied(owner, import, package))?
        };
        if target != owner && !importer.dependencies.contains(package) {
            return Err(denied(owner, import, package));
        }
        let definition = &self.owners[target];
        if target != owner && !matches!(definition.sources, ModuleSources::Extension { .. }) {
            check_requires(importer, import)?;
        }
        let identity = ModuleIdentity {
            owner: target.clone(),
            module: module.into(),
        };
        let missing = |expected: String| {
            CompileError::codegen(format!(
                "module {import} is missing from package {}: expected {expected}",
                target.name()
            ))
        };
        let target = match &definition.sources {
            ModuleSources::Project => {
                let OwnerIdentity::Project(root) = definition.owner.0.as_ref() else {
                    unreachable!("catalog source kind was validated");
                };
                let relative = Path::new("src").join(module_file(module)?);
                checked_module_file(root, &relative, import, target.name())?;
                ModuleTarget::Local {
                    root,
                    relative,
                    hash: None,
                }
            }
            ModuleSources::Pinned { hashes, source } => {
                let hash = hashes
                    .get(module)
                    .ok_or_else(|| missing(format!("locked module {module}")))?;
                match source {
                    PinnedSource::Local(root) => {
                        let relative = Path::new("modules").join(module_file(module)?);
                        checked_module_file(root, &relative, import, target.name())?;
                        ModuleTarget::Local {
                            root,
                            relative,
                            hash: Some(hash),
                        }
                    }
                    PinnedSource::Cache(directory) => {
                        #[cfg(feature = "loader-integrity-blake3")]
                        {
                            let OwnerIdentity::Locked { base_url, .. } = target.0.as_ref() else {
                                unreachable!("only locked packages use a cache");
                            };
                            let relative =
                                module_file(module)?.to_string_lossy().replace('\\', "/");
                            let url =
                                format!("{}/modules/{relative}", base_url.trim_end_matches('/'));
                            ModuleTarget::File {
                                path: module_cache_path(directory, &url, hash),
                                hash: Some(hash),
                            }
                        }
                        #[cfg(not(feature = "loader-integrity-blake3"))]
                        {
                            let _ = directory;
                            return Err(CompileError::codegen(
                                "cached packages require cargo feature 'loader-integrity-blake3'",
                            ));
                        }
                    }
                    PinnedSource::Embedded(modules) => ModuleTarget::Text {
                        source: modules
                            .get(module)
                            .ok_or_else(|| missing(format!("embedded module {module}")))?,
                        hash: Some(hash),
                    },
                }
            }
            ModuleSources::Extension { modules, .. } => ModuleTarget::Text {
                source: modules
                    .get(module)
                    .ok_or_else(|| missing(format!("extension module {module}")))?,
                hash: None,
            },
        };
        Ok(ResolvedModule { identity, target })
    }
}

#[derive(Debug)]
enum ModuleTarget<'a> {
    Local {
        root: &'a Path,
        relative: PathBuf,
        hash: Option<&'a str>,
    },
    File {
        path: PathBuf,
        hash: Option<&'a str>,
    },
    Text {
        source: &'a str,
        hash: Option<&'a str>,
    },
}

/// A resolution with immutable owner provenance. Reading never writes or fetches.
#[derive(Debug)]
pub struct ResolvedModule<'a> {
    identity: ModuleIdentity,
    target: ModuleTarget<'a>,
}

impl ResolvedModule<'_> {
    pub fn identity(&self) -> &ModuleIdentity {
        &self.identity
    }

    pub fn read_source(&self) -> Result<Cow<'_, str>, CompileError> {
        let (source, hash) = match &self.target {
            ModuleTarget::Local {
                root,
                relative,
                hash,
            } => {
                let import = format!("{}.{}", self.identity.owner.name(), self.identity.module);
                let path =
                    checked_module_file(root, relative, &import, self.identity.owner.name())?;
                let source = std::fs::read_to_string(&path).map_err(|e| {
                    CompileError::codegen(format!(
                        "cannot read module {import} at '{}': {e}",
                        path.display()
                    ))
                })?;
                (Cow::Owned(source), *hash)
            }
            ModuleTarget::File { path, hash } => {
                let source = std::fs::read_to_string(path).map_err(|e| {
                    CompileError::codegen(format!(
                        "cannot read module {}.{} at '{}': {e}; run 'hiko install' for locked sources",
                        self.identity.owner.name(), self.identity.module, path.display()
                    ))
                })?;
                (Cow::Owned(source), *hash)
            }
            ModuleTarget::Text { source, hash } => (Cow::Borrowed(*source), *hash),
        };
        if let Some(hash) = hash {
            #[cfg(any(feature = "loader-integrity-blake3", feature = "loader-embedded"))]
            verify_blake3(
                source.as_bytes(),
                hash,
                &format!(
                    "module {}.{}",
                    self.identity.owner.name(),
                    self.identity.module
                ),
            )?;
            #[cfg(not(any(feature = "loader-integrity-blake3", feature = "loader-embedded")))]
            {
                let _ = hash;
                return Err(CompileError::codegen(
                    "pinned sources require cargo feature 'loader-integrity-blake3' or 'loader-embedded'",
                ));
            }
        }
        Ok(source)
    }
}

fn denied(owner: &PackageOwner, import: &str, package: &str) -> CompileError {
    CompileError::codegen(format!(
        "package {} cannot import {import}: declare {package} as its dependency",
        owner.name()
    ))
}

fn check_requires(importer: &PackageDefinition, import: &str) -> Result<(), CompileError> {
    if let ModuleSources::Extension { requires, .. } = &importer.sources
        && !requires.contains(import)
    {
        return Err(CompileError::codegen(format!(
            "extension package {} imports {import}, which its requires does not list",
            importer.owner.name()
        )));
    }
    Ok(())
}

fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\'')
}

fn validate_package(name: &str) -> Result<(), CompileError> {
    if name == "Proj" || name.starts_with("__") || !identifier(name) {
        return Err(CompileError::codegen(format!(
            "package name '{name}' is invalid or reserved; choose an upper-case package name other than Proj or __Builtin"
        )));
    }
    Ok(())
}

fn import_path(import: &str) -> Result<(&str, &str), CompileError> {
    let Some((package, module)) = import.split_once('.') else {
        return Err(CompileError::codegen(format!(
            "named import '{import}' must use Package.Module[.Module...]"
        )));
    };
    if package != "__Builtin" && !identifier(package) {
        return Err(CompileError::codegen(format!(
            "invalid import package '{package}'"
        )));
    }
    module_file(module)?;
    Ok((package, module))
}

fn module_file(module: &str) -> Result<PathBuf, CompileError> {
    let mut path = PathBuf::new();
    for segment in module.split('.') {
        if !identifier(segment) {
            return Err(CompileError::codegen(format!(
                "invalid module path '{module}'; use upper-case identifiers separated by dots"
            )));
        }
        path.push(segment);
    }
    path.set_extension("hml");
    Ok(path)
}

fn normalized_hash(hash: &str) -> Result<String, CompileError> {
    let hash = normalize_blake3(hash);
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CompileError::codegen(
            "a pinned module needs a 64-digit BLAKE3 hash",
        ));
    }
    Ok(hash)
}

/// Check each written component rather than accepting case-folding or symlinks.
fn checked_module_file(
    root: &Path,
    relative: &Path,
    import: &str,
    owner: &str,
) -> Result<PathBuf, CompileError> {
    let expected = root.join(relative);
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let wanted = component.as_os_str();
        let entries = std::fs::read_dir(&path).map_err(|e| {
            CompileError::codegen(format!(
                "cannot resolve module {import} in package {owner}: expected {}: {e}",
                expected.display()
            ))
        })?;
        let mut found = None;
        for entry in entries {
            let entry = entry.map_err(|e| {
                CompileError::codegen(format!(
                    "cannot inspect package directory '{}': {e}",
                    path.display()
                ))
            })?;
            if entry.file_name() == wanted {
                found = Some(entry);
                break;
            }
        }
        let entry = found.ok_or_else(|| CompileError::codegen(format!(
            "module {import} is missing from package {owner}: expected {} (exact spelling required)", expected.display()
        )))?;
        let kind = entry.file_type().map_err(|e| {
            CompileError::codegen(format!(
                "cannot inspect module path '{}': {e}",
                entry.path().display()
            ))
        })?;
        if kind.is_symlink() {
            return Err(CompileError::codegen(format!(
                "module {import} in package {owner} resolves through a symlink at '{}'; use a plain path under the package source root",
                entry.path().display()
            )));
        }
        path = entry.path();
    }
    if !path.is_file() {
        return Err(CompileError::codegen(format!(
            "module {import} is missing from package {owner}: expected {}",
            expected.display()
        )));
    }
    Ok(path)
}
