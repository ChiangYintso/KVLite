#![feature(backtrace)]
#![feature(map_first_last)]

#[macro_use]
extern crate log;

mod bloom;
pub mod cache;
pub mod collections;
mod compact;
pub mod db;
mod env;
pub mod error;
mod hash;
pub mod ioutils;
pub mod memory;
pub mod sstable;
pub mod wal;

pub type Result<T> = std::result::Result<T, error::KVLiteError>;
