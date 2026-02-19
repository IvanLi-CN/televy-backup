use std::sync::atomic::{AtomicU64, Ordering};

/// Best-effort network byte counters.
///
/// These counters track raw bytes read from / written to the underlying socket(s) as observed by
/// `grammers-mtsender`'s I/O loop. They are intended for "what is the client doing right now"
/// indicators, not for protocol-level payload accounting.
#[derive(Debug, Default)]
pub struct NetStats {
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl NetStats {
    #[inline]
    pub(crate) fn inc_in(&self, n: u64) {
        self.bytes_in.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_out(&self, n: u64) {
        self.bytes_out.fetch_add(n, Ordering::Relaxed);
    }

    /// Returns `(bytes_in, bytes_out)` accumulated since the stats object was created.
    #[inline]
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes_in.load(Ordering::Relaxed),
            self.bytes_out.load(Ordering::Relaxed),
        )
    }
}

