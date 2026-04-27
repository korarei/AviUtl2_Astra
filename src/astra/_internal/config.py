from __future__ import annotations

import importlib.metadata as metadata
import json
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from logging import getLogger
from pathlib import Path
from typing import Annotated, ClassVar, Literal, cast, final, overload, override

from packaging.specifiers import SpecifierSet
from packaging.version import Version
from pydantic import (
    AfterValidator,
    BaseModel,
    BeforeValidator,
    ConfigDict,
    Field,
    ValidationInfo,
    field_validator,
    model_validator,
)

from astra._internal.utils import expand_variables, resolve_glob


logger = getLogger(__name__)

_PACKAGE_DIRECTORIES = (
    "Plugin/",
    "Script/",
    "Language/",
    "Alias/",
    "Default/",
    "Figure/",
    "Preset/",
    "Transition/",
)
_OLD_SCRIPT_SUFFIXES = (".anm", ".obj", ".cam", ".scn", ".tra")
_NEW_SCRIPT_SUFFIXES = (".anm2", ".obj2", ".cam2", ".scn2", ".tra2")


def _require_context(info: ValidationInfo) -> Json:
    ctx = info.context
    if not isinstance(ctx, Json):
        raise RuntimeError("'info.context' must be a Json object")

    return ctx


def _expand_variables(v: str, info: ValidationInfo) -> str:
    ctx = _require_context(info)
    variables = cast(dict[str, str], ctx.get(dict, "variables", {}))
    return expand_variables(v, variables)


def _resolve_glob(v: object, info: ValidationInfo) -> list[Path]:
    if not isinstance(v, list):
        raise TypeError(f"'{info.field_name}' must be an array")

    if len(cast(list[object], v)) == 0:
        raise ValueError(f"'{info.field_name}' must contain at least one entry")

    ctx = _require_context(info)
    root = Path(ctx.get(str, "root", "")) or Path.cwd()
    variables = cast(dict[str, str], ctx.get(dict, "variables", {}))

    paths: list[Path] = []
    for p in cast(list[object], v):
        if not isinstance(p, str):
            raise TypeError(f"'{info.field_name}' entries must be strings")

        paths.extend(resolve_glob(root, expand_variables(p, variables)))

    return paths


def _check_package_directory(v: str, info: ValidationInfo) -> str:
    ctx = _require_context(info)
    variables = cast(dict[str, str], ctx.get(dict, "variables", {}))
    v = expand_variables(v, variables).replace("\\", "/")

    if not v.startswith(_PACKAGE_DIRECTORIES):
        logger.warning(f"'{v}' is not a package directory")

    return v


def _resolve_field(value: str, fallback: str, variables: dict[str, str]) -> str:
    return fallback if value == "" else expand_variables(value, variables)


_ExpandedString = Annotated[str, AfterValidator(_expand_variables)]
_ResolvedPaths = Annotated[list[Path], BeforeValidator(_resolve_glob)]
_PackageDirectory = Annotated[str, AfterValidator(_check_package_directory)]


class _ProxyMeta(type):
    @override
    def __instancecheck__(cls, instance: object) -> bool:
        return isinstance(instance, cls.__bases__[0])


@final
class Toml:
    type Primitive = str | bool | int | float
    type Value = Primitive | list[Value] | dict[str, Value]

    class Array(list[Value], metaclass=_ProxyMeta):
        pass

    class Table(dict[str, Value], metaclass=_ProxyMeta):
        pass

    _data: dict[str, Value]

    def __init__(self, data: Path | dict[str, Value]) -> None:
        if isinstance(data, dict):
            self._data = data
        else:
            with open(data, "rb") as f:
                self._data = tomllib.load(f)

    def data(self) -> dict[str, Value]:
        return self._data

    @overload
    def get[T: Value | Toml](self, cls: type[T], key: str) -> T | None: ...

    @overload
    def get[T: Value | Toml](self, cls: type[T], key: str, default: T) -> T: ...

    def get[T: Value | Toml](
        self, cls: type[T], key: str, default: T | None = None
    ) -> T | None:
        v = self._data.get(key)

        if isinstance(v, cls):
            return v

        if cls is Toml and isinstance(v, dict):
            return cast(T, Toml(v))

        return default


