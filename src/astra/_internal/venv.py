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
        if (m := re.search(r"(\d+)([a-z])", version)) is not None:
            v = 2000000 + (int(m.group(1)) * 100) + ord(m.group(2)) - ord("a") + 1
            if int(cfg.requires_aviutl2) > v:
                logger.warning(f"'{version}' is not compatible with '{cfg.name}'")

    url = f"https://spring-fragrance.mints.ne.jp/aviutl/aviutl2{version}.zip"

    try:
        download(url, aviutl2)
    except Exception:
        shutil.rmtree(dst, ignore_errors=True)
        raise

    (aviutl2 / "data").mkdir(parents=True, exist_ok=True)
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
