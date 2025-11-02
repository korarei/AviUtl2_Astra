# Astra

AviUtl ExEdit2スクリプト用のビルド，開発支援ツール．

## インストール方法

### pip

以下のコマンドを実行する．

```bash
pip install git+https://github.com/korarei/AviUtl2_Astra.git@v0.1.0
```

## 主な機能

### ビルド

ソースファイルをAviUtl ExEdit2が認識する形式に変換する．

#### 文字置換

`${VAR}`のように書いた場所は`variables`で指定した文字列に置換する．プロジェクト名およびスクリプト名は`${PROJECT_NAME}`および`${SCRIPT_NAME}`として利用可能．また，バージョンや作者を設定した場合，`${VERSION}`や`${AUTHOR}`も利用可能となる．これら変数は`variables`を上書きする形で追加される．

#### ファイル埋め込み

`--#include "a.hlsl"`や`--#include <a.hlsl>`と記載した行はそのファイルの中身で置き換わる．`local a --#include`のように行頭が空白でない場合置換されない．

`" "`で指定した場合，ソースファイルからの相対パスで探索を行う．一方，`< >`で指定した場合，`include_directories`で指定したディレクトリを探索する．

### インストール

設定ファイルで指定した場所にビルドしたものをコピーする．コマンドラインでディレクトリを設定した場合，そちらが優先される．

### リリース

`zip`ファイル作成やリリースノートを作成を行う．

#### アーカイブ圧縮

ビルドしたものと`files`で指定したもの，外部から入手したアセットファイルを`zip`にする．

#### リリースノート作成

```markdown
## Change Log
- **v0.1.0**
  - Release

```

のように書かれた部分を抜き取りリリースノートを作成する．

## 設定ファイル

`astra.config.json`をソースファイルと同じ階層に用意する．

設定ファイル内のパスは基本的にこの設定ファイルからの相対パスで指定する．ただし，`assets`のパスはアーカイブパスで指定する．

```JSON
{
  "project": {
    "name": "Project",
    "version": "v0.1.0",
    "author": "name"
  },
  "build": {
    "clean": true,
    "directory": "build",
    "scripts": [
      {
        "name": "Effect",
        "suffix": ".anm2",
        "newline": "\r\n",
        "source": {
          "tag": ".in",
          "include_directories": [
            "includes"
          ],
          "variables": {
            "LABEL": "アニメーション効果"
          }
        }
      }
    ]
  },
  "install": {
    "clean": true,
    "directory": "C:/ProgramData/aviutl2/Script"
  },
  "release": {
    "clean": true,
    "directory": "release",
    "archive": {
      "files": [
        "../README.md",
        "../LICENSE"
      ],
      "assets": [
        {
          "directory": "assets",
          "url": "https://",
          "texts": [
            {
              "file": "credits.txt",
              "content": "This is a sample asset."
            }
          ]
        }
      ]
    },
    "notes": {
      "source": "../README.md"
    }
  }
}
```

必須項目についてはスキームを確認してほしい．

## コマンド

```bash
astra <command> [options]
```

使用可能なコマンドを以下に示す．`-h`，`--help`でヘルプを表示可能．

### `init`

`astra.config.json`設定ファイルをカレントディレクトリに作成する．

#### 使用方法

```bash
astra init [options]
```

#### オプション

- `-o <directory>`，`--output <directory>`

  設定ファイルを出力するディレクトリを設定する．(デフォルト: カレントディレクトリ)

- `-f`，`--force`

  既存の`astra.config.json`が存在する場合でも上書きする．

### `build`

設定ファイルに基づいてプロジェクトをビルドする．

#### 使用方法

```bash
astra build [options]
```

#### オプション

- `-s <directory>`，`--source <directory>`

  設定ファイルが含まれるソースディレクトリを指定する．(デフォルト: カレントディレクトリ)

- `-c <filename>`，`--config <filename>`

  設定ファイル名を指定する．(デフォルト: `astra.config.json`)

### `install`

ビルドされたものを指定した場所にインストールする．

#### 使用方法

```bash
astra install [options]
```

#### オプション

- `-s <directory>`，`--source <directory>`

  設定ファイルが含まれるソースディレクトリを指定する．(デフォルト: カレントディレクトリ)

- `-c <filename>`，`--config <filename>`

  設定ファイル名を指定する．(デフォルト: `astra.config.json`)

- `-d <directory>`，`--destination <directory>`

  インストール先のディレクトリを指定する．これは設定ファイルより優先される．

### `release`

プロジェクトをリリース用にパッケージ化する．

#### 使用方法

```bash
astra release [options]
```

#### オプション

- `-s <directory>`，`--source <directory>`

  設定ファイルが含まれるソースディレクトリを指定する．(デフォルト: カレントディレクトリ)

- `-c <filename>`，`--config <filename>`

  設定ファイル名を指定する．(デフォルト: `astra.config.json`)

### `schema`

設定ファイル (`astra.config.json`) のためのJSONスキーマを生成する．

#### 使用方法

```bash
astra schema [options]
```

#### オプション

- `-o <directory>`，`--output <directory>`

  スキーマファイルを出力するディレクトリを指定する．(デフォルト: カレントディレクトリ)

- `-f`，`--force`

  既存のスキーマファイルが存在する場合でも強制的に上書きする．

- `-b`，`--build`

  ビルドコマンドに必要な設定ファイルのスキーマファイルを`astra.build_schema.json`として生成する．

- `-i`，`--install`

  インストールコマンドに必要な設定ファイルのスキーマファイルを`astra.install_schema.json`として生成する．

- `-r`，`--release`

  リリースコマンドに必要な設定ファイルのスキーマファイルを`astra.release_schema.json`として生成する．


## License

LICENSEに記載．

## Credits

### jsonschema

https://github.com/python-jsonschema/jsonschema

---

The MIT License

Copyright (c) 2013 Julian Berman

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

## Change Log
- **v0.1.0**
  - Release
