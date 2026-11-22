//! **Multi Link**: a direct-play file fetched in ranges over the links the LumaTV helper offers,
//! beside the default reader and never instead of it.
//!
//! [`status`] reads what the helper says, [`object`] says when an answer is the same file,
//! [`engine`] decides, [`drive`] carries its requests to a transport, and [`reader`] is what the
//! demux thread calls. [`watch`] fetches the helper's state off that thread.

pub mod drive;
pub mod engine;
pub mod object;
pub mod reader;
pub mod status;
pub mod watch;

#[cfg(test)]
mod fakenet;
#[cfg(test)]
mod tests;
