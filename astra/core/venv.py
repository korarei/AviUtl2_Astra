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

    dst /= ".venv/aviutl2"
    shutil.rmtree(dst, ignore_errors=True)
    dst.mkdir(parents=True, exist_ok=True)
    dst = dst.resolve()

    if version is None:
        url = "https://raw.githubusercontent.com/Neosku/aviutl2-catalog-data/main/index.json"
        identifier = "Kenkun.AviUtlExEdit2"
        with cast(BinaryIO, urlopen(url)) as response:
            content = response.read().decode("utf-8")
            data = cast(list[dict[str, str]], json.loads(content))
            if item := next((i for i in data if i["id"] == identifier), None):
                version = item["latest-version"]

    url = f"https://spring-fragrance.mints.ne.jp/aviutl/aviutl2{version}.zip"
    download(url, dst)

    (dst / "data").mkdir(parents=True, exist_ok=True)

    logger.info("Setting up virtual environment completed")
