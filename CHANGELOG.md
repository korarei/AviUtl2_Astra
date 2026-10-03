# Changelog

## [0.7.0] - 2026-10-03

### Added

- `astra run <task>` で任意のコマンドを実行するタスクランナーを追加．タスクごとのシェル，変数，環境変数，依存関係，後処理，非公開設定に対応し，循環依存を検出する．
- ビルド・リリース設定に `depends` と `finally` を追加し，処理の前後にタスクを呼び出せるようにした．
- `astra build <id>` と `astra release <id>` による設定の個別実行に対応．複数のリリース設定を定義できるようにした．
- スクリプトプリプロセッサに `#if`，`#elif`，`#ifdef`，`#ifndef`，`#else`，`#endif`，`#undef`，`#pragma once` を追加．条件式の評価に CEL を使用し，循環インクルードを検出する．
- HLSL の `//#...` と `.aul2` の `;#...` による前処理に対応．
- ExEdit2 向けスクリプト ( `.tra2` を除く ) の `pixelshader`／`computeshader` 定義と呼び出しの検証を追加．定義名の HLSL 識別子・予約語チェック，重複定義とエントリーポイント未定義の検出，呼び出しの第一引数が文字列リテラルであることを検証する．`@` を含まない呼び出し名は定義の存在も確認し，定義順に依存せず，pixel と compute を独立して扱う．
- `pixelshader` / `computeshader` のブロックコメント内で，スクリプトの前処理ディレクティブと変数展開を処理する機能を追加．
- HLSL の `cbuffer` に，非 `float` 型・未対応型，パディング，レイアウトを確定できない宣言や条件付き定義を位置情報付きで警告する機能を追加．配列・行列と `row_major` / `column_major`，`#pragma pack_matrix` を考慮して解析する．
- HLSL から Lua ファイルをインクルードした場合にエラーとする検証を追加．
- プロパティ記法の引数を検証し，変数名や初期値に指定した `_` を変数宣言から補完する機能を追加．旧拡張編集向けの記法と ExEdit2 向けの記法を検証する．
- ExEdit2 向けスクリプトから表示名やツールチップを抽出し，多言語化テンプレート `Default.<name>.aul2` を生成する機能を追加．
- ダウンロードしたファイルと展開結果を `.astra/cache/` に保存するパッケージキャッシュを追加．`astra cache clean`，`astra cache gc`，`astra cache refresh` で管理できるようにした．
- パッケージに含める URL の `extract` と `pick` に対応し，ZIP の展開有無や取り出すファイルを指定できるようにした．
- `astra run` に `Ctrl+R` または `r` による再ビルド・再配置・再起動を追加．設定ファイルの変更も再読み込みする．
- `astra run` で AviUtl2 のデバッグ出力を表示し，終了コードを確認する機能を追加．
- コメントや書式を保持したままプロジェクトバージョンを書き換える `astra set-version <version>` を追加．
- `astra init --name` によるプロジェクト名の指定と，複数作者の設定に対応．
- 設定ファイルと同じディレクトリにある `.env` の読み込み，ルートの変数・環境変数，共通の実行シェル設定に対応．
- `--verbose` による詳細ログの表示を追加．

### Changed

