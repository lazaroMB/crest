//! Use cases of crest and the port traits that adapters implement.

pub mod cost;
pub mod decide;
pub mod flows;
pub mod keymap;
pub mod ports;
pub mod privacy;
pub mod rtk;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tools;
pub mod usecases;
pub mod web;
