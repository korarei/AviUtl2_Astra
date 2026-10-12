# Astra

AviUtl および拡張編集 (ExEdit / ExEdit2) 向けスクリプト・プラグイン開発支援ビルドツール＆タスクランナー．

スクリプトの前処理，外部ビルドとの連携，AviUtl ExEdit2 上での動作確認環境の構築，配布パッケージの作成などを `astra.toml` で一元管理できる．詳細な仕様や設定方法は [Wiki](https://github.com/korarei/AviUtl2_Astra/wiki) を参照されたい．

## 主な機能

- スクリプトの前処理: Lua・HLSL・INI のインクルード，変数展開，条件分岐，GUI プロパティの変換，多言語化テンプレートの自動生成
- ビルドとタスク実行: Debug / Release それぞれのビルド設定，外部コマンドを用いたプラグインのコンパイル，依存関係および後処理タスクの管理
- 動作確認環境の構築: プロジェクト専用の AviUtl ExEdit2 環境への成果物配置と自動起動，再ビルド・再起動による変更の即時反映
- 配布パッケージの作成: `.au2pkg.zip` や汎用 ZIP アーカイブの生成，CHANGELOG からのリリースノート自動抽出
- キャッシュ管理: パッケージ URL の再取得，不要キャッシュの整理，ビルド成果物のクリーンアップ

## 導入

### Releases からダウンロード

[Releases](https://github.com/korarei/AviUtl2_Astra/releases) から使用環境 (OS・アーキテクチャ) に適したアーカイブをダウンロードして展開する．Windows では `windows-x64.zip` または `windows-arm64.zip` を選択し，同梱されている `astra.exe` を PATH の通ったディレクトリに配置する．

### mise

[mise](https://mise.jdx.dev/installing-mise.html) を使用すると，GitHub Releases 経由で Astra を導入・管理できる．[GitHub バックエンド](https://mise.jdx.dev/dev-tools/backends/github.html) を用い，以下のコマンドでグローバル環境に追加する．

```pwsh
mise use -g github:korarei/AviUtl2_Astra@latest
```

プロジェクト単位でバージョンを管理する場合は，プロジェクトルートの `mise.toml` における `[tools]` セクションに以下を記述する (既に `[tools]` が存在する場合は設定行を追加する)．

```toml
[tools]
"github:korarei/AviUtl2_Astra" = "latest"
```

記述後，プロジェクトディレクトリで以下のコマンドを実行してインストールする．

```pwsh
mise install
```

mise のシェル統合を有効にしている場合は，`astra` コマンドを直接実行できる．有効にしていない場合は，`mise exec` 経由で実行する．

```pwsh
mise exec -- astra --help
```

## 基本的な使い方

### 1. プロジェクトの初期化

以下のコマンドを実行してプロジェクトを新規作成する．

```pwsh
astra init my-effects --name MyEffects
cd my-effects
```

初期化によって最小構成の `astra.toml` が生成される．[入門ガイド](https://github.com/korarei/AviUtl2_Astra/wiki/Getting-Started) を参考に，ソースコードの配置や `builds`・`releases` の設定を追加する．

### 2. ビルド

```pwsh
astra build
```

既定で Debug ビルドを実行し，成果物を `build/<ビルドID>/debug/` に出力する．ネイティブプラグインのコンパイルなど外部ツールの呼び出しもビルドタスクとして連携できる．

### 3. 動作確認

```pwsh
astra run
```

タスク名を指定せずに実行した場合 (Windows 専用)，設定された構成に基づいて `.astra/runtime/aviutl2/` に成果物を配置し，AviUtl ExEdit2 を起動する．起動中は端末上で `Ctrl+R`，または `r` を入力して Enter を押すことで，再ビルドと再起動を即座に行える．

※ 初回起動時はランタイム取得のためネットワーク接続が必要となる．また，ファイル配置にシンボリックリンクを使用するため，作成権限が必要となる．

### 4. 配布パッケージの作成

```pwsh
astra release
```

Release ビルドを実行し，配布用パッケージ (`.au2pkg.zip` や通常の ZIP) を `dist/<リリースID>/` に出力する．

## ドキュメント

セットアップ後の詳細な開発手順については [入門ガイド](https://github.com/korarei/AviUtl2_Astra/wiki/Getting-Started) を参照されたい．各機能の仕様は以下の Wiki ページで解説している．

- [設定ファイル (astra.toml)](https://github.com/korarei/AviUtl2_Astra/wiki/Configuration)
- [プリプロセッサ](https://github.com/korarei/AviUtl2_Astra/wiki/Preprocessor)
- [ビルド設定](https://github.com/korarei/AviUtl2_Astra/wiki/Build-Targets)
- [プロパティと多言語化](https://github.com/korarei/AviUtl2_Astra/wiki/Properties-and-Localization)
- [タスクランナー](https://github.com/korarei/AviUtl2_Astra/wiki/Tasks)
- [実行環境と動作確認](https://github.com/korarei/AviUtl2_Astra/wiki/Testing-and-Runtime)
- [リリースとパッケージング](https://github.com/korarei/AviUtl2_Astra/wiki/Releases-and-Packaging)
- [キャッシュとクリーン](https://github.com/korarei/AviUtl2_Astra/wiki/Cache-and-Cleanup)
- [コマンドリファレンス](https://github.com/korarei/AviUtl2_Astra/wiki/CLI-Reference)

## ライセンス

本ソフトウェアのライセンスについては [LICENSE](./LICENSE) を参照されたい．

本ソフトウェアが利用しているサードパーティ製ライブラリ等のライセンス情報は [THIRD_PARTY_LICENSES](./THIRD_PARTY_LICENSES.md) に記載している．

## 更新履歴

変更内容の一覧は [CHANGELOG](./CHANGELOG.md) を参照されたい．
