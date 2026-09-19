// SPDX-License-Identifier: Apache-2.0

//! Internals of the `kittyview` binary, exposed as a library so that
//! integration tests and the `fuzz/` targets can link against them.
//! This is not a stable public API.

pub mod geometry;
pub mod kitty;
pub mod logo;
pub mod placeholder;
pub mod slideshow;
pub mod svg;
pub mod terminal;
