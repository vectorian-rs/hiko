use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("read_file", "capabilities.filesystem.read_file"),
    BuiltinMeta::opt_in("read_file_bytes", "capabilities.filesystem.read_file_bytes"),
    BuiltinMeta::opt_in("write_file", "capabilities.filesystem.write_file"),
    BuiltinMeta::opt_in("file_exists", "capabilities.filesystem.file_exists"),
    BuiltinMeta::opt_in("list_dir", "capabilities.filesystem.list_dir"),
    BuiltinMeta::opt_in("remove_file", "capabilities.filesystem.remove_file"),
    BuiltinMeta::opt_in("create_dir", "capabilities.filesystem.create_dir"),
    BuiltinMeta::opt_in("is_dir", "capabilities.filesystem.is_dir"),
    BuiltinMeta::opt_in("is_file", "capabilities.filesystem.is_file"),
    BuiltinMeta::opt_in(
        "read_file_tagged",
        "capabilities.filesystem.read_file_tagged",
    ),
    BuiltinMeta::opt_in(
        "edit_file_tagged",
        "capabilities.filesystem.edit_file_tagged",
    ),
    BuiltinMeta::opt_in("glob", "capabilities.filesystem.glob"),
    BuiltinMeta::opt_in("walk_dir", "capabilities.filesystem.walk_dir"),
];
