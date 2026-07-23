# VSS手動テスト手順書

`src/vss.rs` によるVSSシャドウコピー経由の読み取りは、管理者権限と実際のWindows環境が必要なため自動テストの対象外([ADR-0001](adr/0001-vss-via-winapi-com-bindings.md))。リリース前・変更時はこの手順で実機確認する。

## 前提

- Windows実機(NTFSボリューム、C:とD:など2つ以上のドライブがあると望ましいが、C:のみでも可)
- テスト用の使い捨てディレクトリ・ファイルを使う(本番データは使わない)
- `cargo build --release` でビルド済み、`target\release\auto_backup.exe` を使う

## 準備

1. テスト用ディレクトリを作成する。

   ```powershell
   New-Item -ItemType Directory -Force C:\tmp\vss_test\source
   New-Item -ItemType Directory -Force C:\tmp\vss_test\dest
   "hello" | Out-File C:\tmp\vss_test\source\a.txt
   "world" | Out-File C:\tmp\vss_test\source\b.txt
   ```

2. テスト用 `config.toml` を作成する(`C:\tmp\vss_test\config.toml`)。

   ```toml
   [source]
   paths = ["C:/tmp/vss_test/source"]

   [destination]
   path = "C:/tmp/vss_test/dest"

   [archive]
   max_part_size_mb = 500

   [behavior]
   skip_unreadable_files = false

   [work]
   path = "C:/tmp/vss_test/work"

   [log]
   path = "C:/tmp/vss_test/auto_backup.log"
   ```

3. 実行前のシャドウコピー一覧を記録しておく(テスト後に増減がないか比較するため)。

   ```powershell
   vssadmin list shadows
   ```

## テストケース1: 管理者権限あり・ロックなし(正常系)

1. **管理者として** PowerShellを開き、バックアップを実行する。

   ```powershell
   & "C:\dev\script\rust\auto_backup\target\release\auto_backup.exe" backup --config C:\tmp\vss_test\config.toml
   ```

2. `C:\tmp\vss_test\auto_backup.log` を確認し、次の行があることを確認する。
   - `VSS snapshot created for volume C:\`
   - `VSS snapshot used for C:\tmp\vss_test\source`
3. `C:\tmp\vss_test\dest` にアーカイブとマニフェストが作成されていることを確認する。
4. `vssadmin list shadows` を再度実行し、バックアップ完了後にシャドウコピーが残っていない(準備時の一覧と一致する)ことを確認する。

## テストケース2: ロックされたファイルを含む

VSS経由なら、他プロセスが排他ロックしているファイルもバックアップできることを確認する。

1. 別のPowerShellウィンドウで、テスト用ファイルを排他ロックしたまま保持するスクリプトを実行する(このウィンドウは閉じずに残す)。

   ```powershell
   "locked content" | Out-File C:\tmp\vss_test\source\locked.txt
   $fs = [System.IO.File]::Open("C:\tmp\vss_test\source\locked.txt", 'Open', 'ReadWrite', 'None')
   Write-Host "locked.txt をロック中。このウィンドウは閉じずに残してください。"
   # このウィンドウはテスト終了まで放置する
   ```

2. **管理者として** 別ウィンドウでテストケース1と同じバックアップコマンドを実行する。
3. バックアップが成功し、ログに `VSS snapshot used for ...` が出ることを確認する(`skipped_files` にも `locked.txt` が記録されないこと)。
4. 復元して `locked.txt` の中身が正しく含まれていることを確認する。

   ```powershell
   & "C:\dev\script\rust\auto_backup\target\release\auto_backup.exe" restore --config C:\tmp\vss_test\config.toml --input C:\tmp\vss_test\dest\<マニフェスト名>.manifest.toml --output C:\tmp\vss_test\restore
   Get-Content C:\tmp\vss_test\restore\locked.txt
   ```

5. ロックを保持しているウィンドウで `$fs.Close()` を実行してロックを解除する。

## テストケース3: 管理者権限なし(非昇格)でのフォールバック

1. **管理者権限を昇格していない** 通常のPowerShellでバックアップを実行する(テストケース1と同じコマンド)。
2. ログに次のようなフォールバック行が出ることを確認する。
   - `VSS unavailable: ...; falling back to direct read for all sources`
   - `direct read (no VSS) for C:\tmp\vss_test\source`
3. ロックされていないファイルのみの場合、バックアップ自体は成功することを確認する(従来通りの動作)。
4. テストケース2のようにロックされたファイルがある状態で非昇格実行すると、`behavior.skip_unreadable_files = false` の場合はエラーで停止し、`true` の場合はスキップされて `skipped_files` に記録されることを確認する(VSS導入前と同じ挙動であること)。

## テストケース4: 同一ボリューム上の複数source

1. `config.toml` の `source.paths` に、同じドライブ上の2つのディレクトリを追加する。

   ```toml
   [source]
   paths = [
     "C:/tmp/vss_test/source",
     "C:/tmp/vss_test/source2"
   ]
   ```

2. **管理者として** バックアップを実行する。
3. ログに `VSS snapshot created for volume C:\` が **1回だけ** 出ること(2回作成されていないこと)を確認する。
4. 両方のsourceで `VSS snapshot used for ...` が出ることを確認する。

## テストケース5: 異常終了時の後始末(既知の制限の確認)

VSSセッションのシャドウコピー削除は `Drop` 実装([src/vss.rs](../src/vss.rs))で行われるが、`Ctrl+C` や `taskkill /F` による強制終了時はRustのデストラクタが実行されず、シャドウコピーが残る可能性がある。この既知の制限を確認する。

1. **管理者として** テストケース1のバックアップを実行し、進行中に `Ctrl+C` で強制終了する。
2. `vssadmin list shadows` を実行し、シャドウコピーが残っていないか確認する。
   - 残っていた場合は既知の制限として記録し、以下で手動削除する。

   ```powershell
   vssadmin delete shadows /for=C: /oldest
   ```

## 後始末

```powershell
Remove-Item -Recurse -Force C:\tmp\vss_test
```

`vssadmin list shadows` を実行し、テスト前の状態(準備時に記録した一覧)と一致していることを最終確認する。