@final
class Json:
    type Primitive = None | str | bool | int | float
    type Value = Primitive | Sequence[Value] | Mapping[str, Value]

    class Array(list[Value], metaclass=_ProxyMeta):
        pass

    class Object(dict[str, Value], metaclass=_ProxyMeta):
        pass

    _data: dict[str, Value]

    def __init__(self, data: Path | Mapping[str, Value]) -> None:
        if isinstance(data, dict):
            self._data = data
        elif isinstance(data, Mapping):
            self._data = dict(data)
        else:
            with open(data, "rb") as f:
                self._data = json.load(f)

    def __setitem__(self, key: str, value: Value | Json) -> None:
        self._data[key] = value.data() if isinstance(value, Json) else value

    def data(self) -> dict[str, Value]:
        return self._data

    @overload
    def get[T: Value | Json](self, cls: type[T], key: str) -> T | None: ...

    @overload
    def get[T: Value | Json](self, cls: type[T], key: str, default: T) -> T: ...

    def get[T: Value | Json](
        self, cls: type[T], key: str, default: T | None = None
    ) -> T | None:
        v = self._data.get(key)

        if isinstance(v, cls):
            return v

        if cls is Json and isinstance(v, Mapping):
            return cast(T, Json(v))

        return default

    def setdefault[T: Value | Json](self, cls: type[T], key: str, default: T) -> T:
        if isinstance(default, Json):
            v = self._data.setdefault(key, default.data())
            if isinstance(v, dict):
                return cast(T, Json(v))

            self._data[key] = default.data()
            return default
        else:
            v = self._data.setdefault(key, default)
            if isinstance(v, cls):
                return v

            self._data[key] = default
            return default

    def save(self, path: Path) -> None:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self._data, f, ensure_ascii=False, indent=4)
            _ = f.write("\n")

    def dump(self) -> str:
        return json.dumps(self._data, ensure_ascii=False, indent=4) + "\n"


class ConfigModel(BaseModel):
    model_config: ClassVar[ConfigDict] = ConfigDict(
        alias_generator=lambda s: s.replace("_", "-"),
        populate_by_name=True,
        strict=True,
        str_strip_whitespace=True,
        extra="ignore",
    )


class Project(ConfigModel):
    name: str = Field(min_length=1)
    version: str | None = Field(default=None, min_length=1)
    author: str | None = Field(default=None, min_length=1)
    requires_aviutl2: str | None = Field(default=None, min_length=1)
    variables: dict[str, str] = Field(default_factory=dict)

    @field_validator("version", mode="after")
    @classmethod
    def _overwrite_version(cls, v: str | None, info: ValidationInfo) -> str | None:
        ctx = _require_context(info)
        return ctx.get(str, "version") or v

    @model_validator(mode="after")
    def _update_variables(self, info: ValidationInfo) -> Project:
        ctx = _require_context(info)
        variables = self.variables

        variables["PROJECT_NAME"] = self.name

        if version := self.version:
            variables["PROJECT_VERSION"] = version

        if author := self.author:
            variables["PROJECT_AUTHOR"] = author

        if requires_aviutl2 := self.requires_aviutl2:
            variables["PROJECT_REQUIRES_AVIUTL2"] = requires_aviutl2

        variables |= cast(dict[str, str], ctx.get(dict, "defines", {}))

        return self


# build時に変数展開する
class Command(ConfigModel):
    commands: list[str] = Field(default_factory=list)
    artifacts: list[str] = Field(default_factory=list)