- 実装を Python から Rust に移行．Python 実行環境を必要としない構成に変更．
- `astra.toml` の設定形式を刷新し，ルートに設定形式のバージョン ( `version = 2` 以上 ) を必須とした．旧形式の `[[build.plugins]]`，`[[build.scripts]]`，`[release.contents]` は新形式への移行が必要．
- ビルド設定を `[build.<id>]`，リリース設定を `[release.<id>]` に統一．プラグインのビルドコマンドはタスクへ移し，生成物を `artifacts` で指定する方式に変更．パッケージ内の生成物参照を `script:<id>` / `plugin:<id>` から `build:<id>` に変更．
- 変数展開の記法を `${VAR}` から `${{ VAR }}` に変更．未定義変数や未展開の変数をエラーとし，予約変数の上書きを禁止した．
- ビルド・タスク・ターゲットごとに変数のスコープを分ける方式に変更．`BUILD_TYPE`，`BUILD_DIR`，`DEBUG`，`NDEBUG` などの予約変数を用意した．
- ビルドモードの指定を `-c` / `--config` から `--release` / `--debug` に変更．ビルド先を `[astra]` の `build_dir`，配布物の出力先を `dist_dir` で管理し，既定の配布先を `release/` から `dist/` に変更．
- プロジェクトバージョンの上書きをグローバルオプション `-v` / `--project-version` に統一．Astra 自体のバージョン表示は `-V` / `--version` に変更．`-d` / `--define` もグローバルオプションに変更．
- 設定ファイルをカレントディレクトリから親ディレクトリへさかのぼって検索し，見つかった `astra.toml` のディレクトリを基準に処理する方式に変更．
- `astra run` の実行環境を `.venv/` から `.astra/runtime/` に変更．実行対象は `[astra.run].release` で指定した `au2pkg` のリリース設定から構築する．
- 実行環境の配置状態をマニフェストで管理し，内容に変更がない場合は再配置を省略する方式に変更．AviUtl2 の更新時は既存の `data/` を保持する．
- 複数のスクリプトをまとめるビルドでは，ターゲット名からセクション見出しを生成する方式に変更．出力文字コードは拡張子に応じて ExEdit2 向けを UTF-8，旧 ExEdit 向けを Shift JIS とする．
- Lua のインクルードに続く `require` を，インクルード内容を実行する無名関数に置き換える方式に変更．モジュール名とファイル名の一致を検証する．
- Astra が展開する HLSL のインクルード指定を `#include` から `//#include` に変更．
- プロパティの変数名不一致を警告からエラーに変更．インクルード先が見つからない場合もエラーとし，スクリプトの診断にファイル名・行・列を表示する．
- パッケージの拡張機能・ドキュメント・アセットを `package.contents` の `dir` と `sources` で指定する方式に統一．
- リリースノートの入力ファイルを `notes.changelog` で明示する方式に変更．出力ファイル名と文字コードを指定できるようにし，既定のファイル名を `release_notes.md` から `RELEASE_NOTES.md` に変更．
- `astra schema` のファイル出力を，位置引数の出力先ディレクトリから `--output <path>` によるファイル指定に変更．
- `astra init` で最小限の `astra.toml` と `.gitignore`，`.gitattributes` を生成する方式に変更．既存の `.gitignore` と `.gitattributes` は保持する．
- `astra clean` の対象をビルド・配布先の各設定の生成物とパッケージキャッシュに変更．実行環境を保持する方式に変更．

### Removed

- `install`，`uninstall`，`venv` コマンドと `--venv` オプションを削除．テスト環境の構築と配置は `astra run` に統合．
- 仮想環境の activate スクリプトの生成を廃止．
- `.config/` と `.astra/` 配下の設定ファイル，および `astra.tml` の自動検索を廃止．
- `astra init` による `.editorconfig` の生成を廃止．
- Lua モジュール展開時に `if ... then` ブロックを削除する処理を廃止．

### Fixed

- Lua の文字列やブロックコメント内の記述を，プロパティ記法として誤って変換する問題を修正．
- パッケージ作成時のアセットのダウンロード失敗を警告だけで処理し続ける動作を修正し，エラーとして報告するようにした．
- パッケージ内で同じ出力先にファイルを配置した場合，暗黙に上書きする動作を修正．配置先の重複や自動生成する `package.ini`／`package.txt` との衝突を検出する．

## [0.6.5] - 2026-06-23

### Fixed

- `release` コマンドで一時フォルダのクリーンアップ時エラーが発生したとしても無視するように修正．

## [0.6.4] - 2026-06-02

### Added

- プロパティ項目の変数名とソース内変数名が異なる場合警告する機能を追加．
- バイナリ形式での配布を開始．

### Changed

- 仮想環境構築時に初期設定値を書き込むように変更．
- `#define` で定義した変数を再帰的に展開するように変更．

### Fixed

- `au2pkg` のディレクトリ指定で親フォルダのみ指定した場合認識されない問題の修正．

## [0.6.3] - 2026-05-03

### Fixed

- lua モジュール展開後の if 文削除の正規表現パターンミスの修正．

## [0.6.2] - 2026-04-29

### Changed

- プラグインビルドでコマンドとしてリスト以外に文字列を受け付けるように変更．

### Fixed

- 特定のキーが存在しない場合 `field_validator` が動作しない問題を修正．

## [0.6.1] - 2026-04-29

### Fixed

- `release` コマンドで出力先フォルダが存在しない場合，一時フォルダ作成に失敗する問題の解決．
- 仮想環境構築時のバージョン判定で末尾にアルファベットの存在しないものが認識されない問題の解決．

## [0.6.0] - 2026-04-28

### Added

- `build` ディレクトリと `release` ディレクトリにアクセスするときにファイルロックする機能を実装．
- `run` コマンドの追加．
- 仮想環境構築後に activate スクリプトを自動生成するように変更．
- プラグインビルドで使用するシェルを選択できる機能を追加．
- 設定ファイルでケバブケースとスネークケースのどちらも使用できるように変更．
- 設定ファイルのパッケージファイル名の指定方法を拡張．

### Changed

