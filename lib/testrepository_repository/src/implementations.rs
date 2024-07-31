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

use std::path::Path;

use async_trait::async_trait;
use url::Url;

use crate::{
    error::{Eyrify, Result},
    file::File,
    memory::{Memory, MemoryStore},
    repository,
};

/// Open a repository with options
#[derive(Debug, Default)]
pub struct OpenOptions<'a> {
    memory_store: Option<&'a MemoryStore>,
}

impl<'a> OpenOptions<'a> {
    /// Attach a memory store to permit re-opening a MemoryRepository
    pub fn with_memory_store(mut self, memory_store: &'a MemoryStore) -> Self {
        self.memory_store = Some(memory_store);
        self
    }
}

/// All the known Repository implementations.
#[derive(Debug)]
#[non_exhaustive]
pub enum Repository {
    /// In-memory repository
    Memory(Memory),
    /// Python testrepository compatible file repository
    File(File),
}

impl Repository {
    /// Open a repository at the given location
    pub async fn open(location: &Url) -> Result<Self> {
        Self::open_with(location, OpenOptions::default()).await
    }

    /// Open a repository at the given location with given options
    pub async fn open_with(location: &Url, options: OpenOptions<'_>) -> Result<Self> {
        match location.scheme() {
            "file" => {
                let path = Path::new(&location.path()[1..]);
                let path = path.canonicalize().eyre()?;
                Ok(Self::File(File::new(&path).await?))
            }
            "memory" => {
                let relpath = location.host_str().unwrap_or_default();
                let memory_store = options.memory_store.ok_or_else(|| {
                    eyre::eyre!("Memory store required to open a MemoryRepository")
                })?;
                Ok(Self::Memory(Memory::new(relpath, memory_store)?))
            }
            _ => Err(eyre::eyre!("Unknown scheme {}", location))?,
        }
    }
}

#[async_trait]
impl repository::Repository for Repository {
    async fn count(&self) -> Result<usize> {
        match self {
            Self::Memory(r) => r.count().await,
            Self::File(r) => r.count().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_test::{assert_err, assert_ok};
    use tracing_test::traced_test;
    use url::Url;

    use crate::{
        file::File,
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
        let opts = OpenOptions::default().with_memory_store(&store);
        let r = Repository::open_with(&Url::parse("memory://a").unwrap(), opts).await;
        assert_ok!(r);
    }

    #[tokio::test]
    #[traced_test]
    async fn memory_uninitialized() {
        let r = Repository::open(&Url::parse("memory://a").unwrap()).await;
        assert_err!(r);
    }

    #[tokio::test]
    #[traced_test]
    async fn file() {
        let dir = tempfile::tempdir().unwrap();
        let i = File::initialize_v2(dir.path()).await.unwrap();
        let url = Url::from_file_path(dir.path()).unwrap();
        let r = assert_ok!(Repository::open(&url).await);
        let Repository::File(r) = r else {
            panic!("unexpected repository type {:?}", r);
        };
        assert_eq!(i, r);
    }

    #[tokio::test]
    #[traced_test]
    async fn file_python_compat() {
        let dir = tempfile::tempdir().unwrap();
        #[allow(deprecated)]
        let i = File::initialize_v1(dir.path()).await.unwrap();
        let url = Url::from_file_path(dir.path()).unwrap();
        let r = assert_ok!(Repository::open(&url).await);
        let Repository::File(r) = r else {
            panic!("unexpected repository type {:?}", r);
        };
        assert_eq!(i, r);
    }

    #[tokio::test]
    #[traced_test]
    async fn file_uninitialized() {
        let dir = tempfile::tempdir().unwrap();
        let uri = Url::parse(&format!("file:///{}", dir.path().display())).unwrap();
        assert_err!(Repository::open(&uri).await);
    }
}
