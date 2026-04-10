import json
import shutil
from logging import getLogger
from pathlib import Path
from typing import BinaryIO, cast
from urllib.request import urlopen

from astra.core.utils import download


logger = getLogger(__name__)


def venv(dst: Path, version: str | None) -> None:
    logger.info("Setting up virtual environment to: %s", dst)

    dst /= ".venv"
    shutil.rmtree(dst, ignore_errors=True)

    aviutl2 = dst / "aviutl2"
    aviutl2.mkdir(parents=True, exist_ok=True)
    aviutl2 = aviutl2.resolve()

    if version is None:
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

    url = f"https://spring-fragrance.mints.ne.jp/aviutl/aviutl2{version}.zip"

    try:
        download(url, aviutl2)
    except Exception:
        shutil.rmtree(dst, ignore_errors=True)
        raise

    (aviutl2 / "data").mkdir(parents=True, exist_ok=True)
    _ = (dst / ".gitignore").write_text("*", encoding="utf-8", newline="\n")

    logger.info("Setting up virtual environment completed")
