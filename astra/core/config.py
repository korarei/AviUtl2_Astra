from __future__ import annotations

import json
import tomllib
from collections.abc import ItemsView
from dataclasses import dataclass, field
from pathlib import Path
from typing import TypeVar, cast, overload

from astra.core.utils import expand_variables

T = TypeVar("T", str, bool)


def _exp(text: str, env: dict[str, str]) -> str:
    return expand_variables(text, env)


def _exp_opt(text: str | None, env: dict[str, str]) -> str | None:
    return expand_variables(text, env) if text else None


def _exp_list(items: list[str], env: dict[str, str]) -> list[str]:
    return [expand_variables(s, env) for s in items]


class Json:
    _data: dict[str, object]

    def __init__(self, data: dict[str, object] | Path | None = None) -> None:
        if isinstance(data, Path):
            try:
                with open(data, encoding="utf-8") as f:
                    self._data = json.load(f)
            except FileNotFoundError as e:
                raise FileNotFoundError(f"JSON file not found: {data}") from e
            except json.JSONDecodeError as e:
                raise ValueError(f"Invalid JSON format at {data}: {e}") from e
            except OSError as e:
                raise OSError(f"Error reading JSON file at {data}: {e}") from e
        else:
            self._data = data if data is not None else {}

    @overload
    def string(self, key: str) -> str | None: ...

    @overload
    def string(self, key: str, default: str) -> str: ...

    def string(self, key: str, default: str | None = None) -> str | None:
        v = self._data.get(key)
        return v if type(v) is str else default

    @overload
    def object(self, key: str) -> Json | None: ...

    @overload
    def object(self, key: str, default: Json) -> Json: ...

    def object(self, key: str, default: Json | None = None) -> Json | None:
        v = self._data.get(key)
        if type(v) is dict:
            return Json(cast("dict[str, object]", v))

        if default is not None:
            self._data[key] = default._data
            return default

        return None

    @overload
    def dict_of(self, key: str, cls: type[T]) -> dict[str, T] | None: ...

    @overload
    def dict_of(
        self, key: str, cls: type[T], default: dict[str, T]
    ) -> dict[str, T]: ...

    def dict_of(
        self, key: str, cls: type[T], default: dict[str, T] | None = None
    ) -> dict[str, T] | None:
        v = self._data.get(key)
        if type(v) is dict:
            return {
                k: val
                for k, val in cast("dict[str, object]", v).items()
                if isinstance(val, cls)
            }

        return default

    @overload
    def objects(self, key: str) -> list[Json] | None: ...

    @overload
    def objects(self, key: str, default: list[Json]) -> list[Json]: ...

    def objects(
        self, key: str, default: list[Json] | None = None
    ) -> list[Json] | None:
        v = self._data.get(key)
        if type(v) is list:
            return [
                Json(cast("dict[str, object]", i))
                for i in cast("list[object]", v)
                if type(i) is dict
            ]

        if default is not None:
            self._data[key] = [i._data for i in default]
            return default

        return None

    @overload
    def list_of(self, key: str, cls: type[T]) -> list[T] | None: ...

    @overload
    def list_of(self, key: str, cls: type[T], default: list[T]) -> list[T]: ...

    def list_of(
        self, key: str, cls: type[T], default: list[T] | None = None
    ) -> list[T] | None:
        v = self._data.get(key)
        if type(v) is list:
            return [i for i in cast("list[object]", v) if isinstance(i, cls)]

        return default

    def items(self) -> ItemsView[str, object]:
        return self._data.items()

    @overload
    def set(self, key: str, value: T | Json) -> None: ...

    @overload
    def set(self, key: str, value: dict[str, T] | dict[str, Json]) -> None: ...

    @overload
    def set(self, key: str, value: list[T] | list[Json]) -> None: ...

    def set(self, key: str, value: object) -> None:
        if type(value) is Json:
            self._data[key] = value._data
        elif type(value) is dict:
            self._data[key] = {
                k: v._data if type(v) is Json else v
                for k, v in cast("dict[str, object]", value).items()
            }
        elif type(value) is list:
            self._data[key] = [
                v._data if type(v) is Json else v
                for v in cast("list[object]", value)
            ]
        else:
            self._data[key] = value

    def save(self, path: Path, indent: int = 4) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self._data, f, ensure_ascii=False, indent=indent)

    def dumps(self, indent: int = 4) -> str:
        return json.dumps(self._data, ensure_ascii=False, indent=indent)


