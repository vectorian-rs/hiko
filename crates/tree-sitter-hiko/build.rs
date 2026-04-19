fn main() {
    let mut build = cc::Build::new();
    build
        .include("src")
        .file("src/parser.c")
        .flag_if_supported("-std=c11")
        .warnings(false)
        .compile("tree-sitter-hiko");

    println!("cargo:rerun-if-changed=grammar.js");
    println!("cargo:rerun-if-changed=tree-sitter.json");
    println!("cargo:rerun-if-changed=src/parser.c");
    println!("cargo:rerun-if-changed=src/node-types.json");
    println!("cargo:rerun-if-changed=queries/highlights.scm");
}
