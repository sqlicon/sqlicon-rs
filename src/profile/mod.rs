//! Informix connection profile management compatible with C sqlicon (~/.config/sqlicon/profiles.conf).

pub mod crypto;
pub mod store;

pub use store::{Profile, ProfileStore};
