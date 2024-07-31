//! The public contract for repositories.

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

use async_trait::async_trait;

use crate::error::Result;

/// A repository containing test results.
#[async_trait]
pub trait Repository {
    /// Get the number of test runs this repository has stored.
    async fn count(&self) -> Result<usize>;

    /// Get the id of the latest test run, if any.
    async fn latest_id(&self) -> Result<Option<usize>> {
        let count = self.count().await?;
        if count == 0 {
            Ok(None)
        } else {
            Ok(Some(count - 1))
        }
    }
}
