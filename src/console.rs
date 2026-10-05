//! コンソールの簡易編集モード(QuickEdit)を実行中だけ無効にする。
//!
//! conhost では簡易編集モードで文字を選択している間、stdout への書き込みが
//! ブロックされる。進捗を `println!` で出しているため、ウィンドウをクリックした
//! だけでバックアップ処理全体が止まってしまう。

#[cfg(windows)]
mod imp {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::winbase::STD_INPUT_HANDLE;
    use winapi::um::wincon::{ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE};
    use winapi::um::winnt::HANDLE;

    /// 簡易編集モードを外した入力モード。`ENABLE_EXTENDED_FLAGS` を立てないと
    /// `ENABLE_QUICK_EDIT_MODE` の変更が反映されない。
    pub(super) fn without_quick_edit(mode: DWORD) -> DWORD {
        (mode | ENABLE_EXTENDED_FLAGS) & !ENABLE_QUICK_EDIT_MODE
    }

    /// drop 時に元の入力モードへ戻す。
    pub struct QuickEditGuard {
        restore: Option<(HANDLE, DWORD)>,
    }

    impl QuickEditGuard {
        /// コンソールが無い(リダイレクト・タスクスケジューラ等)場合は何もしない。
        pub fn disable() -> Self {
            unsafe {
                let handle = GetStdHandle(STD_INPUT_HANDLE);
                let mut mode: DWORD = 0;
                if handle.is_null()
                    || handle == INVALID_HANDLE_VALUE
                    || GetConsoleMode(handle, &mut mode) == 0
                    || SetConsoleMode(handle, without_quick_edit(mode)) == 0
                {
                    return Self { restore: None };
                }
                Self {
                    restore: Some((handle, mode)),
                }
            }
        }
    }

    impl Drop for QuickEditGuard {
        fn drop(&mut self) {
            if let Some((handle, mode)) = self.restore {
                unsafe {
                    SetConsoleMode(handle, mode);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn clears_quick_edit_and_sets_extended_flags() {
            let mode = ENABLE_QUICK_EDIT_MODE | 0x0001;
            let new_mode = without_quick_edit(mode);
            assert_eq!(new_mode & ENABLE_QUICK_EDIT_MODE, 0);
            assert_ne!(new_mode & ENABLE_EXTENDED_FLAGS, 0);
            assert_ne!(new_mode & 0x0001, 0); // 他のフラグは保持
        }
    }
}

#[cfg(windows)]
pub use imp::QuickEditGuard;

#[cfg(not(windows))]
pub struct QuickEditGuard;

#[cfg(not(windows))]
impl QuickEditGuard {
    pub fn disable() -> Self {
        Self
    }
}