class Toml:
    _data: dict[str, object]

    def __init__(self, data: dict[str, object] | Path) -> None:
        if isinstance(data, Path):
            try:
                with open(data, "rb") as f:
                    self._data = tomllib.load(f)
            except FileNotFoundError as e:
                raise FileNotFoundError(f"TOML file not found: {data}") from e
            except tomllib.TOMLDecodeError as e:
                raise ValueError(f"Invalid TOML format at {data}: {e}") from e
            except OSError as e:
                raise OSError(f"Error reading TOML file at {data}: {e}") from e
        else:
            self._data = data

    @overload
    def string(self, key: str) -> str | None: ...

    @overload
    def string(self, key: str, default: str) -> str: ...

    def string(self, key: str, default: str | None = None) -> str | None:
        v = self._data.get(key)
        return v if type(v) is str else default

    @overload
    def boolean(self, key: str) -> bool | None: ...

    @overload
    def boolean(self, key: str, default: bool) -> bool: ...

    def boolean(self, key: str, default: bool | None = None) -> bool | None:
        v = self._data.get(key)
        return v if type(v) is bool else default

    @overload
    def table(self, key: str) -> Toml | None: ...

    @overload
    def table(self, key: str, default: Toml) -> Toml: ...

    def table(self, key: str, default: Toml | None = None) -> Toml | None:
        v = self._data.get(key)
        if type(v) is dict:
            return Toml(cast("dict[str, object]", v))

        return default

    @overload
    def dict_of(self, key: str, cls: type[T]) -> dict[str, T] | None: ...

    @overload
    def dict_of(
        self, key: str, cls: type[T], default: dict[str, T]
    ) -> dict[str, T]: ...

    def dict_of(
        self, key: str, cls: type[T], default: dict[str, T] | None = None
    ) -> dict[str, T] | None:
        v = self._data.get(key)
        if type(v) is dict:
            return {
                k: val
                for k, val in cast("dict[str, object]", v).items()
                if isinstance(val, cls)
            }

        return default

    @overload
    def tables(self, key: str) -> list[Toml] | None: ...

    @overload
    def tables(self, key: str, default: list[Toml]) -> list[Toml]: ...

    def tables(
        self, key: str, default: list[Toml] | None = None
    ) -> list[Toml] | None:
        v = self._data.get(key)
        if type(v) is list:
            return [
                Toml(cast("dict[str, object]", i))
                for i in cast("list[object]", v)
                if type(i) is dict
            ]

        return default

    @overload
    def list_of(self, key: str, cls: type[T]) -> list[T] | None: ...

    @overload
    def list_of(self, key: str, cls: type[T], default: list[T]) -> list[T]: ...

    def list_of(
        self, key: str, cls: type[T], default: list[T] | None = None
    ) -> list[T] | None:
        v = self._data.get(key)
        if type(v) is list:
            return [i for i in cast("list[object]", v) if isinstance(i, cls)]

        return default

    def items(self) -> ItemsView[str, object]:
        return self._data.items()


@dataclass(frozen=True)
class Project:
    name: str
    version: str | None = None
    author: str | None = None
    requires_aviutl2: str | None = None
    variables: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class Command:
    commands: list[str] = field(default_factory=list)
    artifacts: list[str] = field(default_factory=list)


@dataclass(frozen=True)
class Plugin:
    id: str
    release: Command = field(default_factory=Command)
    debug: Command = field(default_factory=Command)


@dataclass(frozen=True)
class ScriptSource:
    files: list[Path]
    variables: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class Script:
    id: str
    name: str
    newline: str = "\r\n"
    include_directories: list[Path] = field(default_factory=list)
    variables: dict[str, str] = field(default_factory=dict)
    sources: list[ScriptSource] = field(default_factory=list)
    artifacts: list[Path] = field(default_factory=list)


