//! IO related helpers.
//!
//! Less part of the crate interface, and more available for reuse in testr crates, without making a noddy crate in crates.io

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

#[cfg(not(windows))]
use std::os::unix::io::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::{
    fs::File,
    io,
    os::windows::io::{FromRawHandle, IntoRawHandle},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use tokio::{
    fs::File as TokioFile,
    task::{self},
};
use tracing::instrument;

use crate::error::{Eyrify, Result};

#[cfg(windows)]
#[instrument(err)]
fn _open_dir(p: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    options.open(p)
}

#[cfg(not(windows))]
#[instrument(err)]
fn _open_dir(p: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    use libc;

    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(p)
}

/// Open a directory from a path. After this, use the fs_at OpenOptions to
/// manipulate files and directories.
pub async fn open_dir(p: &Path) -> Result<ArcFile> {
    task::spawn_blocking({
        let p = p.to_owned();
        move || _open_dir(&p)
    })
    .await
    .eyre()?
    .eyre()
    .map(TokioFile::from)
    .map(Arc::new)
}

/// Workaround for https://github.com/rbtcollins/fs_at/issues/151 -
/// testrepository does not need the enhanced capabilities that prevent Clone /
/// Send / Sync.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenOptions {
    read: bool,
    create: bool,
    create_new: bool,
    write_mode: fs_at::OpenOptionsWriteMode,
}

impl OpenOptions {
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    pub fn write(&mut self, write_mode: fs_at::OpenOptionsWriteMode) -> &mut Self {
        self.write_mode = write_mode;
        self
    }

    pub async fn open_blocking_dir<F>(&self, fd: &ArcFile, name: &Path, f: F) -> Result<ArcFile>
    where
        F: FnOnce(&fs_at::OpenOptions, &File, PathBuf) -> std::io::Result<File> + Send + 'static,
    {
        self.open_blocking_file(fd, name, f).await.map(Arc::new)
    }

    pub async fn open_blocking_file<F>(&self, fd: &ArcFile, name: &Path, f: F) -> Result<TokioFile>
    where
        F: FnOnce(&fs_at::OpenOptions, &File, PathBuf) -> std::io::Result<File> + Send + 'static,
    {
        task::spawn_blocking({
            let owned_self = *self;
            let name = PathBuf::from(name);
            let owned_fd = Arc::clone(fd);
            move || {
                // Safety: owned_fd, the tokio File, is moved into the closure and dropped after the fs_at all
                // completes, so it lives long enough. As it is within Arc, no mut ref can exist until the drop
                // completes.
                //
                #[cfg(windows)]
                let std_fd = unsafe { File::from_raw_handle(owned_fd.as_raw_handle()) };
                #[cfg(not(windows))]
                let std_fd = unsafe { File::from_raw_fd(owned_fd.as_raw_fd()) };
                let owned_opts = owned_self.into();
                let r = f(&owned_opts, &std_fd, name).map(TokioFile::from);
                // std_fd is not intended to own the fd, so we take ownership of it again.
                #[cfg(windows)]
                std_fd.into_raw_handle();
                #[cfg(not(windows))]
                std_fd.into_raw_fd();
                drop(owned_fd);
                r
            }
        })
        .await
        .eyre()?
        .eyre()
    }
}

impl From<OpenOptions> for fs_at::OpenOptions {
    fn from(s: OpenOptions) -> Self {
        let mut opts = fs_at::OpenOptions::default();
        opts.read(s.read)
            .create(s.create)
            .create_new(s.create_new)
            .write(s.write_mode);
        opts
    }
}

/// Note that the pervasive type used for directories is Arc<TokioFile>. This
/// allows dup2-free assurance that the file descriptor lifetime is greater than
/// the blocking call lifetime : if the future is dropped, the file remains open
/// until the blocking call completes.
pub type ArcFile = Arc<TokioFile>;

/// Async version of fs_at::OpenOptions.
#[async_trait]
pub trait AsyncOptionOptions {
    async fn mkdir_at<P: AsRef<Path> + Send>(&self, dir: &ArcFile, name: P) -> Result<ArcFile>;

    async fn open_dir_at<P: AsRef<Path> + Send>(&self, d: &ArcFile, p: P) -> Result<ArcFile>;

    async fn open_at<P: AsRef<Path> + Send>(&self, d: &ArcFile, p: P) -> Result<TokioFile>;
}

#[async_trait]
impl AsyncOptionOptions for OpenOptions {
    async fn mkdir_at<P: AsRef<Path> + Send>(&self, dir: &ArcFile, name: P) -> Result<ArcFile> {
        self.open_blocking_dir(dir, name.as_ref(), move |s, d, name| s.mkdir_at(d, name))
            .await
    }

    async fn open_dir_at<P: AsRef<Path> + Send>(&self, d: &ArcFile, p: P) -> Result<ArcFile> {
        self.open_blocking_dir(d, p.as_ref(), move |s, d, name| s.open_dir_at(d, name))
            .await
    }

    async fn open_at<P: AsRef<Path> + Send>(&self, d: &ArcFile, p: P) -> Result<TokioFile> {
        self.open_blocking_file(d, p.as_ref(), move |s, d, name| s.open_at(d, name))
            .await
    }
}
