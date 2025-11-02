import json
import re
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jsonschema import validate, ValidationError

from astra.core import config


@dataclass
class Script():
    source: Path
    target: Path
    includes: list[Path]
    variables: dict[str, str]
    newline: str


@dataclass
class Build():
    clean: bool
    directory: Path
    scripts: list[Script]


def load(path: Path, version: str | None) -> Build:
    schema: dict[str, Any] = config.get_schema(["project", "build"])

    try:
        with open(path, encoding="utf-8") as f:
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
    proj_ver: str | None = data["project"].get("version", None)
    ver: str | None = version or (proj_ver if proj_ver else None)
    build: Path = root / Path(data["build"]["directory"])

    scripts: list[Script] = []
    for script in data["build"]["scripts"]:
        name: str = script.get("name", proj_name)
        suffix: str = script["suffix"]
        tag: str = script["source"].get("tag", ".in")
        includes: list[str] = script["source"].get("include_directories", [])
        variables: dict[str, str] = dict(script["source"].get("variables", {}))
        variables["PROJECT_NAME"] = proj_name
        variables["SCRIPT_NAME"] = name

        if ver:
            variables["VERSION"] = ver

        if "author" in data["project"]:
            variables["AUTHOR"] = data["project"]["author"]

        scripts.append(Script(
            root / f"{name}{tag}{suffix}",
            build / f"{name}{suffix}",
            [root / Path(p) for p in includes],
            variables,
            script.get("newline", "\r\n")
        ))

    return Build(
        data["build"].get("clean", True),
        build,
        scripts
    )


def run(scripts: list[Script]) -> None:
    var_re: re.Pattern[str] = re.compile(r"\$\{([a-zA-Z0-9_]+)\}")
    include_re: re.Pattern[str] = re.compile(
        r'^\s*--#include\s+(?:"(.*)"|<(.*)>).*$', re.MULTILINE)

    for script in scripts:
        def repl_var(m: re.Match[str]) -> str:
            key: str = m.group(1)
            return script.variables.get(key, m.group(0))

        def repl_include(m: re.Match[str]) -> str:
            quoted: str | None = m.group(1)
            angled: str | None = m.group(2)
            libs: list[Path] = []

            if quoted:
                libs.append(script.source.parent / quoted)
                libs.extend(dir / quoted for dir in script.includes)
            elif angled:
                libs.extend(dir / angled for dir in script.includes)

            for lib in libs:
                if lib.exists() and lib.is_file() and lib.resolve() != script.source:
                    try:
                        return lib.read_text(encoding="utf-8")
                    except Exception:
                        break

            return m.group(0)

        content: str = script.source.read_text(encoding="utf-8")
        content = re.sub(var_re, repl_var, content)
        content = re.sub(include_re, repl_include, content)

        script.target.parent.mkdir(exist_ok=True, parents=True)
        script.target.write_text(
            content, encoding="utf-8", newline=script.newline)


def build(path: Path, version: str | None) -> None:
    data: Build = load(path, version)

    if data.clean and data.directory.exists() and data.directory.is_dir():
        shutil.rmtree(data.directory)

    data.directory.mkdir(parents=True, exist_ok=True)

    run(data.scripts)