- バリデータとして `pydantic` を導入．
- `package.ini` の `uninstallSubFolderFile` のデフォルト値を `false` に変更．
- ログの表示レベルの変更．

## [0.5.2] - 2026-04-12

### Fixed

- `install` コマンドのヘルプメッセージの `%ProgramData%` を `%%ProgramData%%` に修正．

## [0.5.1] - 2026-04-12

### Added

- `package.ini` の `uninstallSubFolderFile` の対応．

### Fixed

- リリースノート生成機能と lua モジュール展開機能の正規表現を修正．

## [0.5.0] - 2026-04-11

### Added

- 変数展開時に展開されない変数を警告するように変更．
- `init` コマンドで `.editorconfig` を生成するように変更．
- 設定ファイルに astra のバージョンチェックを追加．
- HLSL で `#include` を展開する機能を追加．
- スクリプトファイル内変数定義として `#define` を追加．
- 仮想環境のセットアップ機能を追加．
- コマンドライン引数で変数を定義できる機能を追加．

### Changed

- モジュール展開時に実行されない if 文を削除するように変更．
- パケージ作成時に認識されないフォルダは除外するように変更．
- アンインストール時のディレクトリ削除でユーザに選択を迫らないように変更．
- `clean` コマンドで先にアンインストール処理をするように変更．

### Fixed

- `package.ini` の `[package]` セクションの記述漏れの修正．

## [0.4.3] - 2026-03-14

### Added

- 設定ファイルに `package.txt` で Summary，Website，Report Issue の項目を追加．
- 設定ファイルにスクリプトファイルのエンコーディングを追加．( exedit.auf 向け )

### Changed

- プラグインをビルドしたときのメッセージをバイパスするように修正．
- スクリプトビルド時の空行の扱いを修正．

### Fixed

- `uninstall` コマンドで `Plugin` と `Script` 以外のディレクトリを削除しないように修正．
- ドキュメントが指定されていないときリリースノートを作成しないように修正．

## [0.4.2] - 2026-03-11

### Changed

- `--#include` のインデントを展開時に引き継ぐように変更．

### Fixed

- 一部状況でプロパティ項目の置換に失敗する問題を修正．

## [0.4.1] - 2026-03-11

### Added

- ビルドとアセット作成を行うかどうかの設定項目を追加．

### Fixed

- Python 必要バージョンの修正．
- README のミスの修正．
- `import` 時 `init.py` と `schema.py` が入らない問題の修正．

## [0.4.0] - 2026-03-10

### Added

- 設定ファイルで変数を使用できる機能を追加．
- すべてのパス項目でワイルドカードを使用できるように修正．
- スクリプト実行形式からスクリプトファイル形式に変換できる機能の追加．( @, --track など )
- プラグインをビルドできる機能を追加．
- `release` で生成されるものに `build` 生成物以外を含めれる機能を追加．
- リリースノート生成機能で扱える更新履歴形式の拡大．
- アセットファイルとして URL 以外にファイル指定できるように変更．
- ビルドフォルダを削除するコマンドを追加．
- 一部コマンドでビルドフォルダにキャッシュ ( `astra.json` ) を生成するようにした．

### Changed

- 設定ファイルを JSON から TOML に変更．
- `--#include` 直下に `require` が存在する場合合わせて削除するようにした．
- `release` コマンドを実行したとき `buid` コマンドを実行するように変更．
- `release` で生成されるものは `au2pkg.zip` に固定．
- 各種コマンドの変更．

## [0.3.0] - 2025-11-03

### Added

- モジュールパスを設定できる機能を追加．
- 一部項目でワイルドカードを使用できる機能を追加．
- `install` にシンボリックリンク作成オプションを追加．
- `uninstall` コマンドを追加

### Changed

- `clean` 項目のデフォルトを `false` に変更．
- `install` で初期化フォルダが `Script` のとき，確認するようにした．
- 書き込み先指定を `-t`，`--target` で統一化．

## [0.2.0] - 2025-11-02

### Added

- `build` でバージョン指定できる機能を追加．

## [0.1.0] - 2025-11-02

### Added

- Release

[Unreleased]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.5...HEAD
[0.6.5]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/korarei/AviUtl2_Astra/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/korarei/AviUtl2_Astra/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/korarei/AviUtl2_Astra/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/korarei/AviUtl2_Astra/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/korarei/AviUtl2_Astra/compare/v0.4.3...v0.5.0
[0.4.3]: https://github.com/korarei/AviUtl2_Astra/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/korarei/AviUtl2_Astra/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/korarei/AviUtl2_Astra/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/korarei/AviUtl2_Astra/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/korarei/AviUtl2_Astra/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/korarei/AviUtl2_Astra/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/korarei/AviUtl2_Astra/releases/tag/v0.1.0
