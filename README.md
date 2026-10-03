# Astra

AviUtl および拡張編集 ( ExEdit / ExEdit2 ) 向けのスクリプト・プラグイン開発を支援するビルドツール＆タスクランナー．

スクリプトの前処理，外部ビルドとの連携，AviUtl ExEdit2 の動作確認環境，配布パッケージの作成を `astra.toml` で管理できる．詳しい使い方や設定は [Wiki](./wiki/Home.md) を参照されたい．

## 導入

### Release からダウンロード

[Releases](https://github.com/korarei/AviUtl2_Astra/releases) から，使用する OS・アーキテクチャに対応したアーカイブをダウンロードして展開する．Windows では `windows-x64.zip` または `windows-arm64.zip` で終わるアーカイブを選び，同梱の `astra.exe` を PATH の通った場所に配置する．

### mise

[mise](https://mise.jdx.dev/installing-mise.html) を導入し，GitHub Releases から Astra をインストールする．[GitHub バックエンド](https://mise.jdx.dev/dev-tools/backends/github.html) を使用し，以下のコマンドでグローバル設定に追加する．

```pwsh
mise use -g github:korarei/AviUtl2_Astra@latest
```

プロジェクトごとに管理する場合は，プロジェクトの `mise.toml` の `[tools]` に以下を追加する．既に `[tools]` がある場合は，その中に設定行を追加する．

```toml
[tools]
"github:korarei/AviUtl2_Astra" = "latest"
```

設定後，プロジェクトのディレクトリで以下のコマンドを実行する．

```pwsh
mise install
```

mise をシェルで有効化している場合は，`astra` コマンドをそのまま使用できる．有効化していない場合は，`mise exec` 経由で実行する．

```pwsh
mise exec -- astra --help
```

## 主な機能

- スクリプトの前処理：Lua・HLSL・INI のインクルード，変数展開，条件分岐，GUI プロパティの変換，多言語化テンプレートの生成．
- ビルドとタスク実行：Debug / Release のビルド設定，外部コマンドによるプラグインのコンパイル，依存関係と後処理の管理．
- 動作確認環境の構築：プロジェクト専用の AviUtl ExEdit2 への配置と起動，再ビルド・再起動による変更の反映．
- 配布パッケージの作成：`.au2pkg.zip` や通常の ZIP アーカイブの生成，変更履歴の先頭バージョンからのリリースノート抽出．
- キャッシュ管理：パッケージ URL の再取得，未使用キャッシュの整理，ビルド・配布成果物のクリーン．

## 基本的な使い方

まずプロジェクトを初期化する．

```pwsh
astra init my-effects --name MyEffects
Set-Location my-effects
```

初期化では最小限の `astra.toml` が生成される．[入門ガイド](./wiki/Getting-Started.md) に従ってソースと `builds`・`releases` の構成を追加し，`[astra.run].release` に動作確認用のリリース ID を設定する．

```pwsh
astra build
astra run
astra release
```

`build` は既定で Debug ビルドを行い，成果物を `build/<ビルドID>/debug/` に出力する．`release` は Release ビルドを行い，配布ファイルを `dist/<リリースID>/` に出力する．ネイティブプラグインのコンパイルには外部タスクを使用する．

タスク名を指定しない `run` は Windows 専用で，au2pkg の構成を使って `.astra/runtime/aviutl2/` に成果物を配置し，AviUtl ExEdit2 を起動する．初回の導入にはネットワーク接続が必要で，配置にはシンボリックリンクを作成できる権限が必要となる．ソースの変更後は Astra の端末で `Ctrl+R`，または `r` を入力して Enter を押すと，再ビルドして再起動する．

## ドキュメント

導入後の進め方は [入門ガイド](./wiki/Getting-Started.md) を参照されたい．各機能の詳細は以下の Wiki ページで説明している．

- [設定ファイル ( astra.toml )](./wiki/Configuration.md)
- [プリプロセッサ](./wiki/Preprocessor.md)
- [ビルド設定](./wiki/Build-Targets.md)
- [プロパティと多言語化](./wiki/Properties-and-Localization.md)
- [タスクランナー](./wiki/Tasks.md)
- [実行環境と動作確認](./wiki/Testing-and-Runtime.md)
- [リリースとパッケージング](./wiki/Releases-and-Packaging.md)
- [キャッシュとクリーン](./wiki/Cache-and-Cleanup.md)
- [コマンドリファレンス](./wiki/CLI-Reference.md)

## ライセンス

本プログラムのライセンスは [LICENSE](./LICENSE) を参照されたい．

また，本プログラムが利用するサードパーティ製ライブラリ等のライセンス情報は [THIRD_PARTY_LICENSES](./THIRD_PARTY_LICENSES.md) に記載している．

## 更新履歴

[CHANGELOG](./CHANGELOG.md) を参照されたい．
