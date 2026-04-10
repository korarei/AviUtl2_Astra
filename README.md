# Astra

AviUtl ExEdit2スクリプト用のビルド，開発支援ツール．

## インストール方法

### pip

以下のコマンドを実行する．更新は`-U`付きで実行する．

```pwsh
pip install git+https://github.com/korarei/AviUtl2_Astra.git@v0.5.0
```

> [!NOTE]
> 場合によっては環境変数に`astra.exe`のパスを設定する必要がある．

### uv

以下のコマンドを実行する．更新は`uv tool upgrade --all`または`uv tool install --upgrade <package>`で実行する．

```pwsh
uv tool install git+https://github.com/korarei/AviUtl2_Astra.git@v0.5.0
uv tool update-shell
```

## 主な機能

### ビルド

ソースファイルをAviUtl ExEdit2が認識する形式に変換する．

#### 文字置換

`${VAR}`のように書いた場所は`variables`等で指定した文字列に置換する．

置換前

```lua
--information:Effect@${PROJECT_NAME} v${PROJECT_VERSION} by ${PROJECT_AUTHOR}
```

置換後

```lua
--information:Effect@Project v0.1.0 by Author
```

#### ファイル展開

`--#include "a.hlsl"`や`--#include <a.hlsl>`と書かれた行はそのファイルで置換する．

`"`と`<`の違いはC言語と同様である．

展開前

```lua
--[[pixelshader@a:
--#include "a.hlsl"
]]
```

展開後

```lua
--[[pixelshader@a:
Texture2D src : register(t0)
SamplerState smp : register(s0)

float4
a(float4 pos : SV_Position, float2 uv : TEXCOORD) : SV_Target {
    return src.Sample(smp, uv);
}
]]
```

下記のように書いた場合，`require`行とその下1行を削除して中身を展開する．

```lua
--#include "a.lua"
local a = require("a")
local add = a.add
```

`require`するファイルは下記のように記述しておくことを推奨する．

```lua
--a.lua
local function add(a, b)
    return a + b
end

local function sub(a, b)
    return a - b
end

-- そのまま実行すると`...`は`nil`となり，`require`するとモジュール名となる

if (...) then -- Pythonの`if __name__ != "__main__":`
    return {
        add = add,
        sub = sub
    }
end
```

これにより，モジュールとしても利用可能で展開されても動作するようになる．

#### プロパティ項目の正規化

AviUtl ExEdit2の実行時形式をスクリプトとして認識する形式に変換する．

実行時形式

```lua
--@Effect
local a = 0 --track@a:A,0,100,0,0.01
```

正規化後

```lua
@Effect
--track@a:A,0,100,0,0.01
```

#### プラグインのビルド

設定ファイルに実行したいコマンドを追加することでプラグインのビルドを行うことができる．

コマンドは設定ファイルの置かれるディレクトリがカレントディレクトリとして実行される．

`${BUILD_DIRECTORY}`変数を設定ファイル内で利用可能である．

```toml
commands = [
    "cmake -S ./plugins -B ${BUILD_DIRECTORY}/Release -G Ninja -DCMAKE_BUILD_TYPE=Release",
    "cmake --build ${BUILD_DIRECTORY}/Release",
]
artifacts = ["${BUILD_DIRECTORY}/Release/*.mod2"]
```

### AviUtl ExEdit2へのインストール・アンインストール

設定ファイルに基づき，`Plugin/`や`Script/`等に設置されるものを設置する．

シンボリックリンクとして設置すると一回インストールしておけば以降ビルド毎にインストールしなくてよい．

### リリース

AviUtl2 ExEdit2パッケージ形式`au2pkg.zip`の作成やリリースノートの作成を行う．

#### アーカイブ圧縮

設定ファイルに基づきAviUtl2 ExEdit2パッケージ形式`au2pkg.zip`を生成する．

#### リリースノート作成

設定ファイルとして設定されたドキュメントのうち拡張子を除く名前が`CHANGELOG`または`README`が存在する場合`release_notes.md`が生成される．

> [!NOTE]
> READMEを使う場合，# Changelogセクションが必要． (#の数やChangeとlogの間の空白は問わない)

以下の形式に対応している．

```markdown
## 1.0.0
- Release

## v1.0.0
- Release

## [1.0.0]
- Release

- **1.0.0**
  - Release

- **v1.0.0**
  - Release

- **[1.0.0]**
  - Release
```

## 設定ファイル

設定は`astra.toml`に記述する．

設定ファイル内のパスは基本的にこの設定ファイルからの相対パスで指定する．

設定ファイルではワイルドカードや変数の利用が可能である．

<details>
<summary>astra.tomlの例</summary>

