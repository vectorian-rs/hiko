use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("github_issue_create", "capabilities.github.issue.create"),
    BuiltinMeta::opt_in("github_issue_view", "capabilities.github.issue.view"),
    BuiltinMeta::opt_in("github_issue_update", "capabilities.github.issue.update"),
    BuiltinMeta::opt_in("github_issue_list", "capabilities.github.issue.list"),
];
