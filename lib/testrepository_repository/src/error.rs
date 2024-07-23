//! Public error types for the repository module.

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

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Errors that shouldn't need matching on
    #[error("{0}")]
    Eyre(#[from] eyre::Report),
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Eyrify {
    type Value;
    fn eyre(self) -> Result<Self::Value>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> Eyrify for std::result::Result<T, E> {
    type Value = T;
    fn eyre(self) -> Result<T> {
        self.map_err(|e| eyre::Report::from(e).into())
    }
}
