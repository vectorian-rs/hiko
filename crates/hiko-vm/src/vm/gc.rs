//! GC-aware allocation helpers and interpreter root management.
//!
//! The VM keeps heap allocation and GC root calculation local so that the
//! runtime does not need to understand interpreter-internal references.

use super::*;

impl VM {
    pub(super) fn heap_get(&self, r: GcRef) -> Result<&HeapObject, RuntimeError> {
        self.heap
            .get(r)
            .map_err(|e| RuntimeError { message: e.into() })
    }

    pub(super) fn alloc(&mut self, obj: HeapObject) -> Result<Value, RuntimeError> {
        if self.heap.should_collect() {
            let mut extra_roots = Vec::new();
            obj.for_each_gc_ref(|r| extra_roots.push(r));
            self.gc_collect_with_extra_roots(extra_roots);
        }
        self.heap
            .alloc(obj)
            .map(Value::Heap)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })
    }

    pub(super) fn alloc_string(&mut self, s: String) -> Result<Value, RuntimeError> {
        self.alloc(HeapObject::String(s))
    }

    pub(super) fn checked_relative_ip(
        &self,
        proto_idx: usize,
        base_after_operand: usize,
        offset: i16,
        what: &str,
    ) -> Result<usize, RuntimeError> {
        let target = base_after_operand
            .checked_add_signed(offset as isize)
            .ok_or_else(|| RuntimeError {
                message: format!("{what}: instruction pointer overflow"),
            })?;
        if target > self.chunk_for_checked(proto_idx)?.code.len() {
            return Err(RuntimeError {
                message: format!("{what}: relative jump target {target} lands outside chunk"),
            });
        }
        Ok(target)
    }

    pub(super) fn capture_value(
        &self,
        frame_idx: usize,
        is_local: bool,
        index: usize,
    ) -> Result<Value, RuntimeError> {
        if is_local {
            let base = self.frames[frame_idx].base;
            let slot = base.checked_add(index).ok_or_else(|| RuntimeError {
                message: "MakeClosure: local capture index overflow".into(),
            })?;
            self.stack.get(slot).copied().ok_or_else(|| RuntimeError {
                message: format!("MakeClosure: local capture index {index} out of bounds"),
            })
        } else {
            self.frames[frame_idx]
                .captures
                .get(index)
                .copied()
                .ok_or_else(|| RuntimeError {
                    message: format!("MakeClosure: upvalue index {index} out of bounds"),
                })
        }
    }

    /// Collect with the current execution state plus any request-local roots.
    ///
    /// The root set intentionally includes the blocked continuation because
    /// async I/O suspends the process between slices.
    pub(super) fn gc_collect_with_extra_roots(
        &mut self,
        extra_roots: impl IntoIterator<Item = GcRef>,
    ) {
        let roots = self
            .stack
            .iter()
            .chain(self.frames.iter().flat_map(|f| f.captures.iter()))
            .chain(self.globals.iter())
            .chain(self.handlers.iter().flat_map(|h| h.captures.iter()))
            .filter_map(|v| match v {
                Value::Heap(r) => Some(*r),
                _ => None,
            })
            .chain(self.string_cache.values().copied())
            .chain(self.blocked_continuation.iter().copied())
            .chain(extra_roots);
        self.heap.collect(roots);
    }

    /// Run a cheaper boundary collection when a process is about to yield or
    /// suspend. This keeps short-lived request garbage from accumulating across
    /// slices.
    pub(super) fn gc_collect_at_boundary_if_needed(&mut self) {
        if self.heap.should_collect_at_boundary() {
            self.gc_collect_with_extra_roots(std::iter::empty());
        }
    }
}
