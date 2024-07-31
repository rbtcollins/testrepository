//! File repository compatible with the python Testr

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

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use eyre::eyre;
use fs_at::OpenOptionsWriteMode;
use futures::future::TryFutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
use tracing::instrument;

use crate::{
    error::{Eyrify, Result},
    io::{self, ArcFile, AsyncOptionOptions, OpenOptions},
    repository::Repository,
};

pub static REPO_DIR: &str = ".testrepository";
pub static FORMAT_FILE: &str = "format";
pub static NEXT_STREAM_FILE: &str = "next-stream";

/// Shared version helper functions.
async fn count(root: &ArcFile) -> Result<usize> {
    let mut stream_content = String::new();
    let stream_content = OpenOptions::default()
        .read(true)
        .open_at(root, NEXT_STREAM_FILE)
        .map_err(|r| r.into())
        .and_then(|mut f| async move {
            f.read_to_string(&mut stream_content)
                .await
                .eyre()
                .map(|_len| stream_content)
        })
        .await?;
    stream_content.trim().parse::<usize>().eyre()
}

/// File repository compatible with the python Testrepository
#[derive(Debug)]
struct TestRepositoryV1Repo {
    root: ArcFile,
}

#[async_trait]
impl Repository for TestRepositoryV1Repo {
    async fn count(&self) -> Result<usize> {
        count(&self.root).await
    }
}

/// File repository that uses different storage...
#[derive(Debug)]
struct TestRepositoryV2Repo {
    root: ArcFile,
}
#[async_trait]
impl Repository for TestRepositoryV2Repo {
    async fn count(&self) -> Result<usize> {
        count(&self.root).await
    }
}

/// File repository version layer (could be a type parameter or a dyn instead...)
#[derive(Debug)]
enum FileRepositoryVersion {
    V1(TestRepositoryV1Repo),
    V2(TestRepositoryV2Repo),
}

#[async_trait]
impl Repository for FileRepositoryVersion {
    async fn count(&self) -> Result<usize> {
        match self {
            FileRepositoryVersion::V1(repo) => repo.count().await,
            FileRepositoryVersion::V2(repo) => repo.count().await,
        }
    }
}

impl PartialEq for FileRepositoryVersion {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FileRepositoryVersion::V1(_), FileRepositoryVersion::V1(_)) => true,
            (FileRepositoryVersion::V2(_), FileRepositoryVersion::V2(_)) => true,
            _ => false,
        }
    }
}

/// Repositories opened on File URLs
#[derive(Debug, PartialEq)]
pub struct File {
    engine: FileRepositoryVersion,
    path: PathBuf,
}

impl File {
    /// Create a new File repository instance reading from a repository located
    /// at path.
    #[instrument(ret, err)]
    pub async fn new(path: &Path) -> Result<Self> {
        tracing::debug!("Opening repository at '{}'", path.display());
        let base = io::open_dir(path).await?;
        let root = OpenOptions::default()
            .read(true)
            .open_dir_at(&base, REPO_DIR)
            .await?;
        let engine = Self::validate_format(&root).await?;
        Ok(Self {
            engine,
            path: path.to_owned(),
        })
    }

    /// Initialize a python testr compatible repository at the given path
    #[instrument(ret, err)]
    #[deprecated(since = "0.1.0", note = "Use initialize instead")]
    pub async fn initialize_v1(path: &Path) -> Result<Self> {
        let base = io::open_dir(path).await?;
        if OpenOptions::default()
            .read(true)
            .open_dir_at(&base, REPO_DIR)
            .await
            .is_ok()
        {
            Err(eyre!(
                ".testrepository already exists at '{}'",
                path.display()
            ))?
        }

        let root = OpenOptions::default().mkdir_at(&base, REPO_DIR).await?;
        let opts = *OpenOptions::default()
            .create_new(true)
            .write(OpenOptionsWriteMode::Write);
        opts.open_at(&root, FORMAT_FILE)
            .await?
            .write_all(b"1\n")
            .await
            .eyre()?;
        opts.open_at(&root, NEXT_STREAM_FILE)
            .await?
            .write_all(b"0\n")
            .await
            .eyre()?;

        let engine = Self::validate_format(&root).await?;
        Ok(Self {
            engine,
            path: path.canonicalize().eyre()?,
        })
    }

