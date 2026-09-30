//! Past logs are compressed at zstd's most (level 19 with long-distance
//! matching), which takes a core for a while; nobody waits for them. So they
//! run on a thread of their own in Windows' background mode (lowest CPU and
//! disk priority), where the game and everything else come first.

/// Runs `f` on a new low-priority thread; the answer arrives on the receiver
/// (dropped if `f` panics).
pub fn spawn_low<T, F>(f: F) -> tokio::sync::oneshot::Receiver<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("backlog-compress".into())
        .spawn(move || {
            lower_this_thread();
            let _ = tx.send(f());
        });
    if spawned.is_err() {
        log::warn!("couldn't start a thread to compress a past log");
    }
    rx
}

/// Puts the calling thread in background mode, best effort.
#[cfg(windows)]
pub fn lower_this_thread() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    // Background mode lowers the thread's disk and memory priority too, not
    // just its CPU priority. Best effort: if Windows refuses, it still runs.
    // SAFETY: the pseudo-handle of the calling thread, always valid.
    let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
    if ok == 0 {
        log::info!("couldn't lower the compression thread's priority");
    }
}

/// Puts the calling thread in background mode, best effort.
#[cfg(not(windows))]
pub fn lower_this_thread() {
    // TODO(macOS): a background QoS class, with the macOS build.
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn runs_the_work_and_hands_back_its_answer() {
        let rx = super::spawn_low(|| 6 * 7);
        assert_eq!(rx.await.unwrap(), 42);
    }
}
