//! Budgets shared by a process tree.
//!
//! A root process and every process it spawns, directly or indirectly, draw
//! from one counter per limit. Spawning children therefore cannot multiply a
//! script's fuel, memory, I/O or host-work budget: the whole tree stops when
//! the shared counter reaches its limit.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One limit and the amount of it the tree has used.
#[derive(Debug)]
pub struct SharedBudget {
    limit: u64,
    used: AtomicU64,
}

impl SharedBudget {
    pub fn new(limit: u64) -> Arc<Self> {
        Self::with_used(limit, 0)
    }

    pub fn with_used(limit: u64, used: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: AtomicU64::new(used),
        })
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used())
    }

    /// Charge exactly `amount`, or nothing. On failure, returns the amount
    /// used at the time of the attempt.
    pub fn try_charge(&self, amount: u64) -> Result<(), u64> {
        self.used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                let next = used.checked_add(amount)?;
                (next <= self.limit).then_some(next)
            })
            .map(|_| ())
    }

    /// Charge as much of `amount` as is left and return the charged amount.
    pub fn reserve_up_to(&self, amount: u64) -> u64 {
        let mut reserved = 0;
        let _ = self
            .used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                reserved = amount.min(self.limit.saturating_sub(used));
                (reserved > 0).then_some(used + reserved)
            });
        reserved
    }

    /// Return `amount` that was charged earlier.
    pub fn refund(&self, amount: u64) {
        if amount == 0 {
            return;
        }
        let _ = self
            .used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                Some(used.saturating_sub(amount))
            });
    }
}

/// One unit charged to a budget for as long as the slot lives. The live
/// process count of a tree uses this: dropping a process's VM frees its slot.
#[derive(Debug)]
pub struct BudgetSlot(Arc<SharedBudget>);

impl BudgetSlot {
    pub fn acquire(budget: &Arc<SharedBudget>) -> Result<Self, u64> {
        budget.try_charge(1)?;
        Ok(Self(budget.clone()))
    }
}

impl Drop for BudgetSlot {
    fn drop(&mut self) {
        self.0.refund(1);
    }
}

/// Default for `max_live_processes`.
pub const DEFAULT_MAX_LIVE_PROCESSES: u64 = 10_000;

/// Spawn limits for a process tree. The root process is not counted.
#[derive(Clone, Debug)]
pub struct ProcessBudgets {
    /// Spawned processes alive at once.
    pub live: Option<Arc<SharedBudget>>,
    /// Processes spawned over the tree's lifetime.
    pub spawns: Option<Arc<SharedBudget>>,
}

impl Default for ProcessBudgets {
    fn default() -> Self {
        Self {
            live: Some(SharedBudget::new(DEFAULT_MAX_LIVE_PROCESSES)),
            spawns: None,
        }
    }
}

impl ProcessBudgets {
    /// Count one more spawn. Returns the child's live-process slot, which the
    /// child keeps until its VM is dropped.
    pub fn admit_child(&self) -> Result<Option<BudgetSlot>, String> {
        let slot = match &self.live {
            Some(live) => Some(BudgetSlot::acquire(live).map_err(|_| {
                format!(
                    "spawn: process limit exceeded: max_live_processes ({})",
                    live.limit()
                )
            })?),
            None => None,
        };
        if let Some(spawns) = &self.spawns {
            spawns.try_charge(1).map_err(|_| {
                format!(
                    "spawn: process limit exceeded: max_spawns ({})",
                    spawns.limit()
                )
            })?;
        }
        Ok(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_charge_is_all_or_nothing() {
        let budget = SharedBudget::new(10);
        assert_eq!(budget.try_charge(7), Ok(()));
        assert_eq!(budget.try_charge(4), Err(7));
        assert_eq!(budget.try_charge(3), Ok(()));
        assert_eq!(budget.remaining(), 0);
        assert_eq!(budget.try_charge(u64::MAX), Err(10));
    }

    #[test]
    fn reserve_up_to_takes_what_is_left_and_refund_returns_it() {
        let budget = SharedBudget::new(10);
        assert_eq!(budget.reserve_up_to(6), 6);
        assert_eq!(budget.reserve_up_to(6), 4);
        assert_eq!(budget.reserve_up_to(6), 0);
        budget.refund(5);
        assert_eq!(budget.used(), 5);
        budget.refund(100);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn slots_are_released_on_drop() {
        let budget = SharedBudget::new(2);
        let a = BudgetSlot::acquire(&budget).unwrap();
        let _b = BudgetSlot::acquire(&budget).unwrap();
        assert!(BudgetSlot::acquire(&budget).is_err());
        drop(a);
        assert!(BudgetSlot::acquire(&budget).is_ok());
    }

    #[test]
    fn concurrent_charges_never_exceed_the_limit() {
        let budget = SharedBudget::new(1000);
        let charged: u64 = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let mut total = 0;
                        while budget.try_charge(3).is_ok() {
                            total += 3;
                        }
                        total
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert_eq!(charged, budget.used());
        assert!(charged <= 1000 && charged > 1000 - 3);
    }

    #[test]
    fn admit_child_counts_live_and_total_spawns() {
        let budgets = ProcessBudgets {
            live: Some(SharedBudget::new(1)),
            spawns: Some(SharedBudget::new(2)),
        };
        let first = budgets.admit_child().unwrap();
        let err = budgets.admit_child().unwrap_err();
        assert!(err.contains("max_live_processes (1)"), "{err}");
        drop(first);
        let _second = budgets.admit_child().unwrap();
        drop(_second);
        let err = budgets.admit_child().unwrap_err();
        assert!(err.contains("max_spawns (2)"), "{err}");
    }
}
