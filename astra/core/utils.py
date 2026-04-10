import re
from io import BytesIO
from logging import getLogger
from pathlib import Path
from typing import BinaryIO, cast
from urllib.parse import urlparse
from urllib.request import urlopen
from zipfile import ZipFile, is_zipfile


_VAR_PATTERN = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")

logger = getLogger(__name__)


def find_config() -> Path:
    candidates = [
        Path("astra.toml"),
        Path("astra.tml"),
        Path(".config/astra.toml"),
        Path(".config/astra.tml"),
        Path(".astra/astra.toml"),
        Path(".astra/astra.tml"),
    ]

    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()

    raise FileNotFoundError("astra.toml not found.")


def expand_variables(text: str, variables: dict[str, str]) -> str:
    def _replacer(match: re.Match[str]) -> str:
        key = match.group(1)
        if val := variables.get(key):
            return val
        else:
            logger.warning("Variable %s not found", key)
            return match.group(0)

    return _VAR_PATTERN.sub(_replacer, text)


def download(url: str, dst: Path) -> None:
    if not dst.is_dir():
        raise NotADirectoryError(f"Not a directory: {dst}")

    logger.info("Downloading: %s", url)

    with cast(BinaryIO, urlopen(url)) as response:
        data = BytesIO(response.read())

    _ = data.seek(0)
    if is_zipfile(data):
        _ = data.seek(0)
        with ZipFile(data) as zf:
            zf.extractall(dst)

        logger.info("Extracted zip to: %s", dst)
    else:
        name = Path(urlparse(url).path).name or "download"
        path = dst / name
        _ = data.seek(0)
        _ = path.write_bytes(data.read())
        logger.info("Saved file: %s", path)
