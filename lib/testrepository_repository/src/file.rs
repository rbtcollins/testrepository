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

use std::path::Path;

use eyre::eyre;
use fs_at::OpenOptionsWriteMode;
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

/// File repository compatible with the python Testrepository
#[derive(Debug, PartialEq)]
pub struct File {}

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
        Self::validate_format(&root).await?;
        Ok(Self {})
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

        Ok(Self {})
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

        Ok(Self {})
    }

    /// Validate the format of a repository
    ///
    /// ## Arguments
    ///
    /// * `root` - Open handle on the .testrepository directory.
    #[instrument(ret, err)]
    async fn validate_format(root: &ArcFile) -> Result<()> {
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
        opts.open_at(root, NEXT_STREAM_FILE).await.map(|_| ())
    }
}

impl Repository for File {}

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