```toml
# Astra設定
[astra]
# 必要astraバージョン
requires-astra = ">=0.5.0"

# プロジェクト設定
[project]
# プロジェクト名 (必須)
name = "Project"
# プロジェクトバージョン
version = "0.1.0"
# プロジェクト作者
author = "Author"
# 必要AviUtl ExEdit2バージョン (文字列として設定すること)
requires-aviutl2 = "2003600"
# 設定ファイル，スクリプト全体で利用できる変数
variables = { PROJECT_LABEL = "Project" }

# ここで設定されたものは`PROJECT_NAME`のように設定され，valiablesに追加される
# ここで設定されたものは変数として設定ファイルやスクリプトファイル内で利用可能

# 変数一覧 (設定されたものだけ追加される)
# PROJECT_NAME
# PROJECT_VERSION
# PROJECT_AUTHOR
# PROJECT_REQUIRES_AVIUTL2

# ビルド
[build]
# プラグインビルド設定 (複数設定可能)
[[build.plugins]]
# ビルドするかどうか (設定されない場合，true)
enabled = true
# プラグイン固有のID (必須)
id = "core"
# このテーブル内部で利用できる変数
variables = { SOURCE = "./plugins" }
# リリースビルド (必須)
[build.plugins.release]
# コマンド
commands = [
    "cmake -S ${SOURCE} -B ${BUILD_DIRECTORY} -G \"Ninja Multi-Config\"",
    "cmake --build ${BUILD_DIRECTORY} --config Release",
]
# 生成物
artifacts = ["${BUILD_DIRECTORY}/Release/*.aux2"]
# デバッグビルド
[build.plugins.debug]
# コマンド
# ${BUILD_DIRECTORY}は`${build}/plugins/${id}`
commands = [
    "cmake -S ${SOURCE} -B ${BUILD_DIRECTORY} -G \"Ninja Multi-Config\"",
    "cmake --build ${BUILD_DIRECTORY} --config Debug",
]
# 生成物
artifacts = ["${BUILD_DIRECTORY}/Debug/*.aux2"]

# スクリプトビルド設定 (複数設定可能)
[[build.scripts]]
# ビルドするかどうか (設定されない場合，true)
enabled = true
# スクリプト固有のID (必須)
id = "effect"
# スクリプト名 (設定されない場合，プロジェクト名)
# SCRIPT_NAMEとして変数利用可能
name = "Effect"
# ファイル名の頭につける文字
prefix = "@"
# 拡張子 (設定されない場合，拡張子なし)
suffix = ".anm2"
# 改行コード (設定されない場合，CRLF)
newline = "\r\n"
# ソースファイルのエンコーディング (設定されない場合，UTF-8)
source-encoding = "utf-8"
# ターゲットファイルのエンコーディング (設定されない場合，UTF-8)
# 旧スクリプトファイルを作成する場合cp932 (Shift JIS) を指定する
target-encoding = "utf-8"
# このテーブルおよびスクリプトで利用できる変数
variables = { SOURCE = "./script" }
# `--#include`で検索するフォルダ
include_directories = ["${SOURCE}/shaders"]
# ソースファイル (複数設定した場合連結される)
sources = [
    # fileは必須 (ワイルドカードの利用も可能)
    # そのファイル内でしか使えない変数の設定も可能である
    { file = "effect1.lua", LABEL = "Effect1" },
    { file = "effect2.lua", LABEL = "Effect2" },
]

# リリース設定
[release]
# パッケージ設定
[release.package]
# 生成物の名前 (設定されない場合，プロジェクト名)
# `.au2pkg.zip`は追加される．
filename = "${PROJECT_NAME}"
# `package.ini`のid= (設定されない場合，プロジェクト名)
id = "${PROJECT_NAME}"
# `package.ini`のname= (設定されない場合，プロジェクト名)
# `package.txt`にも記載される
name = "${PROJECT_NAME}"
# `package.ini`のinformation= (設定されない場合，追加されない)
information = "${PROJECT_NAME} v${PROJECT_VERSION} by ${PROJECT_AUTHOR}"
# `package.txt`に記載ライセンス表記 (設定されない場合，追加されない)
license = "MIT"
# `package.txt`に記載される概要 (設定されない場合，追加されない)
summary = "Example plugin package summary"
# `package.txt`に記載される説明 (設定されない場合，追加されない)
description = "Example plugin package description"
# `package.txt`に記載されるウェブサイト (設定されない場合，追加されない)
website = "https://example.com"
# `package.txt`に記載されるIssue報告先 (設定されない場合，追加されない)
report-issue = "https://example.com/issues"

# 生成される`package.txt`
# [ ${name} ]
#
# ${summary}
#
# Version: ${PROJECT_VERSION}
# License: ${license}
# Author: ${PROJECT_AUTHOR}
# Website: ${website}
# Report Issue: ${report-issue}
#
# ${description}

# 内容物の設定
[release.contents]
# Plugin/に設置するもの
[[release.contents.extensions]]
# ファイルの設置場所
directory = "Plugin/${PROJECT_NAME}"
# ファイル (IDを設定した場合，artifactsに置換される)
files = ["plugin:core"]

# Script/に設置するもの
[[release.contents.extensions]]
directory = "Script/${PROJECT_NAME}"
# `.mod2`を追加する場合は`"plugin:module"`のようにして追加すること
files = ["script:effect"]

# directoryはAviUtl ExEdit2 SDKのreadmeを確認すること
# フォルダは大文字小文字が区別される (`script/`はAviUtl ExEdit2で認識されない)