@dataclass(frozen=True)
class Build:
    root: Path
    project: Project
    plugins: list[Plugin] = field(default_factory=list)
    scripts: list[Script] = field(default_factory=list)


@dataclass(frozen=True)
class ReleasePackage:
    filename: str
    name: str
    id: str
    information: str | None = None
    version: str | None = None
    author: str | None = None
    license: str | None = None
    description: str | None = None


@dataclass(frozen=True)
class AssetSource:
    directory: str = ""
    files: list[Path | str] = field(default_factory=list)


@dataclass(frozen=True)
class AssetDocument:
    filename: str
    content: str = ""


@dataclass(frozen=True)
class ReleaseAsset:
    name: str
    directory: str = ""
    sources: list[AssetSource] = field(default_factory=list)
    documents: list[AssetDocument] = field(default_factory=list)


@dataclass(frozen=True)
class ReleaseExtension:
    directory: str = ""
    files: list[Path] = field(default_factory=list)


@dataclass(frozen=True)
class ReleaseDocument(ReleaseExtension):
    pass


@dataclass(frozen=True)
class ReleaseContentContainer:
    extensions: list[ReleaseExtension] = field(default_factory=list)
    documents: list[ReleaseDocument] = field(default_factory=list)
    assets: list[ReleaseAsset] = field(default_factory=list)


@dataclass(frozen=True)
class Release:
    package: ReleasePackage
    contents: ReleaseContentContainer


@dataclass(frozen=True)
class Install:
    extensions: list[ReleaseExtension] = field(default_factory=list)


@dataclass(frozen=True)
class Artifact:
    plugin: dict[str, list[Path]] = field(default_factory=dict)
    script: dict[str, list[Path]] = field(default_factory=dict)


