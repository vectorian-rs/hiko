use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use hiko_common::blake3_hex;
use serde::Deserialize;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor};
use tree_sitter_hiko::{HIGHLIGHTS_QUERY, LANGUAGE};

type DynError = Box<dyn Error>;

const DEFAULT_LIBRARIES_DIR: &str = "libraries";
const DOCGEN_CSS_PATH: &str = "_assets/docgen.css";
const NARRATIVE_DOC_PATCH_MARKER: &str = "hiko-docgen narrative patch";
const DOCGEN_CSS: &str = r#":root {
  color-scheme: light;
  --bg: #f8f4ec;
  --panel: #fffdf9;
  --text: #201b15;
  --muted: #6c6356;
  --line: #ddd2bf;
  --accent: #7a331b;
  --accent-soft: #f6e4d7;
  --code-bg: #fff9f1;
  --code-line: #eadbc8;
  --tok-keyword: #8f2d15;
  --tok-comment: #8f806f;
  --tok-string: #26665a;
  --tok-string-special: #4a5ec9;
  --tok-number: #8e5b00;
  --tok-number-float: #8e5b00;
  --tok-constant-builtin-boolean: #4b3fb5;
  --tok-type-definition: #7d3a96;
  --tok-type: #1e5a99;
  --tok-constructor: #8b4d13;
  --tok-function-special: #9f2246;
  --tok-function: #9f2246;
  --tok-module: #1f5f9e;
  --tok-namespace: #6a2f88;
  --tok-variable: #201b15;
  --tok-variable-member: #8f2d15;
}

