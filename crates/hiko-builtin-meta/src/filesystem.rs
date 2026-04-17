use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "read_file",
        capability_path: Some("capabilities.filesystem.read_file"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "read_file_bytes",
        capability_path: Some("capabilities.filesystem.read_file_bytes"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "write_file",
        capability_path: Some("capabilities.filesystem.write_file"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "file_exists",
        capability_path: Some("capabilities.filesystem.file_exists"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "list_dir",
        capability_path: Some("capabilities.filesystem.list_dir"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "remove_file",
        capability_path: Some("capabilities.filesystem.remove_file"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "create_dir",
        capability_path: Some("capabilities.filesystem.create_dir"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "is_dir",
        capability_path: Some("capabilities.filesystem.is_dir"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "is_file",
        capability_path: Some("capabilities.filesystem.is_file"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "read_file_tagged",
        capability_path: Some("capabilities.filesystem.read_file_tagged"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "edit_file_tagged",
        capability_path: Some("capabilities.filesystem.edit_file_tagged"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "glob",
        capability_path: Some("capabilities.filesystem.glob"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "walk_dir",
        capability_path: Some("capabilities.filesystem.walk_dir"),
        in_core_default: false,
        surface: PUBLIC,
    },
];
