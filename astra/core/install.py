import json
import logging
import os
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


def install(cfg: Path, dst: Path | None, editable: bool) -> None:
    data: Install = load(cfg, dst)

    if data.directory is None:
        logging.warning("installation failed: target directory not found.")
        return

    if data.clean and data.directory.exists() and data.directory.is_dir():
        if data.directory.name.lower() == "script":
            answer: str = input(
                f"The directory '{data.directory}' seems important. Delete it? [y/N]: ").strip().lower()
            if answer == "y":
                shutil.rmtree(data.directory)
        else:
            shutil.rmtree(data.directory)

    if editable:
        data.directory.mkdir(parents=True, exist_ok=True)

        for path in data.files:
            if path.is_dir():
                os.symlink(path, data.directory / path.name, True)
            else:
                os.symlink(path, data.directory / path.name)
    else:
        utils.copy(data.files, data.directory)


def uninstall(cfg: Path, dst: Path | None) -> None:
    data: Install = load(cfg, dst)

    if data.directory is None:
        logging.warning("uninstallation failed: target directory not found.")
        return

    if data.directory.name.lower() == "script":
        for file in data.files:
            path: Path = data.directory / file.name
            if path.exists():
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
    elif data.directory.exists():
        shutil.rmtree(data.directory)
