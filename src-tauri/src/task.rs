use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct Flags {
    paused: bool,
    cancelled: bool,
}

/// Cooperative control: no thread is suspended while holding filesystem/state locks.
#[derive(Default)]
pub struct ScanControl {
    flags: Mutex<Flags>,
    wake: Condvar,
}

impl ScanControl {
    pub fn checkpoint(&self) -> bool {
        let Ok(mut flags) = self.flags.lock() else {
            return false;
        };
        while flags.paused && !flags.cancelled {
            let Ok(next) = self.wake.wait(flags) else {
                return false;
            };
            flags = next;
        }
        !flags.cancelled
    }

    pub fn pause(&self) {
        if let Ok(mut flags) = self.flags.lock() {
            if !flags.cancelled {
                flags.paused = true;
            }
        }
    }

    pub fn resume(&self) {
        if let Ok(mut flags) = self.flags.lock() {
            flags.paused = false;
        }
        self.wake.notify_all();
    }

    pub fn cancel(&self) {
        if let Ok(mut flags) = self.flags.lock() {
            flags.cancelled = true;
            flags.paused = false;
        }
        self.wake.notify_all();
    }

    pub fn cancelled(&self) -> bool {
        self.flags.lock().map_or(true, |flags| flags.cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn pause_waits_resume_continues_and_cancel_wakes_paused_worker() {
        let control = Arc::new(ScanControl::default());
        let (tx, rx) = mpsc::channel();
        control.pause();
        let worker = Arc::clone(&control);
        let join = std::thread::spawn(move || {
            tx.send(worker.checkpoint()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
        control.resume();
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
        join.join().unwrap();

        control.pause();
        let worker = Arc::clone(&control);
        let (tx, rx) = mpsc::channel();
        let join = std::thread::spawn(move || {
            tx.send(worker.checkpoint()).unwrap();
        });
        control.cancel();
        assert!(!rx.recv_timeout(Duration::from_secs(1)).unwrap());
        join.join().unwrap();
        control.resume();
        assert!(!control.checkpoint());
    }
}
