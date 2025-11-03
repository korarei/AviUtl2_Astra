import json
import logging
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jsonschema import validate, ValidationError

from astra.core import config
from astra.core import utils


@dataclass
class Install():
    clean: bool
    directory: Path | None
    files: list[Path]


def load(cfg: Path, dst: Path | None) -> Install:
    schema: dict[str, Any] = config.get_schema(["project", "build", "install"])

    try:
        with open(cfg, "r", encoding="utf-8") as f:
            data: dict[str, Any] = json.load(f)
    except FileNotFoundError:
        raise FileNotFoundError(f"File not found: {cfg}")
    except json.JSONDecodeError:
        raise ValueError(f"Invalid JSON file: {cfg}")

    try:
        validate(data, schema)
    except ValidationError as e:
        raise ValueError(f"Invalid config file: {e.message}")

    root: Path = cfg.parent
    proj_name: str = data["project"]["name"]
    build: Path = root / Path(data["build"]["directory"])
    files: list[Path] = []
    for script in data["build"]["scripts"]:
        name: str = script.get("name", proj_name)
        suffix: str = script["suffix"]
        files.append(build / f"{name}{suffix}")

    for module in data["build"].get("modules", []):
        path: Path = root / module["path"]
        files.extend(path.parent.glob(path.name))

    directory: str | None = data["install"].get("directory", None)

    return Install(
        data["install"].get("clean", False),
        dst or (Path(directory).resolve() if directory else None),
        files
    )


def install(path: Path, dst: Path | None = None) -> None:
    data: Install = load(path, dst)

    if data.directory is None:
        logging.warning("installation failed: target directory not found.")
        return

    if data.clean and data.directory.exists() and data.directory.is_dir():
        shutil.rmtree(data.directory)

    utils.copy(data.files, data.directory)
