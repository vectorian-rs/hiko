use crate::BuiltinMeta;

pub const BUILTINS: &[BuiltinMeta] = &[
    BuiltinMeta::opt_in("read_file", "capabilities.filesystem.read_text"),
    BuiltinMeta::opt_in("read_file_bytes", "capabilities.filesystem.read_bytes"),
    BuiltinMeta::opt_in("write_file", "capabilities.filesystem.write_text"),
    BuiltinMeta::opt_in("file_exists", "capabilities.filesystem.exists"),
    BuiltinMeta::opt_in("list_dir", "capabilities.filesystem.list_entries"),
    BuiltinMeta::opt_in("remove_file", "capabilities.filesystem.remove"),
    BuiltinMeta::opt_in("create_dir", "capabilities.filesystem.create_directory"),
    BuiltinMeta::opt_in("is_dir", "capabilities.filesystem.is_directory"),
    BuiltinMeta::opt_in("is_file", "capabilities.filesystem.is_regular_file"),
    BuiltinMeta::opt_in("glob", "capabilities.filesystem.glob_matches"),
    BuiltinMeta::opt_in("walk_dir", "capabilities.filesystem.walk"),
];