class Plugin(ConfigModel):
    id: str = Field(min_length=1)
    shell: str | None = Field(default=None, min_length=1)
    variables: dict[str, str] = Field(default_factory=dict)
    release: Command
    debug: Command = Field(default_factory=Command)

    @field_validator("variables", mode="after")
    @classmethod
    def _update_vars(cls, v: dict[str, str], info: ValidationInfo) -> dict[str, str]:
        ctx = _require_context(info)
        v |= cast(dict[str, str], ctx.get(dict, "variables", {}))

        return v

    @model_validator(mode="after")
    def _overwrite_debug(self) -> Plugin:
        if "debug" not in self.model_fields_set:
            self.debug = self.release

        return self


class _Script(ConfigModel):
    id: str = Field(min_length=1)
    name: str = Field(default="", min_length=1)
    prefix: Literal["", "@"] = ""
    suffix: str = ""
    newline: Literal["\r\n", "\n"] = "\r\n"
    source_encoding: str = "utf-8"
    target_encoding: str = "utf-8"
    variables: dict[str, str] = Field(default_factory=dict)
    include_directories: list[str] = Field(default_factory=list)
    sources: list[dict[str, str]] = Field(default_factory=list)
    artifacts: list[str] = Field(default_factory=list)

    @field_validator("suffix", mode="after")
    @classmethod
    def _check_suffix(cls, v: str) -> str:
        v = v.lower()

        if v not in (*_OLD_SCRIPT_SUFFIXES, *_NEW_SCRIPT_SUFFIXES):
            logger.warning("Invalid suffix")

        return v

    @model_validator(mode="after")
    def _check_encoding(self) -> _Script:
        suffix = self.suffix.lower()
        encoding = self.target_encoding.lower().replace("-", "").replace("_", "")

        if suffix in _NEW_SCRIPT_SUFFIXES and encoding != "utf8":
            raise ValueError("'target-encoding' must be utf-8")

        if suffix in _OLD_SCRIPT_SUFFIXES and encoding != "cp932":
            raise ValueError("'target-encoding' must be cp932")

        return self


@dataclass(frozen=True)
class ScriptSource:
    files: list[Path]
    variables: dict[str, str]


class Script:
    id: str
    name: str
    prefix: str
    suffix: str
    newline: str
    source_encoding: str
    target_encoding: str
    variables: dict[str, str]
    include_directories: list[Path]
    sources: list[ScriptSource]
    artifacts: list[Path]

    def __init__(self, script: _Script, ctx: Json) -> None:
        root = Path(ctx.get(str, "root", "")) or Path.cwd()
        variables = cast(dict[str, str], ctx.get(dict, "variables", {}))

        name = script.name or variables["PROJECT_NAME"]
        variables = {
            **variables,
            **script.variables,
            "SCRIPT_NAME": name,
        }

        self.id = script.id
        self.name = name
        self.prefix = script.prefix
        self.suffix = script.suffix
        self.newline = script.newline
        self.source_encoding = script.source_encoding
        self.target_encoding = script.target_encoding
        self.variables = variables

        self.include_directories = [
            p
            for v in script.include_directories
            for p in resolve_glob(root, expand_variables(v, variables))
        ]

        self.artifacts = [
            p
            for v in script.artifacts
            for p in resolve_glob(root, expand_variables(v, variables))
        ]

        self.sources = []
        for src in script.sources:
            file = src.get("file")
            if file is None:
                raise ValueError("'file' is required")

            self.sources.append(
                ScriptSource(
                    resolve_glob(root, expand_variables(file, variables)),
                    {k: v for k, v in src.items() if k != "file"},
                )
            )


@dataclass(frozen=True)
class Build:
    root: Path
    plugins: list[Plugin] = field(default_factory=list)
    scripts: list[Script] = field(default_factory=list)