body {
  margin: 0;
  font-family: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Georgia, serif;
  background: linear-gradient(180deg, #fffaf2 0%, var(--bg) 100%);
  color: var(--text);
}

main {
  max-width: 980px;
  margin: 0 auto;
  padding: 48px 20px 64px;
}

h1, h2 {
  font-weight: 600;
  margin-bottom: 0.45rem;
}

p, li, td {
  line-height: 1.55;
}

.eyebrow {
  color: var(--accent);
  font-size: 0.92rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  margin-bottom: 8px;
}

.panel {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 16px;
  padding: 20px 22px;
  box-shadow: 0 10px 30px rgba(41, 28, 16, 0.06);
  margin-top: 20px;
}

.meta {
  color: var(--muted);
  font-size: 0.95rem;
}

table {
  width: 100%;
  border-collapse: collapse;
}

th, td {
  padding: 12px 10px;
  border-top: 1px solid var(--line);
  text-align: left;
  vertical-align: top;
}

thead th {
  border-top: 0;
  color: var(--muted);
  font-size: 0.93rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

pre, code {
  font-family: "SFMono-Regular", "Menlo", "Consolas", monospace;
}

code {
  background: var(--accent-soft);
  border-radius: 6px;
  padding: 0.12rem 0.35rem;
  font-size: 0.95em;
  word-break: break-all;
}

pre.code-block {
  margin: 0;
  background: var(--code-bg);
  border: 1px solid var(--code-line);
  border-radius: 12px;
  padding: 14px 16px;
  overflow-x: auto;
  line-height: 1.45;
}

pre.code-block code {
  background: transparent;
  border-radius: 0;
  padding: 0;
  word-break: normal;
}

.siglist {
  list-style: none;
  margin: 0;
  padding: 0;
}

.siglist li {
  border-top: 1px solid var(--line);
  padding: 12px 0;
}

.siglist li:first-child {
  border-top: 0;
  padding-top: 0;
}

a {
  color: var(--accent);
  text-decoration: none;
}

a:hover {
  text-decoration: underline;
}

.tok-keyword { color: var(--tok-keyword); font-weight: 700; }
.tok-comment { color: var(--tok-comment); font-style: italic; }
.tok-string { color: var(--tok-string); }
.tok-string-special { color: var(--tok-string-special); }
.tok-number { color: var(--tok-number); }
.tok-number-float { color: var(--tok-number-float); }
.tok-constant-builtin-boolean { color: var(--tok-constant-builtin-boolean); font-weight: 700; }
.tok-type-definition { color: var(--tok-type-definition); }
.tok-type { color: var(--tok-type); }
.tok-constructor { color: var(--tok-constructor); }
.tok-function-special { color: var(--tok-function-special); font-weight: 700; }
.tok-function { color: var(--tok-function-special); }
.tok-module { color: var(--tok-module); font-weight: 700; }
.tok-namespace { color: var(--tok-namespace); }
.tok-variable { color: var(--tok-variable); }
.tok-variable-member { color: var(--tok-variable-member); }
"#;
const NARRATIVE_DOC_CSS_PATCH: &str = r#"
    /* hiko-docgen narrative patch */
    pre.code-block code { background: transparent; border-radius: 0; padding: 0; word-break: normal; }
    .tok-keyword { color: #8f2d15; font-weight: 700; }
    .tok-comment { color: #8f806f; font-style: italic; }
    .tok-string { color: #26665a; }
    .tok-string-special { color: #4a5ec9; }
    .tok-number { color: #8e5b00; }
    .tok-number-float { color: #8e5b00; }
    .tok-constant-builtin-boolean { color: #4b3fb5; font-weight: 700; }
    .tok-type-definition { color: #7d3a96; }
    .tok-type { color: #1e5a99; }
    .tok-constructor { color: #8b4d13; }
    .tok-function-special { color: #9f2246; font-weight: 700; }
    .tok-function { color: #9f2246; }
    .tok-module { color: #1f5f9e; font-weight: 700; }
    .tok-namespace { color: #6a2f88; }
    .tok-variable { color: #201b15; }
    .tok-variable-member { color: #8f2d15; }
"#;

#[derive(Debug, Deserialize)]
struct PackageManifest {
    name: String,
    version: String,
    modules: BTreeMap<String, String>,
}

#[derive(Debug)]
struct PackageInfo {
    manifest: PackageManifest,
    dir_name: String,
    dir: PathBuf,
    modules_dir: PathBuf,
    modules: Vec<ModuleInfo>,
}

#[derive(Debug)]
struct ModuleInfo {
    name: String,
    hash: String,
    docs_exists: bool,
    docs_path: PathBuf,
    source_page_path: PathBuf,
    exports: Vec<ExportItem>,
    highlighted_html: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportItem {
    kind: &'static str,
    name: String,
    detail: Option<String>,
}

#[derive(Debug)]
struct HighlightSpan {
    start: usize,
    end: usize,
    order: usize,
    class_name: String,
}

fn main() -> Result<(), DynError> {
    let libraries_root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LIBRARIES_DIR));

    generate_docs(&libraries_root)
}

fn generate_docs(libraries_root: &Path) -> Result<(), DynError> {
    let mut packages = collect_packages(libraries_root)?;
    packages.sort_by(|a, b| {
        a.manifest
            .name
            .cmp(&b.manifest.name)
            .then_with(|| a.manifest.version.cmp(&b.manifest.version))
    });

    write_shared_assets(libraries_root)?;
    write_root_index(libraries_root, &packages)?;

    for package in &packages {
        write_package_index(libraries_root, package)?;
        write_modules_index(libraries_root, package)?;

        for module in &package.modules {
            rewrite_narrative_doc_page(module)?;
            write_module_source_page(libraries_root, package, module)?;
        }
    }

    Ok(())
}

fn invalid_input(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

fn collect_packages(libraries_root: &Path) -> Result<Vec<PackageInfo>, DynError> {
    let mut packages = Vec::new();

    for entry in fs::read_dir(libraries_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let package_toml = path.join("package.toml");
        if !package_toml.exists() {
            continue;
        }

        packages.push(load_package(&path)?);
    }

    Ok(packages)
}

fn load_package(package_dir: &Path) -> Result<PackageInfo, DynError> {
    let manifest_text = fs::read_to_string(package_dir.join("package.toml"))?;
    let manifest: PackageManifest = toml::from_str(&manifest_text)?;
    let modules_dir = package_dir.join("modules");
    let dir_name = package_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_input("package directory must have a valid UTF-8 name"))?
        .to_owned();

    let manifest_names: BTreeSet<_> = manifest.modules.keys().cloned().collect();
    let mut actual_names = BTreeSet::new();

    for entry in fs::read_dir(&modules_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("hml") {
            continue;
        }

        let module_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| invalid_input("module filename must be valid UTF-8"))?
            .to_owned();
        actual_names.insert(module_name);
    }

    let missing: Vec<_> = manifest_names.difference(&actual_names).cloned().collect();
    if !missing.is_empty() {
        return Err(invalid_input(format!(
            "package {} is missing module files for: {}",
            manifest.name,
            missing.join(", ")
        ))
        .into());
    }

    let extra: Vec<_> = actual_names.difference(&manifest_names).cloned().collect();
    if !extra.is_empty() {
        return Err(invalid_input(format!(
            "package {} has unlisted module files: {}",
            manifest.name,
            extra.join(", ")
        ))
        .into());
    }

    let mut modules = Vec::new();
    for (module_name, expected_hash) in &manifest.modules {
        let source_path = modules_dir.join(format!("{module_name}.hml"));
        let raw_source = fs::read_to_string(&source_path)?;
        let actual_hash = format!("blake3:{}", blake3_hex(raw_source.as_bytes()));
        if &actual_hash != expected_hash {
            return Err(invalid_input(format!(
                "hash mismatch for {}.{}: expected {}, got {}",
                manifest.name, module_name, expected_hash, actual_hash
            ))
            .into());
        }

        let highlighted_html = highlight_source(&raw_source)?;
        let exports = extract_exports(&raw_source)?;
        let docs_path = modules_dir.join(format!("{module_name}.html"));
        let source_page_path = modules_dir.join(format!("{module_name}.source.html"));

        modules.push(ModuleInfo {
            name: module_name.clone(),
            hash: actual_hash,
            docs_exists: docs_path.exists(),
            docs_path,
            source_page_path,
            exports,
            highlighted_html,
        });
    }

    Ok(PackageInfo {
        manifest,
        dir_name,
        dir: package_dir.to_path_buf(),
        modules_dir,
        modules,
    })
}

fn highlight_source(source: &str) -> Result<String, DynError> {
    let language = tree_sitter::Language::from(LANGUAGE);
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| invalid_input("tree-sitter-hiko returned no parse tree"))?;

    if tree.root_node().has_error() {
        return Err(invalid_input(
            "tree-sitter-hiko reported syntax errors while parsing module source",
        )
        .into());
    }

    let query = Query::new(&language, HIGHLIGHTS_QUERY)?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut spans = Vec::new();
    let mut order = 0usize;

    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(query_match) = matches.next() {
        for capture in query_match.captures {
            let range = capture.node.byte_range();
            if range.is_empty() {
                continue;
            }

            spans.push(HighlightSpan {
                start: range.start,
                end: range.end,
                order,
                class_name: capture_names[capture.index as usize].replace('.', "-"),
            });
            order += 1;
        }
    }

    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.end.cmp(&left.end))
            .then_with(|| right.order.cmp(&left.order))
    });

    let mut html = String::with_capacity(source.len() + spans.len() * 24);
    let mut cursor = 0usize;

    for span in spans {
        if span.start < cursor {
            continue;
        }

        html.push_str(&escape_html(&source[cursor..span.start]));
        write!(
            html,
            "<span class=\"tok-{}\">{}</span>",
            span.class_name,
            escape_html(&source[span.start..span.end])
        )?;
        cursor = span.end;
    }

    html.push_str(&escape_html(&source[cursor..]));
    Ok(html)
}

fn extract_exports(source: &str) -> Result<Vec<ExportItem>, DynError> {
    let language = tree_sitter::Language::from(LANGUAGE);
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| invalid_input("tree-sitter-hiko returned no parse tree"))?;

    if tree.root_node().has_error() {
        return Err(invalid_input(
            "tree-sitter-hiko reported syntax errors while extracting exports",
        )
        .into());
    }

    let root = tree.root_node();
    let structure = root
        .named_children(&mut root.walk())
        .find(|node| node.kind() == "structure_declaration");

    let Some(structure) = structure else {
        return Ok(Vec::new());
    };

    let mut exports = Vec::new();
    for child in structure.named_children(&mut structure.walk()) {
        match child.kind() {
            "function_declaration" => {
                for binding in child.named_children(&mut child.walk()) {
                    if binding.kind() != "function_binding" {
                        continue;
                    }

                    if let Some(name) = field_text(binding, "name", source) {
                        exports.push(ExportItem {
                            kind: "fun",
                            name,
                            detail: None,
                        });
                    }
                }
            }
            "value_rec_declaration" => {
                if let Some(name) = field_text(child, "name", source) {
                    exports.push(ExportItem {
                        kind: "val rec",
                        name,
                        detail: None,
                    });
                }
            }
            "value_declaration" => {
                if let Some(pattern) = child.child_by_field_name("pattern")
                    && let Some(name) = export_name_from_pattern(pattern, source)
                {
                    exports.push(ExportItem {
                        kind: "val",
                        name,
                        detail: None,
                    });
                }
            }
            "datatype_declaration" => {
                let detail = child
                    .named_children(&mut child.walk())
                    .filter(|node| node.kind() == "constructor_declaration")
                    .filter_map(|node| field_text(node, "name", source))
                    .collect::<Vec<_>>()
                    .join(" | ");

                if let Some(name) = field_text(child, "name", source) {
                    exports.push(ExportItem {
                        kind: "datatype",
                        name,
                        detail: if detail.is_empty() {
                            None
                        } else {
                            Some(detail)
                        },
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = field_text(child, "name", source) {
                    exports.push(ExportItem {
                        kind: "type",
                        name,
                        detail: None,
                    });
                }
            }
            "effect_declaration" => {
                if let Some(name) = field_text(child, "name", source) {
                    exports.push(ExportItem {
                        kind: "effect",
                        name,
                        detail: None,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(exports)
}

fn export_name_from_pattern(pattern: Node<'_>, source: &str) -> Option<String> {
    if pattern.kind() == "identifier" {
        return Some(node_text(pattern, source));
    }

    for child in pattern.named_children(&mut pattern.walk()) {
        if let Some(name) = export_name_from_pattern(child, source) {
            return Some(name);
        }
    }

    None
}

fn write_shared_assets(libraries_root: &Path) -> Result<(), DynError> {
    let assets_dir = libraries_root.join("_assets");
    fs::create_dir_all(&assets_dir)?;
    fs::write(assets_dir.join("docgen.css"), DOCGEN_CSS)?;
    Ok(())
}

fn write_root_index(libraries_root: &Path, packages: &[PackageInfo]) -> Result<(), DynError> {
    let mut rows = String::new();
    for package in packages {
        let manifest_link = format!("./{}/package.toml", package.dir_name);
        let package_link = format!("./{}/index.html", package.dir_name);
        let modules_link = format!("./{}/modules/index.html", package.dir_name);
        write!(
            rows,
            "<tr><td><code>{}</code></td><td>{}</td><td><a href=\"{}\">{}</a></td><td><a href=\"{}\">package.toml</a></td><td><a href=\"{}\">modules/</a></td></tr>",
            escape_html(&package.manifest.name),
            escape_html(&package.manifest.version),
            package_link,
            escape_html(&package.dir_name),
            manifest_link,
            modules_link
        )?;
    }

    let html = page_shell(
        "Hiko Library Store",
        "./_assets/docgen.css",
        "<div class=\"eyebrow\">Hiko libraries</div><h1>Published packages</h1><p>Static package roots served over HTTP. The loader fetches raw <code>.hml</code> module files; this HTML layer is for humans browsing the store.</p>",
        &format!(
            "<section class=\"panel\"><h2>Packages</h2><table><thead><tr><th>Package</th><th>Version</th><th>Directory</th><th>Manifest</th><th>Modules</th></tr></thead><tbody>{}</tbody></table></section>",
            rows
        ),
    );

    fs::write(libraries_root.join("index.html"), html)?;
    Ok(())
}

fn write_package_index(libraries_root: &Path, package: &PackageInfo) -> Result<(), DynError> {
    let stylesheet = rel_link(&package.dir, &libraries_root.join(DOCGEN_CSS_PATH))?;
    let package_page_title = format!("{} {}", package.manifest.name, package.manifest.version);
    let mut rows = String::new();

    for module in &package.modules {
        let import_name = format!("{}.{}", package.manifest.name, module.name);
        let docs_cell = if module.docs_exists {
            format!(
                "<a href=\"./modules/{}.html\">{}.html</a>",
                module.name, module.name
            )
        } else {
            "—".to_string()
        };

        write!(
            rows,
            "<tr><td><code>{}</code></td><td>{}</td><td><a href=\"./modules/{}.source.html\">source.html</a></td><td><a href=\"./modules/{}.hml\">{}.hml</a></td><td><code title=\"{}\">{}</code></td></tr>",
            escape_html(&import_name),
            docs_cell,
            module.name,
            module.name,
            module.name,
            escape_html(&module.hash),
            escape_html(&trim_hash(&module.hash))
        )?;
    }

    let header = format!(
        "<div class=\"eyebrow\"><a href=\"../index.html\">Library store</a></div><h1>{}</h1><p>Published package root for <code>{}</code>. The manifest is the authoritative list of importable modules and their BLAKE3 hashes.</p>",
        escape_html(&package_page_title),
        escape_html(&package.dir_name)
    );
    let body = format!(
        "<section class=\"panel\"><h2>Package manifest</h2><p class=\"meta\">Raw manifest: <a href=\"./package.toml\">package.toml</a></p><p><code>name = \"{}\"</code><br><code>version = \"{}\"</code></p></section>\
         <section class=\"panel\"><h2>Modules</h2><table><thead><tr><th>Import</th><th>Docs</th><th>Highlighted source</th><th>Raw file</th><th>BLAKE3</th></tr></thead><tbody>{}</tbody></table></section>",
        escape_html(&package.manifest.name),
        escape_html(&package.manifest.version),
        rows
    );

    fs::write(
        package.dir.join("index.html"),
        page_shell(&package_page_title, &stylesheet, &header, &body),
    )?;
    Ok(())
}

fn write_modules_index(libraries_root: &Path, package: &PackageInfo) -> Result<(), DynError> {
    let stylesheet = rel_link(&package.modules_dir, &libraries_root.join(DOCGEN_CSS_PATH))?;
    let mut rows = String::new();

    for module in &package.modules {
        let import_name = format!("{}.{}", package.manifest.name, module.name);
        let docs_cell = if module.docs_exists {
            format!(
                "<a href=\"./{}.html\">{}.html</a>",
                module.name, module.name
            )
        } else {
            "—".to_string()
        };

        write!(
            rows,
            "<tr><td><code>{}</code></td><td>{}</td><td><a href=\"./{}.source.html\">{}.source.html</a></td><td><a href=\"./{}.hml\">{}.hml</a></td><td><code title=\"{}\">{}</code></td></tr>",
            escape_html(&import_name),
            docs_cell,
            module.name,
            module.name,
            module.name,
            module.name,
            escape_html(&module.hash),
            escape_html(&trim_hash(&module.hash))
        )?;
    }

    let header = format!(
        "<div class=\"eyebrow\"><a href=\"../index.html\">{} {}</a></div><h1>modules/</h1><p>Raw module files served for named imports plus syntax-highlighted source pages generated from the Tree-sitter grammar.</p>",
        escape_html(&package.manifest.name),
        escape_html(&package.manifest.version)
    );
    let body = format!(
        "<section class=\"panel\"><h2>Module files</h2><table><thead><tr><th>Import</th><th>Docs</th><th>Highlighted source</th><th>Raw file</th><th>BLAKE3</th></tr></thead><tbody>{}</tbody></table></section>",
        rows
    );

    fs::write(
        package.modules_dir.join("index.html"),
        page_shell(
            &format!(
                "{} {} modules/",
                package.manifest.name, package.manifest.version
            ),
            &stylesheet,
            &header,
            &body,
        ),
    )?;
    Ok(())
}

fn write_module_source_page(
    libraries_root: &Path,
    package: &PackageInfo,
    module: &ModuleInfo,
) -> Result<(), DynError> {
    let stylesheet = rel_link(&package.modules_dir, &libraries_root.join(DOCGEN_CSS_PATH))?;
    let import_name = format!("{}.{}", package.manifest.name, module.name);
    let docs_meta = if module.docs_exists {
        format!(
            "Narrative docs: <a href=\"./{}.html\">{}.html</a><br>",
            module.name, module.name
        )
    } else {
        String::new()
    };

    let exports = if module.exports.is_empty() {
        "<p class=\"meta\">No exported declarations were detected.</p>".to_string()
    } else {
        let mut items = String::new();
        for export in &module.exports {
            let detail = export
                .detail
                .as_ref()
                .map(|detail| format!(" <span class=\"meta\">{}</span>", escape_html(detail)))
                .unwrap_or_default();
            write!(
                items,
                "<li><code>{}</code> <strong>{}</strong>{}</li>",
                export.kind,
                escape_html(&export.name),
                detail
            )?;
        }
        format!("<ul class=\"siglist\">{}</ul>", items)
    };

    let header = format!(
        "<div class=\"eyebrow\"><a href=\"./index.html\">{} modules</a></div><h1>{}</h1><p>Syntax-highlighted source generated from <code>tree-sitter-hiko</code>.</p>",
        escape_html(&package.manifest.name),
        escape_html(&import_name)
    );
    let body = format!(
        "<section class=\"panel\"><h2>Use</h2><pre class=\"code-block\"><code><span class=\"tok-keyword\">import</span> <span class=\"tok-namespace\">{}</span>.<span class=\"tok-module\">{}</span></code></pre><p class=\"meta\">{}Raw source: <a href=\"./{}.hml\">{}.hml</a><br>BLAKE3: <code title=\"{}\">{}</code></p></section>\
         <section class=\"panel\"><h2>Detected exports</h2>{}</section>\
         <section class=\"panel\"><h2>Source</h2><pre class=\"code-block\"><code>{}</code></pre></section>",
        escape_html(&package.manifest.name),
        escape_html(&module.name),
        docs_meta,
        module.name,
        module.name,
        escape_html(&module.hash),
        escape_html(&trim_hash(&module.hash)),
        exports,
        module.highlighted_html
    );

    fs::write(
        &module.source_page_path,
        page_shell(
            &format!("{} source", import_name),
            &stylesheet,
            &header,
            &body,
        ),
    )?;
    Ok(())
}

fn rewrite_narrative_doc_page(module: &ModuleInfo) -> Result<(), DynError> {
    if !module.docs_exists {
        return Ok(());
    }

    let html = fs::read_to_string(&module.docs_path)?;
    let html = ensure_narrative_doc_css_patch(&html);
    let html = highlight_html_code_blocks(&html)?;
    fs::write(&module.docs_path, html)?;
    Ok(())
}

fn ensure_narrative_doc_css_patch(html: &str) -> String {
    if html.contains(NARRATIVE_DOC_PATCH_MARKER) {
        return html.to_owned();
    }

    html.replacen(
        "</style>",
        &format!("{NARRATIVE_DOC_CSS_PATCH}\n  </style>"),
        1,
    )
}

fn highlight_html_code_blocks(html: &str) -> Result<String, DynError> {
    let mut output = String::with_capacity(html.len() + html.len() / 4);
    let mut rest = html;

    while let Some(pre_start) = rest.find("<pre") {
        output.push_str(&rest[..pre_start]);
        rest = &rest[pre_start..];

        let Some(pre_tag_end) = rest.find('>') else {
            output.push_str(rest);
            return Ok(output);
        };
        let after_pre = &rest[pre_tag_end + 1..];

        let Some(code_open_start_rel) = after_pre.find("<code") else {
            output.push_str(rest);
            return Ok(output);
        };
        let before_code = &after_pre[..code_open_start_rel];
        if !before_code.trim().is_empty() {
            output.push_str(rest);
            return Ok(output);
        }

        let code_section = &after_pre[code_open_start_rel..];
        let Some(code_tag_end_rel) = code_section.find('>') else {
            output.push_str(rest);
            return Ok(output);
        };
        let code_body = &code_section[code_tag_end_rel + 1..];
        let Some(code_close_rel) = code_body.find("</code>") else {
            output.push_str(rest);
            return Ok(output);
        };
        let inner_html = &code_body[..code_close_rel];
        let after_code = &code_body[code_close_rel + "</code>".len()..];
        let Some(pre_close_rel) = after_code.find("</pre>") else {
            output.push_str(rest);
            return Ok(output);
        };
        let between_code_and_pre = &after_code[..pre_close_rel];
        if !between_code_and_pre.trim().is_empty() {
            output.push_str(rest);
            return Ok(output);
        }

        let raw_code = decode_html_entities(&strip_span_tags(inner_html));
        let highlighted = highlight_source(&raw_code)?;
        output.push_str("<pre class=\"code-block\"><code>");
        output.push_str(&highlighted);
        output.push_str("</code></pre>");
        rest = &after_code[pre_close_rel + "</pre>".len()..];
    }

    output.push_str(rest);
    Ok(output)
}

fn strip_span_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut rest = html;

    loop {
        let open = rest.find("<span");
        let close = rest.find("</span>");

        match (open, close) {
            (Some(open_idx), Some(close_idx)) if close_idx < open_idx => {
                result.push_str(&rest[..close_idx]);
                rest = &rest[close_idx + "</span>".len()..];
            }
            (Some(open_idx), _) => {
                result.push_str(&rest[..open_idx]);
                let after_open = &rest[open_idx..];
                if let Some(tag_end) = after_open.find('>') {
                    rest = &after_open[tag_end + 1..];
                } else {
                    result.push_str(after_open);
                    break;
                }
            }
            (None, Some(close_idx)) => {
                result.push_str(&rest[..close_idx]);
                rest = &rest[close_idx + "</span>".len()..];
            }
            (None, None) => {
                result.push_str(rest);
                break;
            }
        }
    }

    result
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn page_shell(title: &str, stylesheet_href: &str, header: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><link rel=\"stylesheet\" href=\"{}\"></head><body><main>{}{}</main></body></html>",
        escape_html(title),
        escape_html(stylesheet_href),
        header,
        body
    )
}

fn trim_hash(hash: &str) -> String {
    if hash.len() <= 23 {
        return hash.to_owned();
    }

    let prefix = &hash[..15];
    let suffix = &hash[hash.len() - 8..];
    format!("{prefix}...{suffix}")
}

fn field_text(node: Node<'_>, field_name: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field_name)
        .map(|child| node_text(child, source))
}

fn node_text(node: Node<'_>, source: &str) -> String {
    source[node.byte_range()].to_owned()
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn rel_link(from_dir: &Path, to_file: &Path) -> Result<String, DynError> {
    let from = fs::canonicalize(from_dir)?;
    let to = fs::canonicalize(to_file)?;

    let from_parts = path_parts(&from);
    let to_parts = path_parts(&to);

    let common_prefix_len = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(left, right)| left == right)
        .count();

    let mut result_parts = Vec::new();
    for _ in common_prefix_len..from_parts.len() {
        result_parts.push("..".to_owned());
    }
    for part in &to_parts[common_prefix_len..] {
        result_parts.push(part.clone());
    }

    if result_parts.is_empty() {
        Ok(".".to_owned())
    } else {
        Ok(result_parts.join("/"))
    }
}

fn path_parts(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        NARRATIVE_DOC_PATCH_MARKER, decode_html_entities, ensure_narrative_doc_css_patch,
        escape_html, extract_exports, highlight_html_code_blocks, highlight_source,
        strip_span_tags, trim_hash,
    };

    #[test]
    fn trim_hash_shortens_for_display() {
        let hash = "blake3:321a2d3733c2d1b68f38373e53f963c3ea3b2a36b37a275a5a4be063357d2b46";
        assert_eq!(trim_hash(hash), "blake3:321a2d37...357d2b46");
    }

    #[test]
    fn highlight_source_emits_token_classes() {
        let html = highlight_source("structure Option = struct val x = 1 end").unwrap();
        assert!(html.contains("tok-keyword"));
        assert!(html.contains("tok-module"));
        assert!(html.contains("tok-number"));
    }

    #[test]
    fn extract_exports_finds_functions_and_datatypes() {
        let source = "\
structure Option = struct
  datatype 'a option = None | Some of 'a
  fun is_some x = true
  val default = None
end
";
        let exports = extract_exports(source).unwrap();
        assert!(
            exports
                .iter()
                .any(|item| item.kind == "datatype" && item.name.contains("option"))
        );
        assert!(
            exports
                .iter()
                .any(|item| item.kind == "fun" && item.name == "is_some")
        );
        assert!(
            exports
                .iter()
                .any(|item| item.kind == "val" && item.name == "default")
        );
    }

    #[test]
    fn escape_html_handles_markup() {
        assert_eq!(escape_html("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn narrative_doc_patch_is_idempotent() {
        let html = "<style>body { color: black; }\n  </style>";
        let once = ensure_narrative_doc_css_patch(html);
        let twice = ensure_narrative_doc_css_patch(&once);
        assert_eq!(once, twice);
        assert!(once.contains(NARRATIVE_DOC_PATCH_MARKER));
    }

    #[test]
    fn strip_span_tags_removes_existing_highlight_markup() {
        let html =
            "<span class=\"tok-keyword\">import</span> <span class=\"tok-namespace\">Std</span>";
        assert_eq!(strip_span_tags(html), "import Std");
    }

    #[test]
    fn decode_html_entities_restores_source_text() {
        assert_eq!(
            decode_html_entities("&lt;tag attr=&quot;x&quot;&gt;&amp;&#39;"),
            "<tag attr=\"x\">&'"
        );
    }

    #[test]
    fn highlight_html_code_blocks_rewrites_pre_sections() {
        let html = "<pre><code>import Std.Option\nval x = Option.Some 1</code></pre>";
        let output = highlight_html_code_blocks(html).unwrap();
        assert!(output.contains("pre class=\"code-block\""));
        assert!(output.contains("tok-keyword"));
        assert!(output.contains("tok-namespace"));
        assert!(output.contains("tok-module"));
        assert!(output.contains("tok-number"));
    }

    #[test]
    fn highlight_html_code_blocks_is_idempotent_for_already_highlighted_blocks() {
        let html = "<pre class=\"code-block\"><code><span class=\"tok-keyword\">import</span> <span class=\"tok-namespace\">Std</span>.<span class=\"tok-module\">Option</span></code></pre>";
        let output = highlight_html_code_blocks(html).unwrap();
        assert_eq!(output.matches("tok-keyword").count(), 1);
        assert_eq!(output.matches("tok-module").count(), 1);
    }

    #[test]
    fn highlight_html_code_blocks_rewrites_later_plain_blocks_after_highlighted_ones() {
        let html = "\
<pre class=\"code-block\"><code><span class=\"tok-keyword\">import</span> <span class=\"tok-namespace\">Std</span>.<span class=\"tok-module\">Json</span></code></pre>
<pre><code>import Std.Json

val input_json =
  \"{\\\"count\\\":3,\\\"ok\\\":true,\\\"tags\\\":[\\\"ml\\\",\\\"effects\\\"]}\"

val parsed = json_parse input_json</code></pre>";
        let output = highlight_html_code_blocks(html).unwrap();
        assert_eq!(output.matches("pre class=\"code-block\"").count(), 2);
        assert!(output.contains("json_parse"));
        assert!(output.contains("tok-string"));
    }
}
