import shutil
from logging import getLogger
from os import symlink
from pathlib import Path

from astra._internal.config import Extension, Install


logger = getLogger(__name__)


def _copy_file(src: Path, dst: Path, editable: bool) -> Path | None:
    if not dst.is_dir():
        raise NotADirectoryError(f"'{dst}' is not a directory")

    src = src.resolve()
    if not src.is_file():
        logger.warning(f"'{src}' is not found")
        return None

    path = (dst / src.name).resolve()

    if path.exists() or path.is_symlink():
        path.unlink()

    if editable:
        symlink(src, path)
    else:
        _ = shutil.copy2(src, path)

    return path


def install(dst: Path, cfg: Install, editable: bool = False) -> Extension:
    if dst.is_file():
        raise NotADirectoryError(f"'{dst}' is not a directory")

    dst = dst.resolve()
    dst.mkdir(parents=True, exist_ok=True)

    logger.info(f"Installing to '{dst}' ({'symbolic link' if editable else 'copy'})")

    extensions: list[str] = []

    for extension in cfg.extensions:
        target = dst / extension.directory
        target.mkdir(parents=True, exist_ok=True)
        for file in extension.files:
            path = _copy_file(file, target, editable)
            if path is not None:
                extensions.append(str(path))

    logger.info(f"{len(extensions)} file(s) installed")

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

        parent = path.parent

        while True:
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

            parent = path.parent