class Config:
    _data: Toml
    _root: Path
    _project: Project

    def __init__(self, path: Path, version: str | None = None) -> None:
        self._data = Toml(path)
        self._root = path.parent
        self._project = self._load_project(self._data, version)

    def load_build(self) -> Build:
        build = self._data.table("build")
        if build is None:
            raise ValueError("[build] section is required")

        plugins = self._load_plugins(build)
        scripts = self._load_scripts(build)

        return Build(self._root, self._project, plugins, scripts)

    def load_release(self, artifact: Artifact) -> Release:
        release = self._data.table("release")
        if release is None:
            raise ValueError("[release] section is required")

        package = self._load_release_package(release)
        contents = self._load_release_contents(release, artifact)

        return Release(package, contents)

    def load_install(self, artifact: Artifact) -> Install:
        release = self._data.table("release")
        if release is None:
            raise ValueError("[release] section is required")

        contents = release.table("contents")
        if contents is None:
            raise ValueError("[release.contents] section is required")

        return Install(self._load_release_extension(contents, artifact))

    @staticmethod
    def _load_project(data: Toml, version: str | None = None) -> Project:
        project = data.table("project")
        if project is None:
            raise ValueError("[project] section is required")

        name = project.string("name")
        if name is None:
            raise ValueError("[project].name is required")

        version = version or project.string("version")
        author = project.string("author")
        requires_aviutl2 = project.string("requires-aviutl2")
        variables = project.dict_of("variables", str, {})
        variables["PROJECT_NAME"] = name

        if version:
            variables["PROJECT_VERSION"] = version

        if author:
            variables["PROJECT_AUTHOR"] = author

        if requires_aviutl2:
            variables["PROJECT_REQUIRES_AVIUTL2"] = requires_aviutl2

        return Project(name, version, author, requires_aviutl2, variables)

    def _load_plugins(self, build: Toml) -> list[Plugin]:
        entries = build.tables("plugins")
        if entries is None:
            return []

        configs: list[Plugin] = []
        for entry in entries:
            if not entry.boolean("enabled", True):
                continue

            plugin_id = entry.string("id")
            if plugin_id is None:
                raise ValueError("build.plugins.id is required")

            env = {
                **self._project.variables,
                **entry.dict_of("variables", str, {}),
            }

            release = entry.table("release")
            if release is None:
                raise ValueError(
                    f"build.plugins.release is required for plugin {plugin_id}"
                )

            release_cmd = Command(
                _exp_list(release.list_of("commands", str, []), env),
                _exp_list(release.list_of("artifacts", str, []), env),
            )

            debug = entry.table("debug")
            if debug is not None:
                debug_cmd = Command(
                    _exp_list(debug.list_of("commands", str, []), env),
                    _exp_list(debug.list_of("artifacts", str, []), env),
                )
            else:
                debug_cmd = release_cmd

            configs.append(Plugin(plugin_id, release_cmd, debug_cmd))

        return configs

    def _load_scripts(self, build: Toml) -> list[Script]:
        entries = build.tables("scripts")
        if entries is None:
            return []

        configs: list[Script] = []
        for entry in entries:
            if not entry.boolean("enabled", True):
                continue

            script_id = entry.string("id")
            if script_id is None:
                raise ValueError("build.scripts.id is required")

            env = {
                **self._project.variables,
                **entry.dict_of("variables", str, {}),
            }

            name = _exp(entry.string("name", self._project.name), env)
            prefix = _exp(entry.string("prefix", ""), env)
            suffix = _exp(entry.string("suffix", ""), env)

            env["SCRIPT_NAME"] = name

            sources: list[ScriptSource] = []
            for src in entry.tables("sources", []):
                file = src.string("file")
                if file is None:
                    raise ValueError("build.scripts.sources.file is required")

                sources.append(
                    ScriptSource(
                        self._resolve_glob(file, env),
                        {
                            k: v
                            for k, v in src.items()
                            if isinstance(v, str) and k != "file"
                        },
                    )
                )

            includes: list[Path] = []
            for path in entry.list_of("include_directories", str, []):
                includes.extend(self._resolve_glob(path, env))

            artifacts: list[Path] = []
            for path in entry.list_of("artifacts", str, []):
                artifacts.extend(self._resolve_glob(path, env))

            configs.append(
                Script(
                    script_id,
                    prefix + name + suffix,
                    entry.string("newline", "\r\n"),
                    includes,
                    env,
                    sources,
                    artifacts,
                )
            )

        return configs

    def _load_release_package(self, release: Toml) -> ReleasePackage:
        pkg = release.table("package")
        if pkg is None:
            return ReleasePackage(
                self._project.name, self._project.name, self._project.name
            )

        env = self._project.variables

        return ReleasePackage(
            _exp(pkg.string("filename", self._project.name), env),
            _exp(pkg.string("name", self._project.name), env),
            _exp(pkg.string("id", self._project.name), env),
            _exp_opt(pkg.string("information"), env),
            self._project.version,
            self._project.author,
            _exp_opt(pkg.string("license"), env),
            _exp_opt(pkg.string("description"), env),
        )

    def _load_release_contents(
        self, release: Toml, artifact: Artifact
    ) -> ReleaseContentContainer:
        contents = release.table("contents")
        if contents is None:
            raise ValueError("[release.contents] section is required")

        return ReleaseContentContainer(
            self._load_release_extension(contents, artifact),
            self._load_release_documents(contents),
            self._load_release_assets(contents),
        )

    def _load_release_extension(
        self, contents: Toml, artifact: Artifact
    ) -> list[ReleaseExtension]:
        entries = contents.tables("extensions")
        if entries is None:
            return []

        env = self._project.variables
        items: list[ReleaseExtension] = []

        for entry in entries:
            files: list[Path] = []
            for file in entry.list_of("files", str, []):
                prefix, _, identifier = file.partition(":")

                if prefix == "script" and identifier in artifact.script:
                    files.extend(artifact.script[identifier])
                elif prefix == "plugin" and identifier in artifact.plugin:
                    files.extend(artifact.plugin[identifier])
                else:
                    files.extend(self._resolve_glob(file, env))

            items.append(
                ReleaseExtension(
                    _exp(entry.string("directory", ""), env), files
                )
            )

        return items

    def _load_release_documents(self, contents: Toml) -> list[ReleaseDocument]:
        entries = contents.tables("documents")
        if entries is None:
            return []

        env = self._project.variables
        docs: list[ReleaseDocument] = []

        for entry in entries:
            files: list[Path] = []
            for file in entry.list_of("files", str, []):
                files.extend(self._resolve_glob(file, env))

            docs.append(
                ReleaseDocument(
                    _exp(entry.string("directory", ""), env), files
                )
            )

        return docs

    def _load_release_assets(self, contents: Toml) -> list[ReleaseAsset]:
        entries = contents.tables("assets")
        if entries is None:
            return []

        env = self._project.variables

        assets: list[ReleaseAsset] = []
        for entry in entries:
            if not entry.boolean("enabled", True):
                continue

            name = entry.string("name")
            if name is None:
                raise ValueError("release.contents.assets.name is required")

            sources: list[AssetSource] = []
            for src in entry.tables("sources", []):
                files: list[Path | str] = []
                for path in src.list_of("files", str, []):
                    if path.startswith(("http://", "https://")):
                        files.append(_exp(path, env))
                    else:
                        files.extend(self._resolve_glob(path, env))

                sources.append(
                    AssetSource(_exp(src.string("directory", ""), env), files)
                )

            docs: list[AssetDocument] = []
            for doc in entry.tables("documents", []):
                filename = doc.string("filename")
                if filename is None:
                    raise ValueError(
                        "release.contents.assets.documents.name is required"
                    )
                docs.append(
                    AssetDocument(
                        filename, _exp(doc.string("content", ""), env)
                    )
                )

            assets.append(
                ReleaseAsset(
                    _exp(name, env),
                    _exp(entry.string("directory", ""), env),
                    sources,
                    docs,
                )
            )

        return assets

    def _resolve_glob(self, path: str, env: dict[str, str]) -> list[Path]:
        path = expand_variables(path, env)
        matched = sorted(self._root.glob(path))
        return matched if matched else [self._root / path]


