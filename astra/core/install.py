import shutil
from logging import getLogger
from os import symlink
from pathlib import Path

from astra.core.config import Install

logger = getLogger(__name__)


def _copy_file(src: Path, dst: Path, editable: bool) -> Path | None:
    if not dst.is_dir():
        raise NotADirectoryError(f"Not a directory: {dst}")

    if not src.is_file():
        logger.warning("File not found, skipping: %s", src)
        return None

    src = src.resolve()
    path = (dst / src.name).resolve()

    if path.exists() or path.is_symlink():
        path.unlink()

    if editable:
        symlink(src, path)
    else:
        _ = shutil.copy2(src, path)

    return path


def install(dst: Path, cfg: Install, editable: bool = False) -> list[Path]:
    logger.info("Installing to: %s", dst)

    dst.mkdir(parents=True, exist_ok=True)
    dst = dst.resolve()

    installations: list[Path] = []

    for extension in cfg.extensions:
        target = dst / extension.directory
        target.mkdir(parents=True, exist_ok=True)
        for file in extension.files:
            path = _copy_file(file, target, editable)
            if path:
                installations.append(path)

    logger.info("Install completed: %d files", len(installations))

    return installations


def uninstall(installations: list[Path]) -> None:
    logger.info("Uninstalling from AviUtl2 ExEdit2")

    for path in installations:
        if path.exists() or path.is_symlink():
            path.unlink()

            if path.parent.name.lower() not in ("plugin", "script"):
                answer = input(f"Remove directory '{path.parent}'? [y/N]: ")
                if answer.lower() in ("y", "yes"):
                    _ = shutil.rmtree(path.parent, ignore_errors=True)

    logger.info("Uninstall completed")
