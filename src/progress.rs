use std::time::{Duration, Instant};

/// 連続出力の最小間隔。これより短い間隔では出力しない。
const MIN_INTERVAL: Duration = Duration::from_millis(500);
/// パーセンテージが変化しなくても、この間隔を過ぎたら「動いている」ことを
/// 示すために同じパーセンテージで再出力する(ハートビート)。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

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
        self.print_at(force, Instant::now());
    }

    fn print_at(&mut self, force: bool, now: Instant) {
        let Some(label) = self.label.clone() else {
            return;
        };

        match self.total {
            Some(total) if total > 0 => {
                let percent = (self.current.saturating_mul(100) / total).min(100);
                if !self.should_emit(force, now, Some(percent)) {
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
            Some(_) => {
                if !self.should_emit(force, now, None) {
                    return;
                }
                println!("{label}: done");
            }
            None => {
                if !self.should_emit(force, now, None) {
                    return;
                }
                println!("{label}: {}", self.current);
            }
        }
        self.last_printed_at = Some(now);
    }

    /// 出力すべきかどうかを判定する(副作用なし)。
    ///
    /// `percent` が `Some` のときはパーセンテージ表示の分岐で、値が変われば即出力、
    /// 変わらなくてもハートビート間隔を過ぎていれば出力する。`None` のときは
    /// 最小間隔さえ空いていれば出力する。
    fn should_emit(&self, force: bool, now: Instant, percent: Option<u64>) -> bool {
        if force {
            return true;
        }
        let Some(last) = self.last_printed_at else {
            return true;
        };
        let elapsed = now.duration_since(last);
        if elapsed < MIN_INTERVAL {
            return false;
        }
        match percent {
            Some(percent) => {
                self.last_percent != Some(percent) || elapsed >= HEARTBEAT_INTERVAL
            }
            None => true,
        }
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

    fn console_with(last_percent: Option<u64>, last_printed_at: Option<Instant>) -> ConsoleProgress {
        ConsoleProgress {
            label: Some("copy bytes".to_string()),
            total: Some(1000),
            current: 0,
            last_percent,
            last_printed_at,
        }
    }

    #[test]
    fn emits_when_forced_regardless_of_timing() {
        let now = Instant::now();
        let progress = console_with(Some(42), Some(now));
        assert!(progress.should_emit(true, now, Some(42)));
    }

    #[test]
    fn emits_first_time_without_prior_print() {
        let now = Instant::now();
        let progress = console_with(None, None);
        assert!(progress.should_emit(false, now, Some(0)));
    }

    #[test]
    fn suppresses_output_within_min_interval() {
        let start = Instant::now();
        let progress = console_with(Some(10), Some(start));
        // パーセンテージが変わっていても最小間隔内なら出力しない
        assert!(!progress.should_emit(false, start + Duration::from_millis(100), Some(20)));
    }

    #[test]
    fn emits_when_percent_changes_after_min_interval() {
        let start = Instant::now();
        let progress = console_with(Some(10), Some(start));
        assert!(progress.should_emit(false, start + Duration::from_millis(600), Some(11)));
    }

    #[test]
    fn suppresses_same_percent_before_heartbeat() {
        let start = Instant::now();
        let progress = console_with(Some(10), Some(start));
        // 最小間隔は過ぎているがハートビート間隔前、かつパーセンテージ据え置き
        assert!(!progress.should_emit(false, start + Duration::from_millis(600), Some(10)));
    }

    #[test]
    fn emits_same_percent_after_heartbeat_interval() {
        let start = Instant::now();
        let progress = console_with(Some(10), Some(start));
        // 1%進んでいなくても、ハートビート間隔を過ぎたら同じパーセンテージで再出力
        assert!(progress.should_emit(false, start + HEARTBEAT_INTERVAL, Some(10)));
    }

    #[test]
    fn print_at_reemits_same_percent_after_heartbeat() {
        let start = Instant::now();
        let mut progress = console_with(Some(1), Some(start));
        progress.current = 15; // 1000 のうち 15 = 1%(据え置き)

        // ハートビート間隔前は last_printed_at が更新されない
        progress.print_at(false, start + Duration::from_millis(700));
        assert_eq!(progress.last_printed_at, Some(start));

        // ハートビート間隔を過ぎると再出力され、時刻が更新される
        let later = start + HEARTBEAT_INTERVAL + Duration::from_millis(1);
        progress.print_at(false, later);
        assert_eq!(progress.last_printed_at, Some(later));
    }

    #[test]
    fn unknown_total_emits_after_min_interval() {
        let start = Instant::now();
        let mut progress = console_with(None, Some(start));
        progress.total = None;
        assert!(progress.should_emit(false, start + Duration::from_millis(600), None));
        assert!(!progress.should_emit(false, start + Duration::from_millis(100), None));
    }
}
