//! In memory repository primarily for testing.

// Copyright (c) 2009,2024 Testrepository Contributors
//
// Licensed under either the Apache License, Version 2.0 or the BSD 3-clause
// license at the users choice. A copy of both licenses are available in the
// project source as Apache-2.0 and BSD. You may not use this file except in
// compliance with one of these two licences.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under these licenses is distributed on an "AS IS" BASIS, WITHOUT
// WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.  See the
// license you chose for the specific language governing permissions and
// limitations under that license.

use std::fmt::{Debug, Display};

use tracing::instrument;

use crate::{error::Result, repository::Repository};

/// Process memory backed store for MemoryRepository.
#[derive(Default)]
pub struct MemoryStore {
    repos: std::collections::HashMap<String, ()>,
}

impl MemoryStore {
    /// Create a new Memory Repository in the store.
    pub fn initialize(&mut self, name: &str) {
        self.repos.insert(name.into(), ());
    }
}

impl Debug for MemoryStore {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("MemoryStore")
            .field("repos", &self.repos.keys())
            .finish()
    }
}

/// In memory repository primarily for testing. This offers the full feature set of a repository including closing and
/// reopening, but backed entirely by process memory.
#[derive(Debug)]
pub struct Memory {
    store: MemoryStore,
    path: String,
}

impl Display for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(Memory repository at {} in store {:?})",
            self.path, self.store
        )
    }
}

impl Memory {
    /// Create a new Memory repository with the given store.
    #[instrument(ret(Display), err)]
    pub fn new(path: &str, store: MemoryStore) -> Result<Self> {
        if store.repos.contains_key(path) {
            Ok(Self {
                store,
                path: path.into(),
            })
        } else {
            Err(eyre::eyre!("Repository not found at {}", path))?
        }
    }
}

impl Repository for Memory {}
