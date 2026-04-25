import shutil
from logging import getLogger
from os import symlink
from pathlib import Path

from astra._internal.config import Extension, Install


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


def install(dst: Path, cfg: Install, editable: bool = False) -> Extension:
    if not dst.is_dir():
        raise NotADirectoryError(f"Not a directory: {dst}")

    logger.info("Installing to: %s", dst)

    extensions: list[str] = []

    for extension in cfg.extensions:
        target = dst / extension.directory
        target.mkdir(parents=True, exist_ok=True)
        for file in extension.files:
            path = _copy_file(file, target, editable)
            if path is not None:
                extensions.append(str(path))

    logger.info("Install completed: %d files", len(extensions))

    return Extension(extensions)


def uninstall(extension: Extension) -> None:
    logger.info("Uninstalling from AviUtl2 ExEdit2")

    data = (
        "plugin",
        "script",
        "language",
        "alias",
        "default",
        "figure",
        "preset",
        "transition",
        "data",  # 一応
        "aviutl2",
    )

    for path in extension.files:
        path = Path(path)
        if path.exists() or path.is_symlink():
            path.unlink()

        while True:
            parent = path.parent
            if parent == path:
                break

            path = parent

            if not path.is_dir():
                break

            if path.name.lower() in data:
                break

            try:
                path.rmdir()
            except Exception:
                break

    logger.info("Uninstall completed")
