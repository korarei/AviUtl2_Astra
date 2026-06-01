import json
import re
import shutil
from logging import getLogger
from pathlib import Path
from typing import BinaryIO, cast
from urllib.request import urlopen

from astra._internal.config import Project
from astra._internal.utils import download


logger = getLogger(__name__)

# $script:NAME と $script:VENV は後で追加すること
_ACTIVATE_PS1 = r"""
$cmd = Get-Command astra -CommandType Application -ErrorAction Stop |
    Select-Object -First 1
$script:ASTRA = $cmd.Source

function global:deactivate([switch] $NonDestructive) {
    if (Test-Path function:_old_prompt) {
        $function:prompt = $function:_old_prompt
        Remove-Item function:\_old_prompt
    }

    if (Test-Path function:_old_astra) {
        $function:astra = $function:_old_astra
        Remove-Item function:\_old_astra
    } else {
        Remove-Item function:astra -ErrorAction SilentlyContinue
    }

    if (!$NonDestructive) {
        Remove-Item function:deactivate
    }
}

deactivate -NonDestructive

$function:_old_prompt = $function:prompt

if ((Get-Command astra).CommandType -eq "Function") {
    $function:_old_astra = (Get-Command astra).ScriptBlock
}

function global:prompt {
    $prev = & $function:_old_prompt
    "(" + $script:NAME + ") " + $prev
}

function global:astra {
    if ($args -contains "--venv") {
        & $script:ASTRA @args
    } else {
        & $script:ASTRA --venv "$script:VENV" @args
    }
}
"""

_LAYOUT = r"""[Window.edit]
left=0.259636
group=0
area=3
top=0.546680
hide=0
right=1.000000
bottom=1.000000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.explorer:1]
left=0.744331
group=0
area=4
top=0.000000
hide=1
right=1.000000
bottom=0.434546
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.explorer:2]
left=0.000000
group=0
area=0
top=0.000000
hide=1
right=0.200000
bottom=0.700000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.explorer:3]
left=0.000000
group=0
area=0
top=0.000000
hide=1
right=0.200000
bottom=0.700000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.explorer:4]
left=0.000000
group=0
area=0
top=0.000000
hide=1
right=0.200000
bottom=0.700000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.graph.histogram]
left=0.178228
group=2
area=4
top=0.000000
hide=1
right=0.448054
bottom=0.436047
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.layer.setting]
left=0.726006
group=0
area=3
top=0.436047
hide=1
right=1.000000
bottom=1.000000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.log]
left=0.347675
group=0
area=2
top=0.000000
hide=0
right=1.000000
bottom=0.189726
floating=0
x=2563
y=670
w=1175
h=723
zoom=0
[Window.object.list]
left=0.178852
group=2
area=3
top=0.436047
hide=1
right=0.265607
bottom=1.000000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.preview]
left=0.447744
group=0
area=4
top=0.189726
hide=0
right=1.000000
bottom=0.546680
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.scene.preview]
left=0.000000
group=2
area=0
top=0.740951
hide=1
right=0.180177
bottom=1.000000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.scene.list]
left=0.179716
group=1
area=3
top=0.546680
hide=0
right=0.259636
bottom=1.000000
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.setting]
left=0.000000
group=0
area=0
top=0.000000
hide=0
right=0.179716
bottom=0.740951
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.setting.color]
left=0.000000
group=0
area=0
top=0.523500
hide=1
right=0.178852
bottom=0.816976
floating=1
x=499
y=777
w=451
h=400
zoom=0
[Window.setting.text]
left=0.000000
group=1
area=0
top=0.740951
hide=0
right=0.179716
bottom=1.000000
floating=0
x=637
y=1000
w=451
h=268
zoom=0
[Window.setting.time]
left=0.179716
group=1
area=4
top=0.189726
hide=0
right=0.447744
bottom=0.546680
floating=0
x=0
y=0
w=0
h=0
zoom=0
[Window.system.info]
left=0.179716
group=0
area=2
top=0.000000
hide=0
right=0.347675
bottom=0.189726
floating=0
x=1103
y=732
w=352
h=135
zoom=0
"""

_CONFIG = rf"""[Edit]
EditResume=1
AutoBackup=1
AutoBackupInterval=1
AutoBackupFileMax=100
[Logger]
ViewLogLevel=1
FileLogLevel=4
DebuggerLogLevel=1
MaxViewLine=10000
MaxFileNum=20
{_LAYOUT}
"""


def venv(dst: Path, cfg: Project, version: str | None) -> None:
    dst = dst.resolve()

    if dst.exists():
        if input(f"'{dst}' already exists, overwrite? (y/N): ").lower() != "y":
            logger.info("Cancelled")
            return

        if dst.is_file() or dst.is_symlink():
            dst.unlink()
        elif (dst / "aviutl2").is_dir():
            shutil.rmtree(dst, ignore_errors=True)
        else:
            logger.error(f"'{dst}' is not an astra venv directory")
            return

    logger.info(f"Creating virtual environment at '{dst}'")

    aviutl2 = dst / "aviutl2"
    aviutl2.mkdir(parents=True, exist_ok=True)

    if version is None or version.lower() == "latest":
        logger.info("Connecting to the AviUtl2 Catalog data repository")
        url = "https://raw.githubusercontent.com/Neosku/aviutl2-catalog-data/main/index.json"
        identifier = "Kenkun.AviUtlExEdit2"

        try:
            with cast(BinaryIO, urlopen(url)) as response:
                content = response.read().decode("utf-8")
        except Exception:
            shutil.rmtree(dst, ignore_errors=True)
            raise

        data = cast(list[dict[str, str]], json.loads(content))
        if item := next((i for i in data if i["id"] == identifier), None):
            version = item["latest-version"]

    if version is None:
        shutil.rmtree(dst, ignore_errors=True)
        raise ValueError("Could not determine AviUtl ExEdit2 version")

    version = version.lower()

    logger.info(f"AviUtl ExEdit2 version: {version}")

    if cfg.requires_aviutl2 is not None:
        if (m := re.search(r"(\d+)([a-z])?", version)) is not None:
            revision = int(m.group(1))
            suffix = m.group(2) or "`"
            v = 2000000 + (revision * 100) + ord(suffix) - ord("a") + 1
            if int(cfg.requires_aviutl2) > v:
                logger.warning(f"'{version}' is not compatible with '{cfg.name}'")

    url = f"https://spring-fragrance.mints.ne.jp/aviutl/aviutl2{version}.zip"

    try:
        download(url, aviutl2)
    except Exception:
        shutil.rmtree(dst, ignore_errors=True)
        raise

    data = aviutl2 / "data"
    default = data / "Default"
    default.mkdir(parents=True, exist_ok=True)

    _ = (data / "aviutl2.ini").write_text(_CONFIG, encoding="utf-8", newline="\r\n")
    _ = (default / "Debug.layout").write_text(_LAYOUT, encoding="utf-8", newline="\r\n")

    _ = (dst / ".gitignore").write_text("*", encoding="utf-8", newline="\n")

    scripts = dst / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)

    _ = (scripts / "activate.ps1").write_text(
        f'$script:NAME = "{cfg.name}"\n$script:VENV = "{str(dst)}"\n{_ACTIVATE_PS1}',
        encoding="utf-8",
        newline="\n",
    )

    print("\033[36mTo activate this environment:\033[0m")
    print("  PowerShell:")
    print(f"\033[32m    . {str(scripts)}\\activate.ps1\033[0m")
