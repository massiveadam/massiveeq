//! One owned processor waiting for the next audio callback. Publishing a new
//! processor replaces and frees an unconsumed one on the control thread.
use massiveeq_dsp::ProfileProcessor;
use std::sync::atomic::{AtomicPtr, Ordering};

#[derive(Default)]
pub struct LatestProcessor {
    pending: AtomicPtr<ProfileProcessor>,
}

impl LatestProcessor {
    /// Control thread only: superseded processors are destroyed here, never
    /// in `take` or in the audio callback.
    pub fn publish(&self, processor: Box<ProfileProcessor>) {
        let previous = self
            .pending
            .swap(Box::into_raw(processor), Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: an atomic swap transfers exclusive ownership of the old
            // pointer to exactly one caller. Every non-null pointer originated
            // from Box::into_raw and is reconstructed only after removal.
            unsafe {
                drop(Box::from_raw(previous));
            }
        }
    }

    /// Audio thread: acquire the latest processor without a lock, allocation,
    /// or destruction of a prepared processor.
    pub fn take(&self) -> Option<Box<ProfileProcessor>> {
        let pointer = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pointer.is_null() {
            None
        } else {
            // SAFETY: swap removed the pointer, so no publisher, taker, or
            // destructor can reconstruct the same owned Box again.
            Some(unsafe { Box::from_raw(pointer) })
        }
    }
}

impl Drop for LatestProcessor {
    fn drop(&mut self) {
        let pointer = *self.pending.get_mut();
        if !pointer.is_null() {
            // SAFETY: final destruction has exclusive access. NativeFilter
            // retains its Arc until its audio thread has joined, so this final
            // cleanup occurs on the control thread.
            unsafe {
                drop(Box::from_raw(pointer));
            }
        }
    }
}
