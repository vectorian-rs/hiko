//! Reader-only native admission. Persistent bytes live in object_bytes, not here.
use super::*;
use crate::value::Fields;
use hiko_json::{Limits, OwnedReader};
use std::mem::{self, size_of};

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionStage {
    Total,
    Slots,
    Owner,
}

/// Short construction/output transaction, never a second persistent ledger.
pub(crate) struct Reservation {
    bytes: usize,
    memory: Option<Arc<SharedBudget>>,
    total: Option<Arc<SharedBudget>>,
    kind: Option<Arc<SharedBudget>>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(b) = &self.memory {
            b.refund(self.bytes as u64);
        }
        if let Some(b) = &self.total {
            b.refund(1);
        }
        if let Some(b) = &self.kind {
            b.refund(1);
        }
    }
}
impl Reservation {
    fn commit(mut self, actual: usize) -> Result<(), String> {
        if actual > self.bytes {
            return Err("JSON reader allocation exceeds pre-admission".into());
        }
        if let Some(b) = self.memory.take() {
            b.refund((self.bytes - actual) as u64);
        }
        self.total = None;
        self.kind = None;
        Ok(())
    }
}

pub(crate) enum Output<'a> {
    Scalar(Value),
    Text(&'a str),
    Field(Option<&'a str>),
}

impl Heap {
    #[cfg(test)]
    fn json_admission_checkpoint(&self, stage: AdmissionStage) -> Result<(), String> {
        if self.json_reader_test_fault == Some(stage) {
            Err("injected JSON reader admission failure".into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn json_reader_creation_bytes(input: usize) -> Result<usize, String> {
        OwnedReader::admission_bytes(input)
            .and_then(|n| n.checked_add(size_of::<Vec<OwnedReader>>()))
            .and_then(|n| n.checked_add(Self::json_handle_bytes()))
            .ok_or_else(|| "JSON reader admission size overflow".into())
    }
    fn json_handle_bytes() -> usize {
        HeapObject::HostHandle {
            kind: HostHandleKind::JsonReader,
            id: HostHandleId(0),
        }
        .estimated_bytes()
    }
    pub(crate) fn json_reader_resource_pressure(&self) -> bool {
        self.host_resource_budget
            .as_ref()
            .is_some_and(|b| b.remaining() == 0)
            || self
                .host_resource_kind_budget(HostHandleKind::JsonReader)
                .is_some_and(|b| b.remaining() == 0)
    }
    pub(crate) fn ensure_json_reader_resource(&self) -> Result<(), String> {
        if self.next_host_handle_id == u64::MAX {
            return Err("JSON reader host handle IDs exhausted".into());
        }
        for (b, kind) in [
            (&self.host_resource_budget, None),
            (
                &self
                    .host_resource_kind_budget(HostHandleKind::JsonReader)
                    .cloned(),
                Some("json_reader"),
            ),
        ] {
            if let Some(b) = b
                && b.remaining() == 0
            {
                return Err(HeapLimitExceeded::HostResources {
                    kind: kind.map(str::to_owned),
                    live: b.used() as usize,
                    limit: b.limit() as usize,
                }
                .to_string());
            }
        }
        Ok(())
    }
    pub(crate) fn reserve_json_output(
        &mut self,
        objects: usize,
        bytes: usize,
    ) -> Result<Reservation, String> {
        if let Some(limit) = self.max_objects
            && self
                .live_count()
                .checked_add(objects)
                .is_none_or(|n| n > limit)
        {
            return Err(HeapLimitExceeded::Objects {
                live: self.live_count(),
                limit,
            }
            .to_string());
        }
        let memory = self.memory_budget.clone();
        if let Some(b) = &memory {
            b.try_charge(bytes as u64).map_err(|used| {
                HeapLimitExceeded::Bytes {
                    used_bytes: used as usize,
                    limit_bytes: b.limit() as usize,
                    attempted_bytes: bytes,
                }
                .to_string()
            })?;
        }
        let reservation = Reservation {
            bytes,
            memory,
            total: None,
            kind: None,
        };
        let peak = self
            .current_bytes
            .checked_add(bytes)
            .ok_or("JSON reader peak size overflow")?;
        self.peak_bytes = self.peak_bytes.max(peak);
        #[cfg(test)]
        if let Some(p) = &mut self.phase_peak_bytes {
            *p = (*p).max(peak);
        }
        Ok(reservation)
    }
    pub(crate) fn new_json_reader(&mut self, input: Arc<str>) -> Result<Value, String> {
        if input.len() > Limits::default().max_input_bytes {
            return Err(hiko_json::Error {
                kind: hiko_json::ErrorKind::InputLimit,
                offset: 0,
            }
            .to_string());
        }
        self.ensure_json_reader_resource()?;
        let bytes = Self::json_reader_creation_bytes(input.len())?;
        let mut admission = self.reserve_json_output(1, bytes)?;
        if let Some(b) = &self.host_resource_budget {
            b.try_charge(1).map_err(|live| {
                HeapLimitExceeded::HostResources {
                    kind: None,
                    live: live as usize,
                    limit: b.limit() as usize,
                }
                .to_string()
            })?;
            admission.total = Some(b.clone());
        }
        #[cfg(test)]
        self.json_admission_checkpoint(AdmissionStage::Total)?;
        if let Some(b) = self.host_resource_kind_budget(HostHandleKind::JsonReader) {
            b.try_charge(1).map_err(|live| {
                HeapLimitExceeded::HostResources {
                    kind: Some("json_reader".into()),
                    live: live as usize,
                    limit: b.limit() as usize,
                }
                .to_string()
            })?;
            admission.kind = Some(b.clone());
        }
        #[cfg(test)]
        self.json_admission_checkpoint(AdmissionStage::Slots)?;
        // Reserve owner storage before constructing its two buffers. No allocator
        // override or replaying Parser is used. Global allocator OOM may abort,
        // as for existing heap objects; reservation errors are recoverable.
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(1)
            .map_err(|_| "JSON reader owner allocation failed")?;
        self.host_resources
            .try_reserve(1)
            .map_err(|_| "JSON reader resource table allocation failed")?;
        // Abstract Rust credits aren't VM host-work units. Any VM call consumes
        // at least45 host credits, so this counter cannot precede a u64 VM grant.
        let owner = OwnedReader::new(
            input,
            Limits {
                max_work: u64::MAX,
                ..Limits::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let payload = owner
            .estimated_bytes()
            .checked_add(size_of::<Vec<OwnedReader>>())
            .and_then(|n| {
                n.checked_add((owners.capacity() - 1).checked_mul(size_of::<OwnedReader>())?)
            })
            .ok_or("JSON reader payload size overflow")?;
        owners.push(owner);
        #[cfg(test)]
        self.json_admission_checkpoint(AdmissionStage::Owner)?;
        let total = Self::json_handle_bytes()
            .checked_add(payload)
            .ok_or("JSON reader payload size overflow")?;
        // All checked failure happens before publishing either resource or handle.
        if total > admission.bytes {
            drop(owners);
            return Err("JSON reader allocation exceeds pre-admission".into());
        }
        admission.commit(total)?;
        let id = HostHandleId(self.next_host_handle_id);
        self.next_host_handle_id += 1;
        self.host_resources
            .insert(id, HostResource::JsonReader(owners));
        let r = self.alloc_charged(
            HeapObject::HostHandle {
                kind: HostHandleKind::JsonReader,
                id,
            },
            total,
        );
        let count = self
            .host_resources_by_kind
            .entry(HostHandleKind::JsonReader)
            .or_insert(0);
        *count += 1;
        self.peak_host_resources = self.peak_host_resources.max(self.host_resources.len());
        self.peak_host_resources_by_kind
            .entry(HostHandleKind::JsonReader)
            .and_modify(|p| *p = (*p).max(*count))
            .or_insert(*count);
        Ok(Value::Heap(r))
    }
    pub(crate) fn json_handle(&self, value: Value) -> Result<(GcRef, HostHandleId), String> {
        if let Value::Heap(r) = value
            && let HeapObject::HostHandle {
                kind: HostHandleKind::JsonReader,
                id,
            } = self.get(r).map_err(str::to_owned)?
        {
            return Ok((r, *id));
        }
        Err("expected JSON reader".into())
    }
    pub(crate) fn json_reader(&self, value: Value) -> Result<&OwnedReader, String> {
        let (_, id) = self.json_handle(value)?;
        match self.host_resources.get(&id) {
            Some(HostResource::JsonReader(v)) if v.len() == 1 => Ok(&v[0]),
            None => Err("JSON reader is closed or dangling".into()),
            _ => Err("invalid JSON reader resource entry".into()),
        }
    }
    pub(crate) fn close_json_reader(&mut self, value: Value) -> Result<(), String> {
        let (r, id) = self.json_handle(value)?;
        if id.0 == 0 {
            return Ok(());
        }
        if let Some(resource) = self.host_resources.get(&id)
            && resource.kind() != HostHandleKind::JsonReader
        {
            return Err("invalid JSON reader resource entry".into());
        }
        let Some(resource) = self.host_resources.remove(&id) else {
            return Err("JSON reader is dangling".into());
        };
        drop(resource); // Payload destruction MUST precede refund.
        // ID zero is the closed marker for this NEW kind only. No ID is reused,
        // and an unregistered fabricated nonzero ID isn't treated as closed.
        if let Some(HeapObject::HostHandle { id, .. }) = self.objects[r.0 as usize].as_mut() {
            *id = HostHandleId(0);
        }
        self.release_host_resource_budgets(HostHandleKind::JsonReader);
        let count = self
            .host_resources_by_kind
            .entry(HostHandleKind::JsonReader)
            .or_insert(0);
        *count -= 1;
        if *count == 0 {
            self.host_resources_by_kind
                .remove(&HostHandleKind::JsonReader);
        }
        let slot = &mut self.object_bytes[r.0 as usize];
        let payload = *slot - Self::json_handle_bytes();
        *slot -= payload;
        self.current_bytes -= payload;
        if let Some(b) = &self.memory_budget {
            b.refund(payload as u64);
        }
        Ok(())
    }
    /// No GC while leased. No reader borrows survive the callback. Payload
    /// remains charged in the handle's slot even while its Vec is local.
    pub(crate) fn with_json_reader<T>(
        &mut self,
        value: Value,
        f: impl FnOnce(&mut OwnedReader, &mut Heap) -> Result<T, String>,
    ) -> Result<T, String> {
        self.json_reader(value)?;
        let (_, id) = self.json_handle(value)?;
        let Some(HostResource::JsonReader(owners)) = self.host_resources.get_mut(&id) else {
            return Err("invalid JSON reader resource entry".into());
        };
        let mut owners = mem::take(owners);
        let result = f(&mut owners[0], self);
        match result {
            Ok(value) => {
                let Some(HostResource::JsonReader(slot)) = self.host_resources.get_mut(&id) else {
                    return Err("JSON reader lease entry lost".into());
                };
                *slot = owners;
                Ok(value)
            }
            Err(error) => {
                drop(owners);
                self.close_json_reader(value)?;
                Err(error)
            }
        }
    }
    pub(super) fn drop_json_reader_payloads(&mut self) {
        for resource in self.host_resources.values_mut() {
            if let HostResource::JsonReader(owners) = resource {
                drop(mem::take(owners));
            }
        }
    }
    pub(crate) fn json_output_bound(
        text_bytes: usize,
        field: bool,
    ) -> Result<(usize, usize), String> {
        let objects: usize = if field { 2 } else { 1 };
        let bytes = size_of::<HeapObject>()
            .checked_mul(objects)
            .and_then(|n| n.checked_add(text_bytes))
            .ok_or("JSON reader output size overflow")?;
        Ok((objects, bytes))
    }
    pub(crate) fn alloc_json_output(
        &mut self,
        output: Output<'_>,
        admission: Reservation,
    ) -> Result<Value, String> {
        let base = size_of::<HeapObject>();
        let bytes = match &output {
            Output::Scalar(_) => 0,
            Output::Text(s) => base + s.len(),
            Output::Field(Some(s)) => 2 * base + s.len(),
            Output::Field(None) => base,
        };
        // The complete output fits BEFORE its Arc allocation/heap registration.
        admission.commit(bytes)?;
        Ok(match output {
            Output::Scalar(v) => v,
            Output::Text(s) => Value::Heap(self.alloc_charged(HeapObject::String(s.into()), bytes)),
            Output::Field(None) => Value::Heap(self.alloc_charged(
                HeapObject::Data {
                    tag: 0,
                    fields: Fields::new(),
                },
                base,
            )),
            Output::Field(Some(s)) => {
                let string =
                    Value::Heap(self.alloc_charged(HeapObject::String(s.into()), base + s.len()));
                Value::Heap(self.alloc_charged(
                    HeapObject::Data {
                        tag: 1,
                        fields: smallvec::smallvec![string],
                    },
                    base,
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests;
