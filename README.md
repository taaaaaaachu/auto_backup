# auto_backup

`auto_backup` は、ディレクトリを定期バックアップするための小さなCLIアプリです。

TOML設定ファイルに書いたディレクトリを `tar.zst` 形式で圧縮し、指定サイズを超える場合は分割します。圧縮後はSHA-256チェックサム付きのマニフェストを作成し、設定された保存先へコピーします。コピー後にサイズとチェックサムを検証し、成功した場合だけ作業用のローカル圧縮ファイルを削除します。

アプリ自体は1回実行のコマンドとして作っています。定期実行は Windows タスクスケジューラ、cron、その他のスケジューラから呼び出してください。

## 機能

- TOML設定ファイルに対応
- 複数のバックアップ元ディレクトリに対応
- `tar.zst` 形式で圧縮
- 設定した最大サイズを超えた場合に分割
- 分割ファイルと圧縮ファイル全体のSHA-256チェックサムを記録
- `target/**`、`.git/**`、`node_modules/**` などの除外パターンに対応
- 保存先へコピー後、チェックサム検証に成功してから作業ファイルを削除
- 復元時は分割ファイルを自動検出して結合
- 復元先ディレクトリをコマンドで指定可能
- 復元先に既存ファイルまたは既存ディレクトリがある場合はエラー
- 追記形式のログ出力

## ビルド

通常のCargoでビルドできます。

```powershell
cargo build --release
```

実行ファイルは次の場所に作成されます。

```text
target/release/auto_backup.exe
```

## 設定

[config.example.toml](config.example.toml) を `config.toml` としてコピーし、環境に合わせて編集します。

```toml
[source]
paths = [
  "C:/Users/name/Documents",
  "C:/dev/project"
]

[destination]
path = "D:/backups"

[archive]
max_part_size_mb = 500
compression_level = 6

[exclude]
patterns = [
  "target/**",
  "node_modules/**",
  ".git/**",
  "*.tmp"
]

[behavior]
verify_after_copy = true
delete_local_archive_after_copy = true
skip_unreadable_files = false

[work]
path = "C:/tmp/auto_backup"

[log]
path = "C:/tmp/auto_backup/auto_backup.log"
```

## 設定項目

`source.paths`
: 圧縮するディレクトリの一覧です。

`destination.path`
: 圧縮ファイル、分割ファイル、マニフェストのコピー先ディレクトリです。

`archive.max_part_size_mb`
: コピーされる圧縮ファイル1つあたりの最大サイズです。圧縮後のファイルがこの値を超えた場合、`.part001`、`.part002` のように分割されます。

`archive.compression_level`
: zstdの圧縮レベルです。通常は `6` のままで問題ありません。

`exclude.patterns`
: バックアップ元ディレクトリ内の相対パスに対して適用する除外パターンです。

`behavior.verify_after_copy`
: `true` の場合、保存先へコピーしたファイルをサイズとSHA-256チェックサムで検証します。

`behavior.delete_local_archive_after_copy`
: `true` の場合、保存先へのコピーと検証に成功したあと、作業ディレクトリ内の圧縮ファイルを削除します。

`behavior.skip_unreadable_files`
: `true` の場合、Outlook の `.pst` など、別プロセスにロックされて読めないファイルをスキップしてバックアップを続行します。スキップしたファイルはログとマニフェストの `skipped_files` に記録されます。既定値は `false` です。

`work.path`
: 圧縮や分割のために使う作業ディレクトリです。省略するとOSの一時ディレクトリ配下を使います。

`log.path`
: ログの出力先です。省略するとカレントディレクトリの `auto_backup.log` に出力します。

## バックアップ

```powershell
auto_backup backup --config config.toml
```

バックアップファイル名には、実行日時とバックアップ元ディレクトリ名が入ります。

分割された場合の出力例:

```text
20260612_010000_documents.part001
20260612_010000_documents.part002
20260612_010000_documents.manifest.toml
```

複数のバックアップ元で末尾のディレクトリ名が重複する場合は、親ディレクトリ名を順に足して一意になる名前を作ります。それでも一意にならない場合は短いハッシュを付けます。

## 復元

復元コマンドは、マニフェスト、単一の `.tar.zst` ファイル、または任意の分割ファイルを入力として受け取れます。

```powershell
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.manifest.toml --output C:/restore
```

分割ファイルを指定した場合は、同じディレクトリにあるマニフェストを使って自動的に全パートを検出し、結合してから復元します。

次の入力はいずれも有効です。

```powershell
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.manifest.toml --output C:/restore
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.part001 --output C:/restore
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.tar.zst --output C:/restore
```

復元先に既存ファイルまたは既存ディレクトリがある場合、復元は失敗します。安全に使うため、空のディレクトリを復元先に指定してください。

## 開発時の確認

```powershell
mise exec rust -- cargo fmt --check
mise exec rust -- cargo check
mise exec rust -- cargo test
```

## 定期実行

Windows タスクスケジューラを使う場合は、リリースビルドした実行ファイルを指定し、引数に次のような内容を設定します。

```text
backup --config C:/path/to/config.toml
```

`log.path` を省略する場合は、タスクの作業ディレクトリも固定しておくとログの場所が分かりやすくなります。
