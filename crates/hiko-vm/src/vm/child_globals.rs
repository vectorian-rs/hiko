//! Top-level bindings seen by child processes.
//!
//! A child process runs one closure and never runs the main chunk, so the
//! globals that chunk defines (functions, constructors, values) do not exist
//! in the child's VM. At spawn, the parent hands the child a snapshot of its
//! globals as `SendableValue`s, and the child deserializes a global the first
//! time it reads it.
//!
//! Each global is serialized once in the parent and the snapshot is shared
//! (`Arc`) by every child and grandchild until the parent defines another
//! global. A global that cannot be sent (for example an `Rng` value) is
//! recorded with the reason, which the child reports if it reads that global.

use super::*;
use crate::sendable::{SendableValue, deserialize, serialize};

type SnapshotEntry = Arc<Result<SendableValue, String>>;

#[derive(Debug, Default, Clone)]
pub(crate) struct GlobalSnapshot {
    values: HashMap<Arc<str>, SnapshotEntry>,
    /// Names bound both to a builtin and, later, by the program. In a child
    /// the program's binding wins, as it does in the parent.
    rebound_builtins: Vec<Arc<str>>,
}

/// A global defined by this VM's own `SetGlobal`, with its cached serialized
/// form.
#[derive(Debug)]
pub(super) struct DefinedGlobal {
    name: Arc<str>,
    sendable: Option<SnapshotEntry>,
}

impl VM {
    /// Record that `SetGlobal` wrote `slot`, invalidating its cached form.
    pub(super) fn note_global_defined(&mut self, slot: usize, name: &str) {
        self.spawn_snapshot = None;
        self.defined_globals.insert(
            slot,
            DefinedGlobal {
                name: Arc::from(name),
                sendable: None,
            },
        );
    }

    /// The snapshot a child spawned now inherits: everything this VM
    /// inherited, plus the globals it defined itself.
    pub(super) fn refresh_spawn_snapshot(&mut self) -> Result<(), RuntimeError> {
        if self.spawn_snapshot.is_some() {
            return Ok(());
        }
        if self.defined_globals.is_empty() {
            self.spawn_snapshot = self.inherited_globals.clone();
            return Ok(());
        }
        let mut snapshot = self
            .inherited_globals
            .as_deref()
            .cloned()
            .unwrap_or_default();
        for (&slot, defined) in self.defined_globals.iter_mut() {
            let entry = match &defined.sendable {
                Some(entry) => entry.clone(),
                None => {
                    let result = serialize(self.globals[slot], &self.heap);
                    if let Ok(sendable) = &result {
                        self.heap
                            .charge_host_work(crate::sendable::host_work_for_sendable(sendable))
                            .map_err(|e| RuntimeError {
                                message: format!("spawn: {e}"),
                            })?;
                    }
                    let entry = Arc::new(result);
                    defined.sendable = Some(entry.clone());
                    entry
                }
            };
            if self
                .builtins
                .iter()
                .any(|builtin| *builtin.name == *defined.name)
            {
                snapshot.rebound_builtins.push(defined.name.clone());
            }
            snapshot.values.insert(defined.name.clone(), entry);
        }
        self.spawn_snapshot = Some(Arc::new(snapshot));
        Ok(())
    }

    /// Give a freshly created child the parent's snapshot.
    pub(super) fn inherit_globals(&mut self, snapshot: Option<Arc<GlobalSnapshot>>) {
        if let Some(snapshot) = &snapshot {
            for name in &snapshot.rebound_builtins {
                self.global_names.remove(&**name);
            }
        }
        self.inherited_globals = snapshot;
    }

    /// Load an inherited global into this VM on first use and return its slot.
    pub(super) fn load_inherited_global(&mut self, name: &str) -> Result<usize, RuntimeError> {
        let entry = self
            .inherited_globals
            .as_ref()
            .and_then(|snapshot| snapshot.values.get(name))
            .cloned()
            .ok_or_else(|| RuntimeError {
                message: format!("undefined global: {name}"),
            })?;
        let sendable = entry.as_ref().as_ref().map_err(|reason| RuntimeError {
            message: format!("top-level `{name}` cannot be used in a child process: {reason}"),
        })?;
        // The bytes a child loads here come from the parent's heap; resource
        // budgets for process boundaries (#101) should count them.
        let value = deserialize(sendable.clone(), &mut self.heap).map_err(|e| RuntimeError {
            message: e.to_string(),
        })?;
        let slot = self.global_slot(name.to_string());
        self.globals[slot] = value;
        Ok(slot)
    }
}
