/*
 * ShinyProxy
 *
 * Copyright (C) 2016-2026 Open Analytics
 *
 * ===========================================================================
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the Apache License as published by
 * The Apache Software Foundation, either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * Apache License for more details.
 *
 * You should have received a copy of the Apache License
 * along with this program.  If not, see <http://www.apache.org/licenses/>
 */

//! Allocation of the host ports that container ports are published on.
//!
//! Mirrors `MemoryPortAllocator`: ports are handed out from `proxy.docker.port-range-start` upwards and
//! released per proxy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// Error when no port is available.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "Cannot create container: all allocated ports are currently in use. Please try again later or \
     contact an administrator."
)]
pub struct NoPortAvailable;

/// Where the allocated ports are kept.
///
/// The memory registry is used by a single server; the Redis registry (`store::redis`) lets the servers of
/// a realm allocate ports without ever handing out the same one twice, as `RedisPortAllocator` does.
pub trait PortRegistry: Send + Sync + std::fmt::Debug {
    /// Every allocated port, per owner.
    fn allocated(&self) -> BTreeMap<String, BTreeSet<u16>>;

    /// Adds a port to an owner; `false` when somebody else took it in the meantime (then the caller
    /// retries with a fresh view).
    fn add(&self, owner_id: &str, port: u16) -> bool;

    /// Releases every port of an owner.
    fn release(&self, owner_id: &str);

    /// The ports of an owner.
    fn owned(&self, owner_id: &str) -> BTreeSet<u16> {
        self.allocated().remove(owner_id).unwrap_or_default()
    }
}

/// Keeps the allocated ports in memory (one server).
#[derive(Debug, Default)]
pub struct MemoryPortRegistry {
    state: Mutex<BTreeMap<String, BTreeSet<u16>>>,
}

impl PortRegistry for MemoryPortRegistry {
    fn allocated(&self) -> BTreeMap<String, BTreeSet<u16>> {
        self.state.lock().expect("not poisoned").clone()
    }

    fn add(&self, owner_id: &str, port: u16) -> bool {
        let mut state = self.state.lock().expect("not poisoned");
        if state.values().any(|ports| ports.contains(&port)) {
            return false;
        }
        state.entry(owner_id.to_string()).or_default().insert(port);
        true
    }

    fn release(&self, owner_id: &str) {
        self.state.lock().expect("not poisoned").remove(owner_id);
    }
}

/// Hands out host ports.
#[derive(Debug)]
pub struct PortAllocator {
    registry: Box<dyn PortRegistry>,
    range_from: u16,
    range_to: Option<u16>,
}

impl PortAllocator {
    /// Creates an allocator for the given range (`range_to` `None` means "until the end").
    pub fn new(range_from: u16, range_to: Option<u16>) -> Self {
        PortAllocator::with_registry(
            Box::new(MemoryPortRegistry::default()),
            range_from,
            range_to,
        )
    }

    /// Creates an allocator that keeps its state in the given registry.
    pub fn with_registry(
        registry: Box<dyn PortRegistry>,
        range_from: u16,
        range_to: Option<u16>,
    ) -> Self {
        PortAllocator {
            registry,
            range_from,
            range_to,
        }
    }

    /// Allocates a free port for the given owner.
    ///
    /// With a shared registry another server may take the port between reading and claiming it, so the
    /// search is retried (the Java implementation does the same with `WATCH`/`MULTI`).
    pub fn allocate(&self, owner_id: &str) -> Result<u16, NoPortAvailable> {
        for _ in 0..100 {
            let allocated: BTreeSet<u16> = self
                .registry
                .allocated()
                .values()
                .flatten()
                .copied()
                .collect();

            let mut candidate = self.range_from;
            while allocated.contains(&candidate) {
                candidate = candidate.checked_add(1).ok_or(NoPortAvailable)?;
            }
            if let Some(range_to) = self.range_to {
                if candidate > range_to {
                    return Err(NoPortAvailable);
                }
            }
            if self.registry.add(owner_id, candidate) {
                return Ok(candidate);
            }
        }
        Err(NoPortAvailable)
    }

    /// Registers a port that is already in use (app recovery).
    pub fn add_existing_port(&self, owner_id: &str, port: u16) {
        self.registry.add(owner_id, port);
    }

    /// Releases all ports of an owner.
    pub fn release(&self, owner_id: &str) {
        self.registry.release(owner_id);
    }

    /// The ports owned by the given owner.
    pub fn owned_ports(&self, owner_id: &str) -> BTreeSet<u16> {
        self.registry.owned(owner_id)
    }

    /// Total number of allocated ports.
    pub fn allocated_count(&self) -> usize {
        self.registry.allocated().values().map(BTreeSet::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_out_ports_from_the_start_of_the_range() {
        let allocator = PortAllocator::new(20000, None);
        assert_eq!(allocator.allocate("a").unwrap(), 20000);
        assert_eq!(allocator.allocate("a").unwrap(), 20001);
        assert_eq!(allocator.allocate("b").unwrap(), 20002);
        assert_eq!(allocator.owned_ports("a"), BTreeSet::from([20000, 20001]));
        assert_eq!(allocator.allocated_count(), 3);
    }

    #[test]
    fn reuses_released_ports() {
        let allocator = PortAllocator::new(20000, None);
        allocator.allocate("a").unwrap();
        allocator.allocate("a").unwrap();
        allocator.release("a");
        assert_eq!(allocator.allocate("b").unwrap(), 20000);
        assert!(allocator.owned_ports("a").is_empty());
    }

    #[test]
    fn respects_the_end_of_the_range() {
        let allocator = PortAllocator::new(20000, Some(20001));
        assert_eq!(allocator.allocate("a").unwrap(), 20000);
        assert_eq!(allocator.allocate("b").unwrap(), 20001);
        assert_eq!(allocator.allocate("c").unwrap_err(), NoPortAvailable);
    }

    #[test]
    fn registers_existing_ports() {
        let allocator = PortAllocator::new(20000, None);
        allocator.add_existing_port("recovered", 20000);
        assert_eq!(allocator.allocate("new").unwrap(), 20001);
        assert_eq!(allocator.owned_ports("recovered"), BTreeSet::from([20000]));
    }
}
