import shutil
from pathlib import Path


def copy(src: list[Path], dst: Path) -> None:
    dst.mkdir(parents=True, exist_ok=True)

    for path in filter(Path.exists, src):
        target = dst / path.name
        if path.is_dir():
            shutil.copytree(path, target, dirs_exist_ok=True)
        else:
            shutil.copy2(path, target)
