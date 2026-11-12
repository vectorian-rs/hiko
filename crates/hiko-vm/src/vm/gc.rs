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
                return Err(GcError::SCRATCH);
            }
        }
        if self.forced {
            if let Some(lease) = self.lease {
                lease.debit_gc(units).map_err(GcError)?;
            } else if let Some(budget) = self.budget {
                // Dispatch always holds an entry lease; only direct collection
                // calls without one (tests) charge the grant here.
                let message = crate::process::FUEL_EXHAUSTED_MESSAGE;
                let cost = units.ok_or(GcError(message))?;
                budget.try_charge(cost).map_err(|_| GcError(message))?;
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
    /// The first checked GC failure is terminal for this VM (D5).
    pub(super) fn gc_failed(&self) -> Result<(), RuntimeError> {
        match &self.gc_failure {
            Some(message) => Err(RuntimeError {
                message: message.clone(),
            }),
            None => Ok(()),
        }
    }
    fn latch_gc_error(&mut self, error: GcError) -> RuntimeError {
        // A terminal cause also invalidates any not-yet-delivered request.
        self.pending_runtime_request = None;
        let message = self
            .gc_failure
            .get_or_insert_with(|| error.message().into());
        RuntimeError {
            message: message.clone(),
        }
    }
    /// Call only while all references needed by the instruction are VM roots.
    /// This checks the complete instruction, not a reservation: shared charges
    /// can still fail in any subsequent allocation.
    pub(super) fn allocation_safepoint(
        &mut self,
        objects: usize,
        bytes: usize,
    ) -> Result<(), RuntimeError> {
        self.gc_failed()?;
        let object_limit = |heap: &Heap| {
            heap.max_objects().filter(|&limit| {
                heap.live_count()
                    .checked_add(objects)
                    .is_none_or(|live| live > limit)
            })
        };
        // Whole-instruction limit pressure wins even when cadence is also due.
        let forced = self.heap.allocation_would_exceed_limits(bytes)
            || (objects > 1 && object_limit(&self.heap).is_some());
        if forced || self.heap.should_collect() {
            self.gc_collect_checked(&[], forced)?;
        }
        if let Some(limit) = object_limit(&self.heap) {
            return Err(RuntimeError {
                message: crate::heap::HeapLimitExceeded::Objects {
                    live: self.heap.live_count(),
                    limit,
                }
                .to_string(),
            });
        }
        self.heap
            .ensure_can_allocate_bytes(bytes)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })
    }
    pub(super) fn alloc(&mut self, obj: HeapObject) -> Result<Value, RuntimeError> {
        self.gc_failed()?;
        self.heap
            .alloc(obj)
            .map(Value::Heap)
            .map_err(|e| RuntimeError {
                message: e.to_string(),
            })
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
    /// Production safepoints use only VM roots; `extra_roots` supports tests.
    pub(super) fn gc_collect_checked(
        &mut self,
        extra_roots: &[GcRef],
        forced: bool,
    ) -> Result<(), RuntimeError> {
        self.gc_failed()?;
        // Ordinary/boundary GC is unmetered: every charge below would be a
        // no-op, so skip the prologue and hand try_collect a free debit.
        // Active fault/trace seams keep the fully metered path, so ordinary
        // tests exercise the same unmetered branch production takes.
        #[cfg(test)]
        let metered = forced || self.gc_test_fault.is_some() || self.gc_test_trace.is_some();
        #[cfg(not(test))]
        let metered = forced;
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
            if metered {
                // Keep this prepay list in sync with the roots chain below: a
                // container missing here under-charges, one missing there
                // leaks live objects.
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
            }
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
            if metered {
                self.heap
                    .try_collect(roots, |stage, units| debit.charge(stage, units))
            } else {
                self.heap.try_collect(roots, |_, _| Ok(()))
            }
        })();
        result.map_err(|e| self.latch_gc_error(e))
    }

    /// Existing expected-success test seam.
    #[cfg(test)]
    pub(super) fn gc_collect_with_extra_roots(
        &mut self,
        roots: impl IntoIterator<Item = GcRef>,
    ) -> Result<(), RuntimeError> {
        let extra: Vec<GcRef> = roots.into_iter().collect();
        self.gc_collect_checked(&extra, false)
    }
    pub(super) fn gc_collect_at_boundary_if_needed(&mut self) -> Result<(), RuntimeError> {
        if self.heap.should_collect_at_boundary() {
            self.gc_collect_checked(&[], false)?;
        }
        Ok(())
    }
}
