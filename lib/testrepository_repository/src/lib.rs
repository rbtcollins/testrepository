//! Storage of test results.
//!
//! A Repository provides storage and indexing of results.
//!
//! The Repository trait defines the contract for Repository implementations.
//!
//! The file module is compatible with the Python testrepository implementation.
//! The memory module is a simple in-memory implementation useful for testing or
//! in-process storage for small workloads.
//!
//! Repositories are identified by their URL, and new ones are made by calling
//! the initialize function in the appropriate repository module.

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

pub mod error;
pub mod file;
pub mod implementations;
pub mod io;
pub mod memory;
pub mod repository;
