use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 連続出力の最小間隔。これより短い間隔では出力しない。
const MIN_INTERVAL: Duration = Duration::from_millis(500);
/// パーセンテージが変化しなくても、この間隔を過ぎたら「動いている」ことを
/// 示すために同じパーセンテージで再出力する(ハートビート)。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
/// 進捗スレッドが状態を見に行く間隔。
const TICK_INTERVAL: Duration = Duration::from_millis(500);

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

/// 進捗をコンソールに出力する。
///
/// 処理スレッドは `advance()` で状態を更新するだけでなく、専用のスレッドが
/// 定期的に状態を見て出力する。そのため1つの巨大ファイルの圧縮などで
/// `advance()` が長時間呼ばれなくても、ハートビートは途切れない。
pub struct ConsoleProgress {
    state: Arc<Mutex<State>>,
    stop: Option<Sender<()>>,
    ticker: Option<JoinHandle<()>>,
}

impl ConsoleProgress {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let (stop, stopped) = mpsc::channel::<()>();
        let ticker_state = Arc::clone(&state);
        let ticker = thread::spawn(move || {
            // 送信側が drop されると Disconnected になりループを抜ける
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(TICK_INTERVAL) {
                lock(&ticker_state).print(false);
            }
        });
        Self {
            state,
            stop: Some(stop),
            ticker: Some(ticker),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // 片方のスレッドがパニックしても、進捗表示のせいで処理全体を止めない
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ConsoleProgress {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(ticker) = self.ticker.take() {
            ticker.join().ok();
        }
    }
}

impl ProgressReporter for ConsoleProgress {
    fn start(&mut self, label: &str, total: Option<u64>) {
        let mut state = self.state();
        state.label = Some(label.to_string());
        state.total = total;
        state.current = 0;
        state.last_percent = None;
        state.last_printed_at = None;
        state.print(true);
    }

    fn advance(&mut self, amount: u64) {
        let mut state = self.state();
        state.current = state.current.saturating_add(amount);
        state.print(false);
    }

    fn finish(&mut self) {
        let mut state = self.state();
        if let Some(total) = state.total {
            state.current = total;
        }
        state.print(true);
        state.label = None;
    }
}

#[derive(Default)]
struct State {
    label: Option<String>,
    total: Option<u64>,
    current: u64,
    last_percent: Option<u64>,
    last_printed_at: Option<Instant>,
}

impl State {
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

    fn console_with(last_percent: Option<u64>, last_printed_at: Option<Instant>) -> State {
        State {
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
    fn ticker_emits_heartbeat_without_advance() {
        let mut progress = ConsoleProgress::new();
        progress.start("copy bytes", Some(1000));
        let started = progress.state().last_printed_at.expect("start should print");

        // advance() を一切呼ばない(処理スレッドが1つの I/O で詰まっている状態)
        thread::sleep(HEARTBEAT_INTERVAL + TICK_INTERVAL * 2);

        let last = progress.state().last_printed_at.expect("should have printed");
        assert!(last > started, "ticker thread should re-emit progress");
    }

    #[test]
    fn ticker_is_silent_after_finish() {
        let mut progress = ConsoleProgress::new();
        progress.start("copy bytes", Some(1000));
        progress.finish();
        let finished = progress.state().last_printed_at;

        thread::sleep(HEARTBEAT_INTERVAL + TICK_INTERVAL * 2);

        assert_eq!(progress.state().last_printed_at, finished);
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
