#[macro_use]
extern crate log;

mod buffer;
pub mod command;
pub mod config;
pub mod db;
pub mod error;
pub mod memory;
pub mod sstable;
mod version;
mod wal;

pub type Result<T> = std::result::Result<T, error::KVLiteError>;

pub use db::DBImpl;
