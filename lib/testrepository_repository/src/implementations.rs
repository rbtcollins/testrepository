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

use crate::error::Result;

/// All the known Repository implementations.
#[derive(Debug)]
#[non_exhaustive]
pub enum Repository {}

impl Repository {
    pub async fn open(location: &Url) -> Result<Self> {
        match location.scheme() {
            _ => Err(eyre::eyre!("Unknown scheme {}", location))?,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_test::assert_err;
    use url::Url;

    use crate::implementations::Repository;

    #[tokio::test]
    async fn unknown_scheme() {
        assert_err!(Repository::open(&Url::parse("foo:").unwrap()).await);
    }
}