class ReleasePackage(ConfigModel):
    filename: str = Field(default="", min_length=1)
    id: str = Field(default="", min_length=1)
    name: str = Field(default="", min_length=1)
    uninstall_subdirectory_files: bool = False
    information: _ExpandedString | None = None
    version: _ExpandedString | None = None
    author: _ExpandedString | None = None
    license: _ExpandedString | None = None
    summary: _ExpandedString | None = None
    description: _ExpandedString | None = None
    website: _ExpandedString | None = None
    report_issue: _ExpandedString | None = None

    @field_validator("filename", mode="after")
    @classmethod
    def _rename_filename(cls, v: str) -> str:
        if not v.endswith(".zip"):
            return f"{v}.au2pkg.zip"

        return v

    @model_validator(mode="after")
    def _overwrite_metadata(self, info: ValidationInfo) -> ReleasePackage:
        ctx = _require_context(info)
        variables = cast(dict[str, str], ctx.get(dict, "variables", {}))
        name = variables.get("PROJECT_NAME", "")

        self.filename = _resolve_field(self.filename, f"{name}.au2pkg.zip", variables)
        self.name = _resolve_field(self.name, name, variables)
        self.id = _resolve_field(self.id, name, variables)

        if self.version is None:
            self.version = variables.get("PROJECT_VERSION")

        if self.author is None:
            self.author = variables.get("PROJECT_AUTHOR")

        return self


class ReleaseExtension(ConfigModel):
    directory: _PackageDirectory = ""
    files: list[Path] = Field(default_factory=list)

    @field_validator("files", mode="before")
    @classmethod
    def _resolve_files(cls, v: object, info: ValidationInfo) -> list[Path]:
        if not isinstance(v, list):
            raise TypeError(f"'{info.field_name}' must be an array")

        ctx = _require_context(info)
        root = Path(ctx.get(str, "root", "")) or Path.cwd()
        variables = cast(dict[str, str], ctx.get(dict, "variables", {}))
        artifact = cast(dict[str, dict[str, list[str]]], ctx.get(dict, "artifact", {}))

        extensions: list[Path] = []
        for extension in cast(list[object], v):
            if not isinstance(extension, str):
                raise TypeError(f"'{info.field_name}' entries must be strings")

            prefix, sep, identifier = extension.partition(":")

            if sep == ":" and prefix in ("script", "plugin"):
                if (found := artifact[f"{prefix}s"].get(identifier)) is not None:
                    extensions.extend(map(Path, found))
                else:
                    logger.warning(f"'{prefix}:{identifier}' not found")
            else:
                paths = resolve_glob(root, expand_variables(extension, variables))
                extensions.extend(paths)

        if len(extensions) == 0:
            raise ValueError(f"'{info.field_name}' must contain at least one entry")

        return extensions


class ReleaseDocument(ConfigModel):
    directory: _PackageDirectory = ""
    files: _ResolvedPaths = Field(default_factory=list)


class AssetSource(ConfigModel):
    directory: _PackageDirectory = ""
    files: list[Path | str] = Field(default_factory=list)

    @field_validator("files", mode="before")
    @classmethod
    def _resolve_files(cls, v: object, info: ValidationInfo) -> list[Path | str]:
        if not isinstance(v, list):
            raise TypeError(f"'{info.field_name}' must be an array")

        ctx = _require_context(info)
        root = Path(ctx.get(str, "root", "")) or Path.cwd()
        variables = cast(dict[str, str], ctx.get(dict, "variables", {}))

        files: list[Path | str] = []
        for file in cast(list[object], v):
            if not isinstance(file, str):
                raise TypeError(f"'{info.field_name}' entries must be strings")

            if file.startswith(("http://", "https://")):
                files.append(file)
                continue

            files.extend(resolve_glob(root, expand_variables(file, variables)))

        if len(files) == 0:
            raise ValueError(f"'{info.field_name}' must contain at least one entry")

        return files


class AssetDocument(ConfigModel):
    filename: _ExpandedString = Field(min_length=1)
    content: _ExpandedString = ""


class ReleaseAsset(ConfigModel):
    name: str = Field(min_length=1)
    directory: _PackageDirectory = ""
    sources: list[AssetSource] = Field(default_factory=list)
    documents: list[AssetDocument] = Field(default_factory=list)


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


