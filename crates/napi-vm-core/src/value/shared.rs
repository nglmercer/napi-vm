//! Transferable shared data blocks. Guest wrappers and their realms remain Rc
//! owned on the VM thread; only this allocation and native wait signals travel.
use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Allocation {
    data: NonNull<u8>,
    length: AtomicUsize,
    maximum_length: usize,
    growable: bool,
    layout: Layout,
    access: Mutex<()>,
    waiters: Mutex<VecDeque<(usize, Arc<WaitSignal>)>>,
}

// SAFETY: this allocation owns its bytes until the final Arc drops. Safe VM
// reads/writes hold `access`, including mixed-width atomic accesses. Exposing a
// raw pointer does not permit safe Rust to dereference it. No guest values,
// external finalizers, Rc handles or interpreter state reside here.
unsafe impl Send for Allocation {}
unsafe impl Sync for Allocation {}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: these are the exact allocation pointer and layout.
        unsafe { dealloc(self.data.as_ptr(), self.layout) };
    }
}

#[derive(Default)]
struct WaitSignal {
    owner_wake: Mutex<Option<std::sync::Weak<crate::host::WakeSignal>>>,
    notified: Mutex<bool>,
    wake: Condvar,
}

impl std::fmt::Debug for WaitSignal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WaitSignal")
            .field("notified", &self.notified)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedWaitResult {
    Cancelled,
    NotEqual,
    TimedOut,
    Ok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedGrowError {
    NotGrowable,
    InvalidLength,
}

/// Thread-safe, owned SAB backing memory, without guest object identity.
#[derive(Debug, Clone)]
pub struct SharedMemory(Arc<Allocation>);

impl SharedMemory {
    pub(crate) fn zeroed(length: usize) -> Option<Self> {
        Self::zeroed_with_maximum(length, None)
    }

    pub(crate) fn zeroed_with_maximum(length: usize, maximum: Option<usize>) -> Option<Self> {
        let maximum_length = maximum.unwrap_or(length);
        if maximum_length < length {
            return None;
        }
        let allocation_length = maximum_length.max(1).checked_add(7)? & !7;
        let layout = Layout::from_size_align(allocation_length, 8).ok()?;
        // SAFETY: layout is nonzero; Allocation owns its deallocation.
        let data = NonNull::new(unsafe { alloc_zeroed(layout) })?;
        Some(Self(Arc::new(Allocation {
            data,
            length: AtomicUsize::new(length),
            maximum_length,
            growable: maximum.is_some(),
            layout,
            access: Mutex::new(()),
            waiters: Mutex::new(VecDeque::new()),
        })))
    }

    pub fn len(&self) -> usize {
        self.0.length.load(Ordering::SeqCst)
    }

    pub fn maximum_length(&self) -> usize {
        self.0.maximum_length
    }

    pub fn is_growable(&self) -> bool {
        self.0.growable
    }

    /// Publish growth without relocating memory shared with another owner.
    /// Reserved bytes are zeroed at allocation; every access uses the same
    /// lock, and only the length and native data block cross threads.
    pub fn grow(&self, length: usize) -> Result<(), SharedGrowError> {
        if !self.is_growable() {
            return Err(SharedGrowError::NotGrowable);
        }
        let _access = self.access();
        if length < self.len() || length > self.maximum_length() {
            return Err(SharedGrowError::InvalidLength);
        }
        self.0.length.store(length, Ordering::SeqCst);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    pub(crate) fn data(&self) -> NonNull<u8> {
        self.0.data
    }

    pub(crate) fn access(&self) -> MutexGuard<'_, ()> {
        self.0.access.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn register_wait(
        &self,
        offset: usize,
        timeout_ms: f64,
        equal: impl FnOnce() -> bool,
    ) -> Result<SharedWaitRegistration, SharedWaitResult> {
        let signal = Arc::new(WaitSignal::default());
        let mut waiters = self.0.waiters.lock().unwrap_or_else(|e| e.into_inner());
        if !equal() {
            return Err(SharedWaitResult::NotEqual);
        }
        if timeout_ms == 0.0 {
            return Err(SharedWaitResult::TimedOut);
        }
        waiters.push_back((offset, signal.clone()));
        Ok(SharedWaitRegistration {
            memory: self.clone(),
            signal,
        })
    }

    /// Equality and registration share notify's lock, preventing lost wakeups.
    pub(crate) fn wait(
        &self,
        offset: usize,
        timeout_ms: f64,
        equal: impl FnOnce() -> bool,
        cancelled: impl Fn() -> bool,
    ) -> SharedWaitResult {
        let registration = match self.register_wait(offset, timeout_ms, equal) {
            Ok(registration) => registration,
            Err(result) => return result,
        };
        let deadline = if timeout_ms.is_finite() {
            Duration::try_from_secs_f64(timeout_ms / 1000.0)
                .ok()
                .and_then(|duration| Instant::now().checked_add(duration))
        } else {
            None
        };
        let mut notified = registration
            .signal
            .notified
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while !*notified && !cancelled() {
            let remaining = deadline.map_or(Duration::from_millis(10), |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10))
            });
            if remaining.is_zero() {
                break;
            }
            notified = registration
                .signal
                .wake
                .wait_timeout(notified, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(notified);
        if cancelled() {
            SharedWaitResult::Cancelled
        } else {
            registration.complete()
        }
    }

    /// Wake at most count blocking waiters at this byte offset, in FIFO order.
    pub fn notify(&self, offset: usize, count: usize) -> usize {
        let mut waiters = self.0.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let mut notified = 0;
        waiters.retain(|(location, signal)| {
            if *location != offset || notified == count {
                return true;
            }
            *signal.notified.lock().unwrap_or_else(|e| e.into_inner()) = true;
            signal.wake.notify_one();
            if let Some(owner) = signal
                .owner_wake
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
            {
                owner.fire();
            }
            notified += 1;
            false
        });
        notified
    }
}

/// Native-only waiter ownership. The scheduler retains the guest promise and
/// polls this signal on its owner thread; notify never touches that promise.
#[derive(Debug)]
pub struct SharedWaitRegistration {
    memory: SharedMemory,
    signal: Arc<WaitSignal>,
}

impl SharedWaitRegistration {
    /// Waking an owner is native signalling; notification never enters its VM.
    pub fn set_owner_wake(&self, wake: &Arc<crate::host::WakeSignal>) {
        *self
            .signal
            .owner_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(wake));
        // Notify may win before the scheduler installs its wake latch.
        if self.is_notified() {
            wake.fire();
        }
    }

    pub fn is_notified(&self) -> bool {
        *self
            .signal
            .notified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Arbitrate timeout and notification using the shared FIFO list lock.
    pub fn complete(&self) -> SharedWaitResult {
        let mut waiters = self
            .memory
            .0
            .waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        waiters.retain(|(_, entry)| !Arc::ptr_eq(entry, &self.signal));
        if self.is_notified() {
            SharedWaitResult::Ok
        } else {
            SharedWaitResult::TimedOut
        }
    }
}

impl Drop for SharedWaitRegistration {
    fn drop(&mut self) {
        self.complete();
    }
}
