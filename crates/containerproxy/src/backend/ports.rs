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

/// Hands out host ports.
#[derive(Debug)]
pub struct PortAllocator {
    state: Mutex<State>,
    range_from: u16,
    range_to: Option<u16>,
}

#[derive(Debug, Default)]
struct State {
    /// Owner (proxy id) to the ports it owns.
    ports: BTreeMap<String, BTreeSet<u16>>,
}

impl PortAllocator {
    /// Creates an allocator for the given range (`range_to` `None` means "until the end").
    pub fn new(range_from: u16, range_to: Option<u16>) -> Self {
        PortAllocator {
            state: Mutex::new(State::default()),
            range_from,
            range_to,
        }
    }

    /// Allocates a free port for the given owner.
    pub fn allocate(&self, owner_id: &str) -> Result<u16, NoPortAvailable> {
        let mut state = self.state.lock().expect("port allocator is not poisoned");
        let allocated: BTreeSet<u16> = state.ports.values().flatten().copied().collect();

        let mut candidate = self.range_from;
        while allocated.contains(&candidate) {
            candidate = candidate.checked_add(1).ok_or(NoPortAvailable)?;
        }
        if let Some(range_to) = self.range_to {
            if candidate > range_to {
                return Err(NoPortAvailable);
            }
        }
        state
            .ports
            .entry(owner_id.to_string())
            .or_default()
            .insert(candidate);
        Ok(candidate)
    }

    /// Registers a port that is already in use (app recovery).
    pub fn add_existing_port(&self, owner_id: &str, port: u16) {
        let mut state = self.state.lock().expect("port allocator is not poisoned");
        state
            .ports
            .entry(owner_id.to_string())
            .or_default()
            .insert(port);
    }

    /// Releases all ports of an owner.
    pub fn release(&self, owner_id: &str) {
        let mut state = self.state.lock().expect("port allocator is not poisoned");
        state.ports.remove(owner_id);
    }

    /// The ports owned by the given owner.
    pub fn owned_ports(&self, owner_id: &str) -> BTreeSet<u16> {
        let state = self.state.lock().expect("port allocator is not poisoned");
        state.ports.get(owner_id).cloned().unwrap_or_default()
    }

    /// Total number of allocated ports.
    pub fn allocated_count(&self) -> usize {
        let state = self.state.lock().expect("port allocator is not poisoned");
        state.ports.values().map(BTreeSet::len).sum()
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