    /// Initialize a rust testr compatible repository at the given path
    #[instrument(ret, err)]
    pub async fn initialize_v2(path: &Path) -> Result<Self> {
        let base = io::open_dir(path).await?;
        if OpenOptions::default()
            .read(true)
            .open_dir_at(&base, REPO_DIR)
            .await
            .is_ok()
        {
            Err(eyre!(
                ".testrepository already exists at '{}'",
                path.display()
            ))?
        }

        let root = OpenOptions::default().mkdir_at(&base, REPO_DIR).await?;
        let opts = *OpenOptions::default()
            .create_new(true)
            .write(OpenOptionsWriteMode::Write);
        opts.open_at(&root, FORMAT_FILE)
            .await?
            .write_all(b"2\n")
            .await
            .eyre()?;
        opts.open_at(&root, NEXT_STREAM_FILE)
            .await?
            .write_all(b"0\n")
            .await
            .eyre()?;

        let engine = Self::validate_format(&root).await?;
        Ok(Self {
            engine,
            path: path.canonicalize().eyre()?,
        })
    }

    /// Validate the format of a repository
    ///
    /// ## Arguments
    ///
    /// * `root` - Open handle on the `.testrepository` directory.
    #[instrument(ret, err)]
    async fn validate_format(root: &ArcFile) -> Result<FileRepositoryVersion> {
        let mut format = String::new();
        let opts = *OpenOptions::default().read(true);
        opts.open_at(root, FORMAT_FILE)
            .await?
            .read_to_string(&mut format)
            .await
            .eyre()?;
        if format != "1\n" && format != "2\n" {
            Err(eyre!("Unknown repository format: {}", format))?
        }
        opts.open_at(root, NEXT_STREAM_FILE).await.map(|_d| {
            if format == "1\n" {
                FileRepositoryVersion::V1(TestRepositoryV1Repo { root: root.clone() })
            } else {
                FileRepositoryVersion::V2(TestRepositoryV2Repo { root: root.clone() })
            }
        })
    }
}

#[async_trait]
impl Repository for File {
    async fn count(&self) -> Result<usize> {
        self.engine.count().await
    }
}

#[cfg(test)]
mod tests {
    use tokio_test::{assert_err, assert_ok};
    use tracing_test::traced_test;

    use crate::file::File;

    #[allow(deprecated)]
    #[tokio::test]
    #[traced_test]
    async fn test_initialize_v1() {
        let dir = tempfile::tempdir().unwrap();
        assert_ok!(File::initialize_v1(dir.path()).await);
        assert_eq!(
            "1\n",
            &assert_ok!(
                tokio::fs::read_to_string(dir.path().join(".testrepository").join("format")).await
            )
        );
        assert_eq!(
            "0\n",
            &assert_ok!(
                tokio::fs::read_to_string(dir.path().join(".testrepository").join("next-stream"))
                    .await
            )
        );
        // TODO: the anydbm module?
        // What was created can be opened
        assert_ok!(File::new(dir.path()).await);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_initialize() {
        let dir = tempfile::tempdir().unwrap();
        assert_ok!(File::initialize_v2(dir.path()).await);
        assert_eq!(
            "2\n",
            &assert_ok!(
                tokio::fs::read_to_string(dir.path().join(".testrepository").join("format")).await
            )
        );
        assert_eq!(
            "0\n",
            &assert_ok!(
                tokio::fs::read_to_string(dir.path().join(".testrepository").join("next-stream"))
                    .await
            )
        );
        // TODO: the anydbm module?
        // What was created can be opened
        assert_ok!(File::new(dir.path()).await);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_already_inited() {
        let dir = tempfile::tempdir().unwrap();
        assert_ok!(File::initialize_v2(dir.path()).await);
        let e = assert_err!(File::initialize_v2(dir.path()).await);
        assert!(
            e.to_string().contains(".testrepository already exists"),
            "bad error {}",
            e.to_string()
        );
    }
}
