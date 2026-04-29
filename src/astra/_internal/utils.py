import importlib.metadata as metadata
import re
from io import BytesIO
from logging import getLogger
from pathlib import Path
from typing import BinaryIO, cast
from urllib.parse import urlparse
from urllib.request import urlopen
from zipfile import ZipFile, is_zipfile


logger = getLogger(__name__)

_VAR_PATTERN = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")


def fetch_version() -> str:
    try:
        return metadata.version("astra")
    except metadata.PackageNotFoundError:
        return "unknown"


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
            candidate = candidate.resolve()
            logger.info(f"Using astra.toml at {candidate}")
            return candidate

    raise FileNotFoundError("astra.toml is not found")


def expand_variables(text: str, variables: dict[str, str]) -> str:
    def _replacer(match: re.Match[str]) -> str:
        key = match.group(1)
        if (val := variables.get(key)) is not None:
            return val
        else:
            logger.warning(f"'{key}' has no match in variables")
            return match.group(0)

    return _VAR_PATTERN.sub(_replacer, text)


def resolve_glob(root: Path, pattern: str) -> list[Path]:
    matched = sorted(root.glob(pattern))
    if len(matched) == 0:
        raise FileNotFoundError(f"'{pattern}' in '{root.resolve()}' has no match")

    return matched


def download(url: str, dst: Path) -> None:
    if not dst.is_dir():
        raise NotADirectoryError(f"'{dst}' is not a directory")

    logger.info(f"Downloading from '{url}'")

    with cast(BinaryIO, urlopen(url)) as response:
        data = BytesIO(response.read())

    _ = data.seek(0)
    if is_zipfile(data):
        logger.info(f"Extracting to '{dst}'")

        _ = data.seek(0)
        with ZipFile(data) as zf:
            zf.extractall(dst)
    else:
        name = Path(urlparse(url).path).name or "download"
        path = dst / name

        logger.info(f"Saving to '{path}'")

        _ = data.seek(0)
        _ = path.write_bytes(data.read())