class Artifact:
    _data: Json

    def __init__(
        self,
        plugins: dict[str, list[str]] | None = None,
        scripts: dict[str, list[str]] | None = None,
    ) -> None:
        if plugins is None:
            plugins = {}

        if scripts is None:
            scripts = {}

        self._data = Json({"plugins": plugins, "scripts": scripts})

    def data(self) -> Mapping[str, Json.Value]:
        return self._data.data()


class Extension:
    _data: Json

    def __init__(self, files: list[str] | None = None) -> None:
        if files is None:
            files = []

        self._data = Json({"files": files})

    def data(self) -> Mapping[str, Json.Value]:
        return self._data.data()

    @property
    def files(self) -> Sequence[str]:
        return cast(Sequence[str], self._data.get(Json.Array, "files", Json.Array()))


class Config:
    _data: Toml
    _root: Path
    _project: Project

    def __init__(
        self,
        path: Path,
        version: str | None = None,
        defines: dict[str, str] | None = None,
    ) -> None:
        path = path.resolve()
        self._root = path.parent
        self._data = Toml(path)
        self._load_astra()
        self._load_project(version, defines or {})

    def load_build(self) -> Build:
        logger.info("Loading build configuration")

        build = self._data.get(Toml, "build")
        if build is None:
            raise KeyError("'build' section is required in astra.toml")

        return Build(self._root, self._load_plugins(build), self._load_scripts(build))

    def load_release(self, artifact: Artifact) -> Release:
        logger.info("Loading release configuration")

        release = self._data.get(Toml, "release")
        if release is None:
            raise KeyError("'release' section is required in astra.toml")

        return Release(
            self._load_release_package(release),
            self._load_release_contents(release, artifact),
        )

    def load_install(self, artifact: Artifact) -> Install:
        logger.info("Loading install configuration")

        release = self._data.get(Toml, "release")
        if release is None:
            raise KeyError("'release' section is required in astra.toml")

        contents = release.get(Toml, "contents")
        if contents is None:
            raise KeyError("'release.contents' section is required in astra.toml")

        extensions = self._load_release_extension(contents, artifact)

        return Install(
            [
                extension
                for extension in extensions
                if extension.directory.startswith(_PACKAGE_DIRECTORIES)
            ]
        )

    def _load_astra(self) -> None:
        astra = self._data.get(Toml, "astra")
        if astra is None:
            return

        version = astra.get(str, "requires-astra") or astra.get(str, "requires_astra")
        if version not in (None, ""):
            try:
                astra_version = metadata.version("astra")
            except metadata.PackageNotFoundError:
                raise RuntimeError("Astra is not installed as a package")

            if Version(astra_version) not in SpecifierSet(version):
                raise ValueError(f"'{version}' does not satisfy '{astra_version}'")

    def _load_project(self, version: str | None, defines: dict[str, str]) -> None:
        project = self._data.get(Toml.Table, "project")
        if project is None:
            raise KeyError("'project' section is required in astra.toml")

        ctx = Json({"version": version, "defines": defines})

        self._project = Project.model_validate(project, context=ctx)

    def _load_plugins(self, build: Toml) -> list[Plugin]:
        ctx = Json({"variables": self._project.variables})

        plugins = build.get(Toml.Array, "plugins")
        if plugins is None:
            return []

        configs: list[Plugin] = []
        for plugin in plugins:
            if not isinstance(plugin, dict):
                raise TypeError("'plugins' entries must be tables")

            plugin = Toml(plugin)

            if not plugin.get(bool, "enabled", True):
                logger.warning(f"Plugin '{plugin.get(str, 'id')}' is disabled")
                continue

            configs.append(Plugin.model_validate(plugin.data(), context=ctx))

        return configs

    def _load_scripts(self, build: Toml) -> list[Script]:
        ctx = Json({"root": str(self._root), "variables": self._project.variables})

        scripts = build.get(Toml.Array, "scripts")
        if scripts is None:
            return []

        configs: list[Script] = []
        for script in scripts:
            if not isinstance(script, dict):
                raise TypeError("'scripts' entries must be tables")

            script = Toml(script)

            if not script.get(bool, "enabled", True):
                logger.warning(f"Script '{script.get(str, 'id')}' is disabled")
                continue

            configs.append(Script(_Script.model_validate(script.data()), ctx))

        return configs

    def _load_release_package(self, release: Toml) -> ReleasePackage:
        ctx = Json({"variables": self._project.variables})

        package = release.get(Toml, "package")
        if package is None:
            return ReleasePackage.model_validate({}, context=ctx)

        return ReleasePackage.model_validate(package.data(), context=ctx)

    def _load_release_contents(
        self, release: Toml, artifact: Artifact
    ) -> ReleaseContentContainer:
        contents = release.get(Toml, "contents")
        if contents is None:
            raise KeyError("'release.contents' section is required in astra.toml")

        return ReleaseContentContainer(
            self._load_release_extension(contents, artifact),
            self._load_release_documents(contents),
            self._load_release_assets(contents),
        )

    def _load_release_extension(
        self, contents: Toml, artifact: Artifact
    ) -> list[ReleaseExtension]:
        extensions = contents.get(Toml.Array, "extensions")
        if extensions is None:
            return []

        ctx = Json(
            {
                "root": str(self._root),
                "variables": self._project.variables,
                "artifact": artifact.data(),
            }
        )

        return [
            ReleaseExtension.model_validate(extension, context=ctx)
            for extension in extensions
        ]

    def _load_release_documents(self, contents: Toml) -> list[ReleaseDocument]:
        documents = contents.get(Toml.Array, "documents")
        if documents is None:
            return []

        ctx = Json({"root": str(self._root), "variables": self._project.variables})

        return [
            ReleaseDocument.model_validate(document, context=ctx)
            for document in documents
        ]

    def _load_release_assets(self, contents: Toml) -> list[ReleaseAsset]:
        ctx = Json({"root": str(self._root), "variables": self._project.variables})

        assets = contents.get(Toml.Array, "assets")
        if assets is None:
            return []

        configs: list[ReleaseAsset] = []
        for asset in assets:
            if not isinstance(asset, dict):
                raise TypeError("'assets' entries must be tables")

            asset = Toml(asset)

            if not asset.get(bool, "enabled", True):
                logger.warning(f"Asset '{asset.get(str, 'name')}' is disabled")
                continue

            configs.append(ReleaseAsset.model_validate(asset.data(), context=ctx))

        return configs


