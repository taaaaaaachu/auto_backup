# auto_backup

指定したフォルダを圧縮して、別の場所(外付けドライブやNASなど)へバックアップする Windows 用のコマンドラインツールです。

- フォルダを `tar.zst` 形式で圧縮し、大きい場合は指定サイズごとに分割して保存します
- 保存先にコピーしたあと、ファイルが壊れていないかをチェックサムで確かめます
- 他のアプリが開いているファイルもバックアップできます(管理者として実行した場合)
- 1回実行して終わるコマンドなので、定期的なバックアップは Windows タスクスケジューラから呼び出します

## インストール

Rust(Cargo)でビルドします。

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

   [archive]
   max_part_size_mb = 500
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

設定ファイルはTOML形式です。`[source]`・`[destination]`・`[archive]` の `max_part_size_mb` は必須で、それ以外は省略すると既定値が使われます。すべての項目を書いた例は [config.example.toml](config.example.toml) を見てください。

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
| `max_part_size_mb` | (必須) | バックアップファイル1つあたりの最大サイズ(MB)。これを超えると分割されます。 |
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

- `max_part_size_mb` 以下に収まった場合

  ```text
  20260612_010000_documents.tar.zst
  20260612_010000_documents.manifest.toml
  ```

- `max_part_size_mb` を超えた場合

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
archive bytes: 536870918/1073741830 (50%)
copy bytes: 209715200/524288000 (40%)
```

数字が変わらなくても2秒ごとに同じ行が表示されます。同じ数字のまま長く止まっている場合は、保存先のドライブやネットワークが応答していない可能性があります。

## 復元

```powershell
auto_backup restore --config config.toml --input <バックアップファイル> --output <復元先フォルダ>
```

`--input` には次のどれを指定しても復元できます。分割されている場合は、同じフォルダにある残りの分割ファイルが自動で使われます。

```powershell
# マニフェスト
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.manifest.toml --output C:/restore
# 分割ファイルのどれか1つ
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.part001 --output C:/restore
# 分割されていないバックアップファイル
auto_backup restore --config config.toml --input D:/backups/20260612_010000_documents.tar.zst --output C:/restore
```

復元前に、マニフェストに記録したチェックサムと照合し、ファイルが壊れている場合は復元しません。また、復元先に同じ名前のファイルやフォルダが1つでもある場合は、何も展開せずに中止します。空のフォルダを復元先に指定してください。

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

VSSを使う部分は管理者権限が必要なため、自動テストの対象外です。変更したときは [docs/manual-testing-vss.md](docs/manual-testing-vss.md) の手順で手動確認してください。
