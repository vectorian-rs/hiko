use super::*;
use ::regex::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

// Host work in opcode-equivalents, from the host_work_calibration example
// (docs/builtins/accounting-matrix.md#calibration). The pattern is compiled
// on every call: the fixed part covers a short pattern (a literal compiles
// in about 700 units, a character class in about 2,800), and longer patterns
// pay per KiB. The text costs little to scan; `regex_replace` also builds
// its output.
const REGEX_HOST_WORK: u64 = 1_500;
const REGEX_PATTERN_PER_KIB_HOST_WORK: u64 = 11_500;
const REGEX_MATCH_PER_KIB_HOST_WORK: u64 = 2;
const REGEX_REPLACE_PER_KIB_HOST_WORK: u64 = 130;

pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] {
    &[
        ("regex_match", regex_match as BuiltinFn),
        ("regex_replace", regex_replace),
    ]
}

/// Patterns a process tree keeps compiled.
const REGEX_CACHE_ENTRIES: usize = 32;
/// Size limits for a cached regex's compiled program and lazy DFA. A
/// pattern over them is compiled with the default limits and not cached, so
/// the cache holds at most about `REGEX_CACHE_ENTRIES` x 512 KiB.
const CACHED_REGEX_SIZE_LIMIT: usize = 256 * 1024;
const CACHED_REGEX_DFA_SIZE_LIMIT: usize = 256 * 1024;

/// Compiled regexes shared by a process tree, most recently used first.
/// `regex_match` and `regex_replace` compile their pattern on every call
/// otherwise. Host work is charged as if the pattern were compiled, so the
/// cache changes the time a call takes, not what it costs.
#[derive(Debug, Default)]
pub(crate) struct RegexCache {
    // `Arc<Regex>`, not `Regex`: a cloned `Regex` gets its own search
    // cache, which it allocates again on its first search.
    entries: Mutex<Vec<(Arc<str>, Arc<Regex>)>>,
}

