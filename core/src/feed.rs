//! Log messages handed over by the server's log hook (`native_log_line`, on the log's writer
//! thread) for the console loop. Messages logged before the bridge starts are kept too, so the
//! console channel gets this run's log from its first line, as far as the queue holds it.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::console::LogLine;

pub struct Feed {
    lines: Mutex<VecDeque<LogLine>>,
    cap: usize,
    closed: AtomicBool,
}

impl Feed {
    pub const fn new(cap: usize) -> Feed {
        Feed { lines: Mutex::new(VecDeque::new()), cap, closed: AtomicBool::new(false) }
    }

    /// One logged message: each of its lines (a multi-line message's further lines too) with
    /// the message's level. The oldest lines are dropped past the cap.
    pub fn push(&self, level: &str, message: &str) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let mut q = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        for line in message.split('\n') {
            q.push_back(LogLine { level: level.to_string(), message: line.trim_end_matches('\r').to_string() });
        }
        while q.len() > self.cap {
            q.pop_front();
        }
    }

    /// Every line pushed since the last call, oldest first.
    pub fn drain(&self) -> Vec<LogLine> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }

    /// Nothing reads the feed (no console channel, or the bridge stopped): drop what it holds
    /// and keep nothing from now on.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_line_messages_keep_their_level_and_the_cap_drops_the_oldest() {
        let f = Feed::new(3);
        f.push("INFO", "Done (1.2s)!");
        assert_eq!(f.drain(), vec![LogLine { level: "INFO".into(), message: "Done (1.2s)!".into() }]);
        assert!(f.drain().is_empty());
        f.push("ERROR", "boom\r\n\tat a.b(C.java:1)\n\tat d.e(F.java:2)");
        f.push("WARN", "last");
        let lines = f.drain();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], LogLine { level: "ERROR".into(), message: "\tat a.b(C.java:1)".into() });
        assert_eq!(lines[1].level, "ERROR");
        assert_eq!(lines[2], LogLine { level: "WARN".into(), message: "last".into() });
    }

    #[test]
    fn a_closed_feed_keeps_nothing() {
        let f = Feed::new(10);
        f.push("INFO", "before");
        f.close();
        f.push("INFO", "after");
        assert!(f.drain().is_empty());
    }
}