# ドキュメント
[[release.contents.documents]]
# ドキュメントの設置場所
directory = "Script/${PROJECT_NAME}"
# ドキュメントファイル
files = ["./*.md", "./LICENSE"]

# アセット
[[release.contents.assets]]
# アセットの作成を行うかどうか (設定されない場合，true)
enabled = true
# アセット名 (必須)
name = "Assets"
# 設置場所
directory = "Script/${PROJECT_NAME}"

# 以下`${directory}/${name}`内に設置される

# ソースファイル
[[release.contents.assets.sources]]
# ファイルの設置場所 (`${name}/`以下)
directory = "external/"
# ファイル
# https or httpはURL先からダウンロードする
# ダウンロードしたものがzipなら展開される (rootなどは維持される)
files = ["https://example.com/archive.zip", "../*.png"]

# ドキュメント (`${name}/`以下に設置)
[[release.contents.assets.documents]]
# ファイル名 (`${directory}/${name}`に結合される)
filename = "readme.txt"
# 内容
content = """
This archive contains additional resources.
"""
```

</details>

## コマンド

コマンドは以下の形式で`astra.toml`を認識する場所で実行する．

```pwsh
astra <command> [options]
```

`astra.toml`を認識する場所は以下のいずれかである．

- `./astra.toml`
- `./.config/astra.toml`
- `./.astra/astra.toml`

使用可能なコマンドを以下に示す．`-h`，`--help`でヘルプを表示可能．

### `init`

`astra.toml`設定ファイルと`.editorconfig`ファイルを作成する．

すでに`astra.toml`が存在する場合使用できない．

<details>
<summary>生成される.editorconfig</summary>

```toml
root = true

[*]  
charset = utf-8
end_of_line = lf
indent_style = space
indent_size = 4
insert_final_newline = true
trim_trailing_whitespace = true

```

</details>

#### 使用方法

```pwsh
astra init [options]
```

#### オプション

- `<target>`

出力先ディレクトリを指定する．(デフォルト: `.`)

### `build`

設定ファイルに基づいてプロジェクトをビルドする．

#### 使用方法

```pwsh
astra build [options]
```

#### オプション

- `<build>`

ビルドディレクトリを指定する．(デフォルト: `./build`)

- `-c <config>`，`--config <config>`

ビルド設定 (ReleaseまたはDebug) を指定する．(デフォルト: Debug)

- `-v <version>`，`--version <version>`

プロジェクトバージョンを指定する．これは設定ファイルより優先される．

### `release`

プロジェクトをリリース設定でビルド後，リリース用にパッケージ化する．

#### 使用方法

```pwsh
astra release [options]
```

#### オプション

- `<target>`

出力先ディレクトリを指定する．(デフォルト: `./release`)

このディレクトリ内にビルドディレクトリを新たに作成する．

- `-v <version>`，`--version <version>`

プロジェクトバージョンを指定する．これは設定ファイルより優先される．

### `install`

リリース時に`Plugin/`や`Script/`等に置かれるものを設置する．

インストールにはビルドディレクトリが必要．(`build`コマンドで生成したキャッシュが必要)

#### 使用方法

```pwsh
astra install [options]
```

#### オプション

- `<target>`

設置先ディレクトリを指定する．(デフォルト: `%ProgramData%/aviutl2`)

> [!NOTE]
> AviUtl ExEdit2の認識する場所以外設置できない．

- `-b <directory>`，`--build <directory>`

ビルドディレクトリを指定する．(デフォルト: `./build`)

- `-e`，`--editable`

コピーではなくシンボリックリンクを設置する．

> [!IMPORTANT]
> Windowsでシンボリックリンクを設置するためには，開発者モードを有効にして標準ユーザー権限でシンボリックリンクを作成可能にする必要がある．

### `uninstall`

インストールしたものをアンインストールする．

アンインストールにはビルドディレクトリが必要．(`install`コマンドで生成したキャッシュが必要)

#### 使用方法

```pwsh
astra uninstall [options]
```

#### オプション

- `-b <directory>`，`--build <directory>`

ビルドディレクトリを指定する．(デフォルト: `./build`)

### `clean`

アンインストール実行後，ビルドディレクトリを削除する．

#### 使用方法

```pwsh
astra clean [options]
```

#### オプション

- `<build>`

ビルドディレクトリを指定する．(デフォルト: `./build`)

### `schema`

`astra.toml`設定ファイルのためのJSONスキーマを生成する．

#### 使用方法

```pwsh
astra schema [options]
```

#### オプション

- `<target>`

スキーマファイルを出力するディレクトリを指定する．(デフォルト: None)

指定されない場合標準出力に出力される．

## ライセンス

本プログラムのライセンスは[LICENSE](./LICENSE)を参照されたい．

また，本プログラムが利用するサードパーティ製ライブラリ等のライセンス情報は[THIRD_PARTY_LICENSES](./THIRD_PARTY_LICENSES.md)に記載している．

## 更新履歴

[CHANGELOG](./CHANGELOG.md)を参照されたい．
