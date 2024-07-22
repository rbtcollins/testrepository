//! Abstraction over different implementations

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

use url::Url;

use crate::{
    error::Result,
    memory::{Memory, MemoryStore},
};

/// Open a repository with options
#[derive(Debug, Default)]
pub struct OpenOptions {
    memory_store: MemoryStore,
}

impl OpenOptions {
    /// Attach a memory store to permit re-opening a MemoryRepository
    pub fn with_memory_store(mut self, memory_store: MemoryStore) -> Self {
        self.memory_store = memory_store;
        self
    }
}

/// All the known Repository implementations.
#[derive(Debug)]
#[non_exhaustive]
pub enum Repository {
    /// In-memory repository
    Memory(Memory),
}

impl Repository {
    /// Open a repository at the given location
    pub async fn open(location: &Url) -> Result<Self> {
        Self::open_with(location, OpenOptions::default()).await
    }

    /// Open a repository at the given location with given options
    pub async fn open_with(location: &Url, options: OpenOptions) -> Result<Self> {
        match location.scheme() {
            "memory" => {
                let relpath = location.host_str().unwrap_or_default();
                Ok(Self::Memory(Memory::new(relpath, options.memory_store)?))
            }
            _ => Err(eyre::eyre!("Unknown scheme {}", location))?,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_test::{assert_err, assert_ok};
    use tracing_test::traced_test;
    use url::Url;

    use crate::{
        implementations::{OpenOptions, Repository},
        memory::MemoryStore,
    };

    #[tokio::test]
    async fn unknown_scheme() {
        assert_err!(Repository::open(&Url::parse("foo:").unwrap()).await);
    }

    #[tokio::test]
    #[traced_test]
    async fn memory() {
        let mut store = MemoryStore::default();
        store.initialize("a");
        let opts = OpenOptions::default().with_memory_store(store);
        let r = Repository::open_with(&Url::parse("memory://a").unwrap(), opts).await;
        assert_ok!(r);
    }

    #[tokio::test]
    #[traced_test]
    async fn memory_uninitialized() {
        let r = Repository::open(&Url::parse("memory://a").unwrap()).await;
        assert_err!(r);
    }
}
