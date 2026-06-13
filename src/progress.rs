use std::time::{Duration, Instant};

pub trait ProgressReporter {
    fn start(&mut self, label: &str, total: Option<u64>);
    fn advance(&mut self, amount: u64);
    fn finish(&mut self);
}

pub struct NoProgress;

impl ProgressReporter for NoProgress {
    fn start(&mut self, _label: &str, _total: Option<u64>) {}
    fn advance(&mut self, _amount: u64) {}
    fn finish(&mut self) {}
}

pub struct ConsoleProgress {
    label: Option<String>,
    total: Option<u64>,
    current: u64,
    last_percent: Option<u64>,
    last_printed_at: Option<Instant>,
}

impl ConsoleProgress {
    pub fn new() -> Self {
        Self {
            label: None,
            total: None,
            current: 0,
            last_percent: None,
            last_printed_at: None,
        }
    }

    fn print(&mut self, force: bool) {
        let Some(label) = self.label.clone() else {
            return;
        };
        let now = Instant::now();
        if !force
            && self
                .last_printed_at
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(500))
        {
            return;
        }

        match self.total {
            Some(total) if total > 0 => {
                let percent = (self.current.saturating_mul(100) / total).min(100);
                if !force && self.last_percent == Some(percent) {
                    return;
                }
                println!(
                    "{label}: {}/{} ({}%)",
                    self.current.min(total),
                    total,
                    percent
                );
                self.last_percent = Some(percent);
            }
            Some(_) => println!("{label}: done"),
            None => println!("{label}: {}", self.current),
        }
        self.last_printed_at = Some(now);
    }
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for ConsoleProgress {
    fn start(&mut self, label: &str, total: Option<u64>) {
        self.label = Some(label.to_string());
        self.total = total;
        self.current = 0;
        self.last_percent = None;
        self.last_printed_at = None;
        self.print(true);
    }

    fn advance(&mut self, amount: u64) {
        self.current = self.current.saturating_add(amount);
        self.print(false);
    }

    fn finish(&mut self) {
        if let Some(total) = self.total {
            self.current = total;
        }
        self.print(true);
        self.label = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingProgress {
        starts: Vec<(String, Option<u64>)>,
        advanced_by: u64,
        finishes: usize,
    }

    impl ProgressReporter for RecordingProgress {
        fn start(&mut self, label: &str, total: Option<u64>) {
            self.starts.push((label.to_string(), total));
        }

        fn advance(&mut self, amount: u64) {
            self.advanced_by += amount;
        }

        fn finish(&mut self) {
            self.finishes += 1;
        }
    }

    #[test]
    fn progress_reporter_can_record_task_lifecycle() {
        let mut progress = RecordingProgress::default();

        progress.start("copy", Some(8));
        progress.advance(3);
        progress.advance(5);
        progress.finish();

        assert_eq!(progress.starts, vec![("copy".to_string(), Some(8))]);
        assert_eq!(progress.advanced_by, 8);
        assert_eq!(progress.finishes, 1);
    }
}
