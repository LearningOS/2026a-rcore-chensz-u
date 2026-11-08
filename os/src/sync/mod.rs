//! Synchronization and interior mutability primitives

mod condvar;
mod mutex;
mod semaphore;

pub(crate) mod deadlock;
mod up;

pub use condvar::Condvar;
pub use mutex::{Mutex, MutexBlocking, MutexSpin};
pub use semaphore::Semaphore;
pub use up::UPSafeCell;