class Cache:
    _data: Json
    _path: Path

    def __init__(self, path: Path) -> None:
        path = path.resolve()
        self._path = path
        self._data = Json(path) if path.is_file() else Json({})

    def load[T: Artifact | Extension](self, cls: type[T]) -> T:
        if cls is Artifact:
            logger.info("Loading artifacts from cache")

            build = self._data.get(Json, "build", Json({}))
            artifacts = build.get(Json, "artifacts", Json({}))
            plugins = artifacts.get(Json.Object, "plugins", Json.Object())
            scripts = artifacts.get(Json.Object, "scripts", Json.Object())

            return cast(
                T,
                Artifact(
                    {
                        k: [p for p in v if isinstance(p, str)]
                        for k, v in plugins.items()
                        if isinstance(v, list)
                    },
                    {
                        k: [p for p in v if isinstance(p, str)]
                        for k, v in scripts.items()
                        if isinstance(v, list)
                    },
                ),
            )
        elif cls is Extension:
            logger.info("Loading extensions from cache")

            install = self._data.get(Json, "install", Json({}))
            extensions = install.get(Json, "extensions", Json({}))
            files = extensions.get(Json.Array, "files", Json.Array())

            return cast(T, Extension([v for v in files if isinstance(v, str)]))
        else:
            raise TypeError(f"'{cls.__name__}' is not supported")

    def save(self, value: Artifact | Extension) -> None:
        if isinstance(value, Artifact):
            logger.info(f"Caching artifacts to '{self._path}'")

            self._data["build"] = {"artifacts": value.data()}
        else:
            logger.info(f"Caching extensions to '{self._path}'")

            self._data["install"] = {"extensions": value.data()}

        self._data.save(self._path)
