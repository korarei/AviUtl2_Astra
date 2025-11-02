import json
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jsonschema import validate, ValidationError

from astra.core import config


@dataclass
class Install():
    clean: bool
    directory: Path | None
    files: list[Path]


def load(path: Path, dst: Path | None) -> Install:
    schema: dict[str, Any] = config.get_schema(["project", "build", "install"])

    try:
        with open(path, "r", encoding="utf-8") as f:
            data: dict[str, Any] = json.load(f)
    except FileNotFoundError:
        raise FileNotFoundError(f"File not found: {path}")
    except json.JSONDecodeError:
        raise ValueError(f"Invalid JSON file: {path}")

    try:
        validate(data, schema)
    except ValidationError as e:
        raise ValueError(f"Invalid config file: {e.message}")

    root: Path = path.parent
    proj_name: str = data["project"]["name"]
    build: Path = root / Path(data["build"]["directory"])
    files: list[Path] = []
    for script in data["build"]["scripts"]:
        name: str = script.get("name", proj_name)
        suffix: str = script["suffix"]
        files.append(build / f"{name}{suffix}")

    directory: str | None = data["install"].get("directory", None)

    return Install(
        data["install"].get("clean", True),
        dst or (Path(directory).resolve() if directory else None),
        files
    )


def install(path: Path, dst: Path | None = None) -> None:
    data: Install = load(path, dst)

    if data.directory is None:
        return

    if data.clean and data.directory.exists() and data.directory.is_dir():
        shutil.rmtree(data.directory)

    data.directory.mkdir(parents=True, exist_ok=True)

    for file in data.files:
        if file.exists() and file.is_file():
            shutil.copy2(file, data.directory)