class Cache:
    _data: Json
    _path: Path

    def __init__(self, path: Path) -> None:
        self._path = path
        try:
            self._data = Json(path)
        except (FileNotFoundError, ValueError, OSError):
            self._data = Json()

    def load_artifacts(self) -> Artifact | None:
        build = self._data.object("build")
        if build is None:
            return None

        artifacts = build.object("artifacts")
        if artifacts is None:
            return None

        plugins: dict[str, list[Path]] = {}
        scripts: dict[str, list[Path]] = {}

        for k, v in artifacts.object("plugins", Json()).items():
            if isinstance(v, list):
                plugins[k] = [
                    Path(p)
                    for p in cast("list[object]", v)
                    if isinstance(p, str)
                ]

        for k, v in artifacts.object("scripts", Json()).items():
            if isinstance(v, list):
                scripts[k] = [
                    Path(p)
                    for p in cast("list[object]", v)
                    if isinstance(p, str)
                ]

        self._data.save(self._path)  # 雑な破損ファイルの修正

        return Artifact(plugins, scripts)

    def save_artifacts(self, artifact: Artifact) -> None:
        build = self._data.object("build", Json())
        artifacts = build.object("artifacts", Json())

        plugins = Json()
        for k, v in artifact.plugin.items():
            plugins.set(k, [str(p) for p in v])

        scripts = Json()
        for k, v in artifact.script.items():
            scripts.set(k, [str(p) for p in v])

        artifacts.set("plugins", plugins)
        artifacts.set("scripts", scripts)

        self._data.save(self._path)

    def load_installations(self) -> list[Path] | None:
        install = self._data.object("install")
        if install is None:
            return None

        installations = install.list_of("installations", str)
        if installations is None:
            return None

        return [Path(p) for p in installations]

    def save_installations(self, installations: list[Path]) -> None:
        install = self._data.object("install", Json())
        install.set("installations", [str(p) for p in installations])
        self._data.save(self._path)
