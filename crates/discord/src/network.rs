//! Waiting for the network to be back, as the client does before it retries
//! an upload or a request (`awaitOnline`, 209489): it asks `navigator.onLine`
//! and, if that is false, waits for the `online` event. Here the machine's
//! network monitor says the same.

use gio::prelude::*;

/// Wait until `online` holds, looking again each time `changed` says
/// something did. Gives up waiting if nothing can change any more.
async fn until_online(mut online: impl FnMut() -> bool, changed: &async_channel::Receiver<()>) {
    while !online() {
        if changed.recv().await.is_err() {
            return;
        }
    }
}

/// Stops listening to the monitor when dropped, also when the wait is.
struct Listening(gio::NetworkMonitor, Option<glib::SignalHandlerId>);

impl Drop for Listening {
    fn drop(&mut self) {
        if let Some(id) = self.1.take() {
            self.0.disconnect(id);
        }
    }
}

/// Returns at once while the network is available, else when it is again.
/// Dropping the future stops waiting.
pub async fn await_online() {
    let monitor = gio::NetworkMonitor::default();
    if monitor.is_network_available() {
        return;
    }
    let (tx, changes) = async_channel::unbounded();
    let id = monitor.connect_network_available_notify(move |_| {
        let _ = tx.try_send(());
    });
    let _listening = Listening(monitor.clone(), Some(id));
    until_online(|| monitor.is_network_available(), &changes).await;
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn run(future: impl Future<Output = ()>) {
        glib::MainContext::new().block_on(future);
    }

    #[test]
    fn online_at_once_waits_for_nothing() {
        let (_tx, changes) = async_channel::unbounded();
        run(until_online(|| true, &changes));
    }

    #[test]
    fn offline_waits_for_a_change_that_brings_it_back() {
        let (tx, changes) = async_channel::unbounded();
        let looked = Cell::new(0);
        // Two changes that leave it offline, a third that brings it back.
        for _ in 0..3 {
            tx.try_send(()).unwrap();
        }
        run(until_online(
            || {
                looked.set(looked.get() + 1);
                looked.get() == 4
            },
            &changes,
        ));
        assert_eq!(looked.get(), 4, "it looked at the start and after each change, no more");
    }

    #[test]
    fn nothing_left_to_change_ends_the_wait() {
        let (tx, changes) = async_channel::unbounded::<()>();
        drop(tx);
        run(until_online(|| false, &changes));
    }
}
