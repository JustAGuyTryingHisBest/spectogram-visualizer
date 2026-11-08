use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

/// Fan-out from one producer to any number of bounded subscribers.
/// `publish` never blocks: a full subscriber loses its oldest item and counts the loss.
pub struct Broadcast<T: Clone + Send> {
    senders: Mutex<Vec<SubscriberEnd<T>>>,
}

struct SubscriberEnd<T> {
    sender: Sender<T>,
    receiver: Receiver<T>,
    dropped: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

pub struct Subscriber<T> {
    receiver: Receiver<T>,
    dropped: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl<T: Clone + Send> Drop for Broadcast<T> {
    fn drop(&mut self) {
        for end in self.senders.lock().unwrap().iter() {
            end.closed.store(true, Ordering::Relaxed);
        }
    }
}

impl<T: Clone + Send> Default for Broadcast<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + Send> Broadcast<T> {
    pub fn new() -> Self {
        Self {
            senders: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self, capacity: usize) -> Subscriber<T> {
        let (sender, receiver) = bounded(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        self.senders.lock().unwrap().push(SubscriberEnd {
            sender,
            receiver: receiver.clone(),
            dropped: Arc::clone(&dropped),
            closed: Arc::clone(&closed),
        });
        Subscriber {
            receiver,
            dropped,
            closed,
        }
    }

    pub fn publish(&self, item: T) {
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|end| {
            // The broadcast keeps a receiver clone for drop-oldest, so disconnection is
            // detected by the subscriber having released its share of the counter.
            if Arc::strong_count(&end.dropped) == 1 {
                return false;
            }
            match end.sender.try_send(item.clone()) {
                Ok(()) => true,
                Err(TrySendError::Disconnected(_)) => false,
                Err(TrySendError::Full(rejected)) => {
                    // Drop the oldest, then retry once; a second Full means a race we also count.
                    let _ = end.receiver.try_recv();
                    end.dropped.fetch_add(1, Ordering::Relaxed);
                    match end.sender.try_send(rejected) {
                        Ok(()) => true,
                        Err(TrySendError::Disconnected(_)) => false,
                        Err(TrySendError::Full(_)) => {
                            end.dropped.fetch_add(1, Ordering::Relaxed);
                            true
                        }
                    }
                }
            }
        });
    }

    pub fn subscriber_count(&self) -> usize {
        self.senders.lock().unwrap().len()
    }
}

impl<T> Subscriber<T> {
    /// Blocks until an item arrives; `None` once the broadcast is gone and the queue is drained.
    pub fn recv(&self) -> Option<T> {
        self.receiver.recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<T> {
        self.receiver.recv_timeout(timeout).ok()
    }

    pub fn try_recv(&self) -> Option<T> {
        self.receiver.try_recv().ok()
    }

    /// True once the producer side is gone (the queue may still hold items).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Items this subscriber lost because it fell behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn slow_subscriber_drops_oldest_and_counts() {
        let broadcast = Broadcast::new();
        let subscriber = broadcast.subscribe(2);
        for item in 1..=4 {
            broadcast.publish(item);
        }
        assert_eq!(subscriber.try_recv(), Some(3));
        assert_eq!(subscriber.try_recv(), Some(4));
        assert_eq!(subscriber.try_recv(), None);
        assert_eq!(subscriber.dropped(), 2);
    }

    #[test]
    fn publish_never_blocks_on_full() {
        let broadcast = Broadcast::new();
        let _subscriber = broadcast.subscribe(1);
        let started = Instant::now();
        for item in 0..1000 {
            broadcast.publish(item);
        }
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn dropped_subscriber_is_pruned() {
        let broadcast = Broadcast::new();
        let subscriber = broadcast.subscribe(4);
        drop(subscriber);
        broadcast.publish(1);
        assert_eq!(broadcast.subscriber_count(), 0);
    }

    #[test]
    fn recv_returns_none_when_broadcast_is_gone() {
        let broadcast = Broadcast::<u32>::new();
        let subscriber = broadcast.subscribe(4);
        assert!(!subscriber.is_closed());
        drop(broadcast);
        assert!(subscriber.is_closed());
        assert_eq!(subscriber.recv(), None);
    }
}
