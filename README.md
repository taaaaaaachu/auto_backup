# auto_backup

指定したフォルダを圧縮して、別の場所(外付けドライブやNASなど)へバックアップする Windows 用のコマンドラインツールです。

- フォルダを `tar.zst` 形式で圧縮し、大きい場合は指定サイズごとに分割して保存します
- 保存先にコピーしたあと、ファイルが壊れていないかをチェックサムで確かめます
- 他のアプリが開いているファイルもバックアップできます(管理者として実行した場合)
- 1回実行して終わるコマンドなので、定期的なバックアップは Windows タスクスケジューラから呼び出します
- `auto_backup update` で最新版に更新できます

## インストール

[Releases](https://github.com/taaaaaaachu/auto_backup/releases) から `auto_backup-x86_64-pc-windows-msvc.zip` をダウンロードし、中の `auto_backup.exe` を好きな場所に置きます。

自分でビルドする場合は Rust(Cargo)を使います。

```powershell
cargo build --release
```

`target/release/auto_backup.exe` ができるので、好きな場所に置いて使います。

## はじめかた

1. [config.example.toml](config.example.toml) をコピーして `config.toml` を作り、バックアップしたいフォルダと保存先を書きます。

   ```toml
   [source]
   paths = ["C:/Users/name/Documents"]

   [destination]
   path = "D:/backups"
   ```

2. 管理者として開いたターミナルでバックアップを実行します。

   ```powershell
   auto_backup backup --config config.toml
   ```

3. 保存先にバックアップファイルができます。

   ```text
   D:/backups/20260612_010000_documents.tar.zst
   D:/backups/20260612_010000_documents.manifest.toml
   ```

## 設定ファイル

設定ファイルはTOML形式です。必須なのは `[source]` と `[destination]` だけで、それ以外は省略すると既定値が使われます。すべての項目を書いた例は [config.example.toml](config.example.toml) を見てください。

### `[source]`

| 項目 | 説明 |
| --- | --- |
| `paths` | バックアップするフォルダの一覧。複数指定できます。 |

### `[destination]`

| 項目 | 説明 |
| --- | --- |
| `path` | バックアップファイルの保存先フォルダ。なければ作成されます。 |

### `[archive]`

| 項目 | 既定値 | 説明 |
| --- | --- | --- |
| `max_part_size_mb` | なし(分割しない) | バックアップファイル1つあたりの最大サイズ(MB)。これを超えると分割されます。FAT32のドライブやクラウドストレージなど、ファイルサイズに上限がある保存先で使います。 |
| `compression_level` | `6` | zstdの圧縮レベル。大きいほど小さくなりますが、時間がかかります。 |

### `[exclude]`

| 項目 | 既定値 | 説明 |
| --- | --- | --- |
| `patterns` | なし | バックアップから除外するファイル・フォルダのパターン。バックアップ元フォルダからの相対パスで書きます。 |

```toml
[exclude]
patterns = ["target/**", "node_modules/**", ".git/**", "*.tmp"]
```

### `[behavior]`

| 項目 | 既定値 | 説明 |
| --- | --- | --- |
| `verify_after_copy` | `true` | 保存先にコピーしたファイルを、サイズとチェックサムで確かめます。 |
| `delete_local_archive_after_copy` | `true` | コピー(と確認)に成功したら、作業フォルダに作った圧縮ファイルを削除します。 |
| `skip_unreadable_files` | `false` | `true` にすると、読み取れないファイルがあっても飛ばしてバックアップを続けます。`false` の場合はエラーで止まります。 |
| `check_for_updates` | `true` | バックアップのたびに新しい版が出ていないか GitHub に確かめ、出ていればログに案内を書きます。確かめられなかった場合も警告をログに書くだけで、バックアップは続けます。 |

### `[work]`

| 項目 | 既定値 | 説明 |
| --- | --- | --- |
| `path` | OSの一時フォルダ配下の `auto_backup` | 圧縮・分割のための作業フォルダ。圧縮後のバックアップファイルを置けるだけの空き容量が必要です。 |

### `[log]`

| 項目 | 既定値 | 説明 |
| --- | --- | --- |
| `path` | 実行したフォルダの `auto_backup.log` | ログの出力先。実行のたびに追記されます。 |

## バックアップ

```powershell
auto_backup backup --config config.toml
```

`paths` に書いたフォルダごとに、次のファイルが保存先に作られます。ファイル名は「実行日時_フォルダ名」です。フォルダ名は小文字になり、英数字・`-`・`_` 以外の文字は `_` に置き換わります。フォルダ名が重複する場合は、親フォルダの名前も付けて区別します。

- 分割しない場合(`max_part_size_mb` を省略した場合、または指定サイズ以下に収まった場合)

  ```text
  20260612_010000_documents.tar.zst
  20260612_010000_documents.manifest.toml
  ```

- `max_part_size_mb` を超えて分割された場合

  ```text
  20260612_010000_documents.part001
  20260612_010000_documents.part002
  20260612_010000_documents.manifest.toml
  ```

`.manifest.toml` は、分割ファイルの一覧とチェックサムを記録したファイルです。復元に使うので、バックアップファイルと一緒に保管してください。`skip_unreadable_files = true` で飛ばしたファイルも、ここの `skipped_files` とログに記録されます。

### 使用中のファイルのバックアップ

管理者として実行すると、Windows のボリュームシャドウコピー(VSS)を使ってフォルダを読み取ります。他のアプリが開いたままのファイルも含めて、実行した時点の状態をそのままバックアップできます。

管理者として実行しなかった場合は、フォルダを直接読み取ります。バックアップは行われますが、他のアプリがロックしているファイルは読み取れません(`skip_unreadable_files` の設定に従って、飛ばすかエラーで止まります)。どちらの方法で読み取ったかはログに記録されます。

### 進捗の表示

実行中は、いま何をしているかと進み具合がコンソールに表示されます。

```text
creating VSS snapshot...
scan entries: 1234
archive bytes: 51.374 GB/102.748 GB (50%)
copy bytes: 200.00 MB/500.00 MB (40%)
```

数字が変わらなくても2秒ごとに同じ行が表示されます。同じ数字のまま長く止まっている場合は、保存先のドライブやネットワークが応答していない可能性があります。

## 復元

```powershell
auto_backup restore --input <バックアップファイル> --output <復元先フォルダ>
```

`--config` は省略できます。省略すると、作業フォルダとログは既定値(`[work]`・`[log]` の既定値)になります。指定した場合も、設定ファイルから読むのは `[work]` と `[log]` だけです。バックアップに使った config.toml をそのまま渡すこともできますし、`[source]`・`[destination]` を書かない復元用の設定ファイルを使うこともできます。

`--input` には次のどれを指定しても復元できます。分割されている場合は、同じフォルダにある残りの分割ファイルが自動で使われます。

```powershell
# マニフェスト
auto_backup restore --input D:/backups/20260612_010000_documents.manifest.toml --output C:/restore
# 分割ファイルのどれか1つ
auto_backup restore --input D:/backups/20260612_010000_documents.part001 --output C:/restore
# 分割されていないバックアップファイル
auto_backup restore --input D:/backups/20260612_010000_documents.tar.zst --output C:/restore
```

復元前に、マニフェストに記録したチェックサムと照合し、ファイルが壊れている場合は復元しません。また、復元先に同じ名前のファイルやフォルダが1つでもある場合は、何も展開せずに中止します。空のフォルダを復元先に指定してください。

## 更新

```powershell
auto_backup update
```

GitHub Releases の最新版をダウンロードし、署名を確かめてから `auto_backup.exe` 自身を置き換えます。すでに最新なら何もしません。`auto_backup.exe` を置いたフォルダに書き込める権限で実行してください。

v0.1.0 にはこの機能がないので、v0.1.0 を使っている場合は一度だけ手動で入れ替えてください。

## 定期的に実行する

Windows タスクスケジューラで、次のようにタスクを作成します。

- **プログラム**: `auto_backup.exe` のフルパス
- **引数**: `backup --config C:/path/to/config.toml`
- **「最上位の特権で実行する」にチェック**: 使用中のファイルもバックアップするために必要です
- `[log]` の `path` を省略する場合は、**開始(作業フォルダー)** も指定しておくと、ログの場所が決まります

## 開発

```powershell
mise exec rust -- cargo fmt --check
mise exec rust -- cargo clippy --all-targets
mise exec rust -- cargo test
```

GitHub に push すると、GitHub Actions で同じチェック(fmt・clippy・test)が Windows 上で実行されます。`v1.0.0` のような `v` で始まるタグを push すると、チェックが通ったあとに `auto_backup.exe` を zip にまとめて署名し、GitHub Release に添付します。署名には GitHub Secrets の `ZIPSIGN_PRIVATE_KEY`(秘密鍵を base64 にしたもの)を使い、対になる公開鍵 [signing-public.key](signing-public.key) は exe に埋め込まれます。`Cargo.toml` の `version` を上げてから、同じ版のタグを push してください。

```powershell
git tag v0.1.0
git push origin v0.1.0
```

VSSを使う部分は管理者権限が必要なため、自動テストの対象外です。変更したときは [docs/manual-testing-vss.md](docs/manual-testing-vss.md) の手順で手動確認してください。

## ライセンス

[MIT License](LICENSE)
