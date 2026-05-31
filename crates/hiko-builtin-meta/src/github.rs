use crate::{BuiltinMeta, BuiltinSurface};

const PUBLIC: BuiltinSurface = BuiltinSurface::Public;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta {
        name: "github_issue_create",
        capability_path: Some("capabilities.github.issue.create"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "github_issue_view",
        capability_path: Some("capabilities.github.issue.view"),
        in_core_default: false,
        surface: PUBLIC,
    },
    BuiltinMeta {
        name: "github_issue_update",
        capability_path: Some("capabilities.github.issue.update"),
        in_core_default: false,
        surface: PUBLIC,
    },
];