impl RegexCache {
    /// The compiled `pattern`, from the cache or compiled now.
    fn get(&self, pattern: &Arc<str>) -> Result<Arc<Regex>, ::regex::Error> {
        {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = entries.iter().position(|(p, _)| p == pattern) {
                let entry = entries.remove(i);
                let regex = entry.1.clone();
                entries.insert(0, entry);
                return Ok(regex);
            }
        }
        // Compile without the lock, so a slow pattern does not stall other
        // processes of the tree.
        let regex = match RegexBuilder::new(pattern)
            .size_limit(CACHED_REGEX_SIZE_LIMIT)
            .dfa_size_limit(CACHED_REGEX_DFA_SIZE_LIMIT)
            .build()
        {
            Ok(regex) => Arc::new(regex),
            Err(::regex::Error::CompiledTooBig(_)) => return Regex::new(pattern).map(Arc::new),
            Err(e) => return Err(e),
        };
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if !entries.iter().any(|(p, _)| p == pattern) {
            entries.insert(0, (pattern.clone(), regex.clone()));
            entries.truncate(REGEX_CACHE_ENTRIES);
        }
        Ok(regex)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

fn regex_work(per_kib: u64, text: &str, pattern: &str) -> u64 {
    host_work(REGEX_HOST_WORK, per_kib, text.len()).saturating_add(host_work(
        0,
        REGEX_PATTERN_PER_KIB_HOST_WORK,
        pattern.len(),
    ))
}

pub(super) fn regex_match(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let (s, pattern) = string_pair(args, heap, "regex_match")?;
    charge(
        heap,
        "regex_match",
        regex_work(REGEX_MATCH_PER_KIB_HOST_WORK, &s, &pattern),
    )?;
    let re = heap
        .regex_cache()
        .get(&pattern)
        .map_err(|e| format!("regex_match: {e}"))?;
    Ok(Value::Bool(re.is_match(&s)))
}

pub(super) fn regex_replace(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let expected = || "regex_replace: expected (String, String, String)".to_string();
    let fields = match &args[0] {
        Value::Heap(r) => match heap.get(*r).map_err(|e| e.to_string())? {
            HeapObject::Tuple(t) if t.len() >= 3 => [t[0], t[1], t[2]],
            _ => return Err(expected()),
        },
        _ => return Err(expected()),
    };
    let [s, pattern, replacement] = fields.map(|v| extract_string_arg(&[v], heap, "regex_replace"));
    let (s, pattern, replacement) = (s?, pattern?, replacement?);
    charge(
        heap,
        "regex_replace",
        regex_work(REGEX_REPLACE_PER_KIB_HOST_WORK, &s, &pattern),
    )?;
    let re = heap
        .regex_cache()
        .get(&pattern)
        .map_err(|e| format!("regex_replace: {e}"))?;
    let replaced = re.replace_all(&s, &*replacement);
    let output_len = match &replaced {
        Cow::Borrowed(text) => text.len(),
        Cow::Owned(text) => text.len(),
    };
    heap.ensure_can_allocate_bytes(size_of::<HeapObject>().saturating_add(output_len))
        .map_err(|e| format!("regex_replace: {e}"))?;
    heap_alloc(heap, HeapObject::String(replaced.into_owned().into()))
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;
    use super::*;

    #[test]
    fn regex_match_basic_true() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"hello \w+");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_basic_false() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"^\d+$");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, false);
    }

    #[test]
    fn regex_match_respects_host_work_limit() {
        let mut heap = Heap::new();
        heap.set_max_host_work(REGEX_HOST_WORK - 1);
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"hello \w+");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("host work budget exceeded"));
    }

    #[test]
    fn regex_match_charges_host_work() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello world");
        let pat = string_arg(&mut heap, r"hello \w+");
        let arg = tuple2(&mut heap, s, pat);
        regex_match(&[arg], &mut heap).unwrap();
        assert!(heap.host_work_used() >= REGEX_HOST_WORK);
    }

    #[test]
    fn regex_match_partial() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc123def");
        let pat = string_arg(&mut heap, r"\d+");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_empty_string() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "");
        let pat = string_arg(&mut heap, r"^$");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap).unwrap();
        assert_bool(result, true);
    }

    #[test]
    fn regex_match_invalid_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let pat = string_arg(&mut heap, r"[invalid");
        let arg = tuple2(&mut heap, s, pat);
        let result = regex_match(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("regex_match"));
    }

    #[test]
    fn regex_match_type_error() {
        let mut heap = Heap::new();
        let result = regex_match(&[Value::Int(42)], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected"));
    }

    #[test]
    fn regex_replace_basic() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello 123 world 456");
        let pat = string_arg(&mut heap, r"\d+");
        let rep = string_arg(&mut heap, "NUM");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello NUM world NUM");
    }

    #[test]
    fn regex_replace_no_match() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "hello");
        let pat = string_arg(&mut heap, r"\d+");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "hello");
    }

    #[test]
    fn regex_replace_with_capture_group() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "foo bar baz");
        let pat = string_arg(&mut heap, r"(\w+)");
        let rep = string_arg(&mut heap, "[$1]");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        assert_eq!(heap_string(result, &heap), "[foo] [bar] [baz]");
    }

    #[test]
    fn regex_replace_invalid_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "test");
        let pat = string_arg(&mut heap, r"(unclosed");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("regex_replace"));
    }

    #[test]
    fn regex_replace_empty_pattern() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc");
        let pat = string_arg(&mut heap, "");
        let rep = string_arg(&mut heap, "X");
        let arg = tuple3(&mut heap, s, pat, rep);
        let result = regex_replace(&[arg], &mut heap).unwrap();
        // Empty pattern matches every position: before each char and at end
        assert_eq!(heap_string(result, &heap), "XaXbXcX");
    }

    #[test]
    fn regex_replace_checks_output_against_memory_limit() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc");
        let pat = string_arg(&mut heap, "");
        let rep = string_arg(&mut heap, "XXXX");
        let arg = tuple3(&mut heap, s, pat, rep);
        heap.set_max_bytes(heap.live_bytes() + size_of::<HeapObject>() + 18);

        let err = regex_replace(&[arg], &mut heap).unwrap_err();
        assert!(err.contains("regex_replace: memory limit exceeded"));
    }

    #[test]
    fn regex_cache_reuses_a_pattern_and_stays_bounded() {
        let cache = RegexCache::default();
        let pattern: Arc<str> = Arc::from("^a+$");
        assert!(cache.get(&pattern).unwrap().is_match("aaa"));
        assert!(cache.get(&pattern).unwrap().is_match("a"));
        assert_eq!(cache.len(), 1);
        for i in 0..100 {
            let pattern: Arc<str> = Arc::from(format!("^x{i}$").as_str());
            assert!(cache.get(&pattern).unwrap().is_match(&format!("x{i}")));
        }
        assert_eq!(cache.len(), REGEX_CACHE_ENTRIES);
    }

    /// A pattern over the cache's size limits still compiles, with the
    /// default limits, and is not kept.
    #[test]
    fn regex_cache_does_not_keep_large_patterns() {
        let cache = RegexCache::default();
        let pattern: Arc<str> = Arc::from(r"\w{100}");
        assert!(cache.get(&pattern).is_ok());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn child_heap_shares_the_regex_cache() {
        let parent = Heap::new();
        let mut child = Heap::new();
        child.share_regex_cache(&parent);
        assert!(std::ptr::eq(parent.regex_cache(), child.regex_cache()));
    }

    /// A cache hit is charged like compiling the pattern.
    #[test]
    fn regex_cache_hit_is_charged_like_a_miss() {
        let mut heap = Heap::new();
        let s = string_arg(&mut heap, "abc");
        let pat = string_arg(&mut heap, "b+");
        let arg = tuple2(&mut heap, s, pat);
        regex_match(&[arg], &mut heap).unwrap();
        let first = heap.host_work_used();
        regex_match(&[arg], &mut heap).unwrap();
        assert_eq!(heap.host_work_used(), 2 * first);
    }
}
