//! GC-aware allocation and prepaid interpreter root management.
use super::*;
use crate::heap::{GcError, GcStage, gc_units};

/// A split borrow keeps roots/heap available while the debit owns only credit.
struct GcDebit<'a> {
    forced: bool,
    lease: &'a mut Option<FuelLease>,
    budget: &'a Option<Arc<crate::budget::SharedBudget>>,
    #[cfg(test)]
    fault: &'a mut Option<GcStage>,
    #[cfg(test)]
    trace: &'a mut Option<Vec<(GcStage, Option<u64>)>>,
}
impl GcDebit<'_> {
    fn charge(&mut self, stage: GcStage, units: Option<u64>) -> Result<(), GcError> {
        #[cfg(not(test))]
        let _ = stage;
        #[cfg(test)]
        {
            if let Some(trace) = self.trace {
                trace.push((stage, units));
            }
            if *self.fault == Some(stage) {
                self.fault.take();
                return Err(GcError::Scratch);
            }
        }
        if self.forced {
            if let Some(lease) = self.lease {
                lease.debit_gc(units).map_err(GcError::Work)?;
            } else if let Some(budget) = self.budget {
                let cost = units.ok_or(GcError::Work("fuel exhausted (max_fuel limit reached)"))?;
                budget
                    .try_charge(cost)
                    .map_err(|_| GcError::Work("fuel exhausted (max_fuel limit reached)"))?;
            }
        }
        Ok(())
    }
    fn container(&mut self, stage: GcStage, length: usize) -> Result<(), GcError> {
        self.charge(stage, gc_units(length).and_then(|n| n.checked_add(1)))
    }
}

impl VM {
    pub(super) fn heap_get(&self, r: GcRef) -> Result<&HeapObject, RuntimeError> {
        self.heap
            .get(r)
            .map_err(|e| RuntimeError { message: e.into() })
    }
    fn latch_gc_error(&mut self, error: GcError) -> RuntimeError {
        let message = self
            .gc_failure
            .get_or_insert_with(|| error.message().into());
        RuntimeError {
            message: message.clone(),
        }
    }
    pub(super) fn alloc(&mut self, obj: HeapObject) -> Result<Value, RuntimeError> {
        if let Some(message) = &self.gc_failure {
            return Err(RuntimeError {
                message: message.clone(),
            });
        }
        // Limit pressure wins classification even when the schedule is also due.
        let forced = self
            .heap
            .allocation_would_exceed_limits(obj.estimated_bytes());
        if forced || self.heap.should_collect() {
            let slots = obj.raw_gc_slots();
            let prepaid = GcDebit {
                forced,
                lease: &mut self.fuel_lease,
                budget: &self.fuel_budget,
                #[cfg(test)]
                fault: &mut self.gc_test_fault,
                #[cfg(test)]
                trace: &mut self.gc_test_trace,
            }
            .container(GcStage::PendingObject, slots);
            prepaid.map_err(|e| self.latch_gc_error(e))?;
            let mut extra_roots = Vec::new();
            extra_roots
                .try_reserve(slots)
                .map_err(|_| self.latch_gc_error(GcError::Scratch))?;
            obj.for_each_gc_ref(|r| extra_roots.push(r));
            self.gc_collect_checked(&extra_roots, forced)?;
        }
        self.heap
            .alloc(obj)
            .map(Value::Heap)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })
    }
    pub(super) fn alloc_string(&mut self, s: String) -> Result<Value, RuntimeError> {
        self.alloc(HeapObject::String(s.into()))
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

    /// Prepay each container and raw length before inspecting/filtering it.
    pub(super) fn gc_collect_checked(
        &mut self,
        extra_roots: &[GcRef],
        forced: bool,
    ) -> Result<(), RuntimeError> {
        if let Some(message) = &self.gc_failure {
            return Err(RuntimeError {
                message: message.clone(),
            });
        }
        let result = (|| {
            let mut debit = GcDebit {
                forced,
                lease: &mut self.fuel_lease,
                budget: &self.fuel_budget,
                #[cfg(test)]
                fault: &mut self.gc_test_fault,
                #[cfg(test)]
                trace: &mut self.gc_test_trace,
            };
            debit.container(GcStage::Stack, self.stack.len())?;
            debit.container(GcStage::Globals, self.globals.len())?;
            debit.container(GcStage::Frames, self.frames.len())?;
            for frame in &self.frames {
                debit.container(GcStage::Captures, frame.captures.len())?;
            }
            debit.container(GcStage::Handlers, self.handlers.len())?;
            for handler in &self.handlers {
                debit.container(GcStage::Captures, handler.captures.len())?;
            }
            debit.container(GcStage::HiddenHandlers, self.hidden_handlers.len())?;
            for hidden in &self.hidden_handlers {
                debit.container(GcStage::Handlers, hidden.handlers.len())?;
                for handler in &hidden.handlers {
                    debit.container(GcStage::Captures, handler.captures.len())?;
                }
            }
            debit.container(GcStage::Cache, self.string_cache.rows.len())?;
            for row in &self.string_cache.rows {
                debit.container(GcStage::Cache, row.len())?;
            }
            debit.container(GcStage::ExtraRoots, extra_roots.len())?;
            let roots = self
                .stack
                .iter()
                .chain(self.frames.iter().flat_map(|f| f.captures.iter()))
                .chain(self.globals.iter().flatten())
                .chain(self.handlers.iter().flat_map(|h| h.captures.iter()))
                .chain(
                    self.hidden_handlers
                        .iter()
                        .flat_map(|h| &h.handlers)
                        .flat_map(|h| h.captures.iter()),
                )
                .filter_map(|v| {
                    if let Value::Heap(r) = v {
                        Some(*r)
                    } else {
                        None
                    }
                })
                .chain(self.string_cache.values())
                .chain(extra_roots.iter().copied());
            self.heap
                .try_collect(roots, |stage, units| debit.charge(stage, units))
        })();
        result.map_err(|e| self.latch_gc_error(e))
    }

    /// Existing expected-success test seam, with checked scratch propagation.
    #[cfg(test)]
    pub(super) fn gc_collect_with_extra_roots(
        &mut self,
        roots: impl IntoIterator<Item = GcRef>,
    ) -> Result<(), RuntimeError> {
        let mut extra = Vec::new();
        for root in roots {
            extra
                .try_reserve(1)
                .map_err(|_| self.latch_gc_error(GcError::Scratch))?;
            extra.push(root);
        }
        self.gc_collect_checked(&extra, false)
    }
    pub(super) fn gc_collect_at_boundary_if_needed(&mut self) -> Result<(), RuntimeError> {
        if self.heap.should_collect_at_boundary() {
            self.gc_collect_checked(&[], false)?;
        }
        Ok(())
    }
}
