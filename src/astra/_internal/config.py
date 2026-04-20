from __future__ import annotations

import importlib.metadata as metadata
import json
import tomllib
from collections.abc import ItemsView
from dataclasses import dataclass, field
from logging import getLogger
from pathlib import Path
from typing import Annotated, ClassVar, Literal, TypeAlias, TypeVar, cast, overload

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
T = TypeVar("T", str, bool)

_PACKAGE_HIERARCHIES = (
    "Plugin/",
    "Script/",
    "Language/",
    "Alias/",
    "Default/",
    "Figure/",
    "Preset/",
    "Transition/",
)


def _expand_str(v: str, info: ValidationInfo) -> str:
    ctx = info.context
    if type(ctx) is not Context:
        raise RuntimeError("info.context is not a Context object")

    env = ctx.get_variables("variables", {})
    return expand_variables(v, env)


def _resolve_paths(v: object, info: ValidationInfo) -> list[Path]:
    if type(v) is not list:
        raise ValueError(f"{info.field_name} must be a list of strings")

    ctx = info.context
    if type(ctx) is not Context:
        raise RuntimeError("info.context is not a Context object")

    root = ctx.get_data(Path, "root", Path().cwd())
    env = ctx.get_variables("variables", {})

    paths: list[Path] = []
    for p in cast(list[object], v):
        if type(p) is not str:
            raise ValueError(f"{info.field_name} must be a list of strings")

        paths.extend(resolve_glob(root, expand_variables(p, env)))

    if not paths:
        raise FileNotFoundError(f"{info.field_name} is empty")

    return paths


def _check_hierarchy(v: str, info: ValidationInfo) -> str:
    ctx = info.context
    if type(ctx) is not Context:
        raise RuntimeError("info.context is not a Context object")

    env = ctx.get_variables("variables", {})
    v = expand_variables(v, env).replace("\\", "/")

    if not v.startswith(_PACKAGE_HIERARCHIES):
        logger.warning(f"{v} is not a package hierarchy")

    return v


_ExpandedStr = Annotated[str, AfterValidator(_expand_str)]
_ResolvedPaths = Annotated[list[Path], BeforeValidator(_resolve_paths)]
_PackageDir = Annotated[str, AfterValidator(_check_hierarchy)]


class Context:
    Data: TypeAlias = str | Path

    _data: dict[str, None | Data]
    _variables: dict[str, dict[str, str]]
    _objects: dict[str, object]

    def __init__(
        self,
        data: dict[str, None | Data],
        variables: dict[str, dict[str, str]],
        objects: dict[str, object],
    ) -> None:
        self._data = data
        self._variables = variables
        self._objects = objects

    @overload
    def get_data[T: Data](self, cls: type[T], key: str) -> T | None: ...

    @overload
    def get_data[T: Data](self, cls: type[T], key: str, default: T) -> T: ...

    def get_data[T: Data](
        self, cls: type[T], key: str, default: T | None = None
    ) -> T | None:
        v = self._data.get(key)
        if isinstance(v, cls):
            return v

        if default is not None:
            self._data[key] = default
            return default

        return None

    @overload
    def get_variables(self, key: str) -> dict[str, str] | None: ...

    @overload
    def get_variables(self, key: str, default: dict[str, str]) -> dict[str, str]: ...

    def get_variables(
        self, key: str, default: dict[str, str] | None = None
    ) -> dict[str, str] | None:
        v = self._variables.get(key)
        if type(v) is dict:
            return v

        if default is not None:
            self._variables[key] = default
            return default

        return None

    @overload
    def get_object[T](self, cls: type[T], key: str, default: T) -> T: ...

    @overload
    def get_object[T](self, cls: type[T], key: str) -> T | None: ...

    def get_object[T](
        self, cls: type[T], key: str, default: T | None = None
    ) -> T | None:
        v = self._objects.get(key)
        if isinstance(v, cls):
            return v

        if default is not None:
            self._objects[key] = default
            return default

        return None


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

    def objects(self, key: str, default: list[Json] | None = None) -> list[Json] | None:
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
                v._data if type(v) is Json else v for v in cast("list[object]", value)
            ]
        else:
            self._data[key] = value

    def save(self, path: Path, indent: int = 4) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self._data, f, ensure_ascii=False, indent=indent)

    def dumps(self, indent: int = 4) -> str:
        return json.dumps(self._data, ensure_ascii=False, indent=indent)


class ConfigModel(BaseModel):
    model_config: ClassVar[ConfigDict] = ConfigDict(
        alias_generator=lambda s: s.replace("_", "-"),
        populate_by_name=True,
        strict=True,
        str_strip_whitespace=True,
    )


class Project(ConfigModel):
    name: str = Field(min_length=1)
    version: str | None = None
    author: str | None = None
    requires_aviutl2: str | None = None
    variables: dict[str, str] = Field(default_factory=dict)

    @field_validator("version", mode="after")
    @classmethod
    def _overwrite_version(cls, v: str | None, info: ValidationInfo) -> str | None:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        return v if (version := ctx.get_data(str, "version")) is None else version

    @model_validator(mode="after")
    def _update_variables(self, info: ValidationInfo) -> Project:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        variables = self.variables

        variables["PROJECT_NAME"] = self.name

        if (version := self.version) is not None:
            variables["PROJECT_VERSION"] = version

        if (author := self.author) is not None:
            variables["PROJECT_AUTHOR"] = author

        if (requires_aviutl2 := self.requires_aviutl2) is not None:
            variables["PROJECT_REQUIRES_AVIUTL2"] = requires_aviutl2

        variables |= ctx.get_variables("defines", {})

        return self


class Command(ConfigModel):
    commands: list[str] = Field(default_factory=list)
    artifacts: list[str] = Field(default_factory=list)

    @field_validator("commands", "artifacts", mode="after")
    @classmethod
    def _expand_variables(cls, v: list[str], info: ValidationInfo) -> list[str]:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        env = ctx.get_variables("variables", {})

        return [expand_variables(text, env) for text in v]


class Plugin(ConfigModel):
    id: str = Field(min_length=1)
    release: Command
    debug: Command = Field(default_factory=Command)

    @model_validator(mode="after")
    def _overwrite_debug(self) -> Plugin:
        if not (self.debug.commands or self.debug.artifacts):
            self.debug = self.release

        return self


class ScriptSource(ConfigModel):
    files: list[Path]
    variables: dict[str, str] = Field(default_factory=dict)

    @model_validator(mode="before")
    @classmethod
    def _resolve_sources(cls, v: object, info: ValidationInfo) -> dict[str, object]:
        if type(v) is not dict:
            raise ValueError("sources entries must be dicts")

        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        root = ctx.get_data(Path, "root", Path().cwd())
        env = ctx.get_variables("variables", {})

        file = cast(dict[str, object], v).get("file")
        if type(file) is not str:
            raise ValueError("file is required")

        files = resolve_glob(root, expand_variables(file, env))
        variables = {k: v for k, v in cast(dict[str, object], v).items() if k != "file"}

        return {"files": files, "variables": variables}


class Script(ConfigModel):
    id: str = Field(min_length=1)
    name: str = ""
    prefix: Literal["", "@"] = ""
    suffix: str = ""
    newline: Literal["\r\n", "\n"] = "\r\n"
    source_encoding: str = "utf-8"
    target_encoding: str = "utf-8"
    variables: dict[str, str] = Field(default_factory=dict)
    include_directories: _ResolvedPaths = Field(default_factory=list)
    sources: list[ScriptSource] = Field(default_factory=list)
    artifacts: _ResolvedPaths = Field(default_factory=list)

    @field_validator("variables", mode="before")
    @classmethod
    def _ignore_variables(cls, _: object) -> dict[str, str]:
        return {}

    @model_validator(mode="after")
    def _overwrite_name(self, info: ValidationInfo) -> Script:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        if not self.name:
            self.name = ctx.get_data(str, "name", "")

        return self

    @model_validator(mode="after")
    def _overwrite_variables(self, info: ValidationInfo) -> Script:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        self.variables = {
            **ctx.get_variables("variables", {}),
            "SCRIPT_NAME": self.name,
        }

        return self


@dataclass(frozen=True)
class Build:
    cwd: Path
    plugins: list[Plugin] = field(default_factory=list)
    scripts: list[Script] = field(default_factory=list)


class ReleasePackage(ConfigModel):
    filename: str = ""
    id: str = ""
    name: str = ""
    uninstall_subdirectory_files: bool = False
    information: _ExpandedStr | None = None
    version: _ExpandedStr | None = None
    author: _ExpandedStr | None = None
    license: _ExpandedStr | None = None
    summary: _ExpandedStr | None = None
    description: _ExpandedStr | None = None
    website: _ExpandedStr | None = None
    report_issue: _ExpandedStr | None = None

    @model_validator(mode="after")
    def _overwrite_metadata(self, info: ValidationInfo) -> ReleasePackage:
        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        env = ctx.get_variables("variables", {})
        name = env.get("PROJECT_NAME", "")

        if not self.filename:
            self.filename = name
        else:
            self.filename = expand_variables(self.filename, env)

        if not self.name:
            self.name = name
        else:
            self.name = expand_variables(self.name, env)

        if not self.id:
            self.id = name
        else:
            self.id = expand_variables(self.id, env)

        if self.version is None:
            self.version = env.get("PROJECT_VERSION")
        else:
            self.version = expand_variables(self.version, env)

        if self.author is None:
            self.author = env.get("PROJECT_AUTHOR")
        else:
            self.author = expand_variables(self.author, env)

        return self


class ReleaseExtension(BaseModel):
    directory: _PackageDir = ""
    files: list[Path] = Field(default_factory=list)

    @field_validator("files", mode="before")
    @classmethod
    def _resolve_files(cls, v: object, info: ValidationInfo) -> list[Path]:
        if type(v) is not list:
            raise ValueError(f"{info.field_name} must be a list of strings")

        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        root = ctx.get_data(Path, "root", Path().cwd())
        env = ctx.get_variables("variables", {})
        artifact = ctx.get_object(Artifact, "artifact", Artifact())

        extensions: list[Path] = []
        for extension in cast(list[object], v):
            if type(extension) is not str:
                raise ValueError(f"{info.field_name} must be a list of strings")

            prefix, sep, identifier = extension.partition(":")

            if prefix == "script" and sep:
                if identifier in artifact.script:
                    extensions.extend(artifact.script[identifier])
                else:
                    raise ValueError(f"Script artifact '{identifier}' not found")
            elif prefix == "plugin" and sep:
                if identifier in artifact.plugin:
                    extensions.extend(artifact.plugin[identifier])
                else:
                    raise ValueError(f"Plugin artifact '{identifier}' not found")
            else:
                extensions.extend(resolve_glob(root, expand_variables(extension, env)))

        if not extensions:
            raise FileNotFoundError(f"{info.field_name} is empty")

        return extensions


class ReleaseDocument(BaseModel):
    directory: _PackageDir = ""
    files: _ResolvedPaths = Field(default_factory=list)


class AssetSource(BaseModel):
    directory: _PackageDir = ""
    files: list[Path | str] = Field(default_factory=list)

    @field_validator("files", mode="before")
    @classmethod
    def _resolve_files(cls, v: object, info: ValidationInfo) -> list[Path | str]:
        if type(v) is not list:
            raise ValueError(f"{info.field_name} must be a list of strings")

        ctx = info.context
        if type(ctx) is not Context:
            raise RuntimeError("info.context is not a Context object")

        root = ctx.get_data(Path, "root", Path().cwd())
        env = ctx.get_variables("variables", {})

        files: list[Path | str] = []
        for file in cast(list[object], v):
            if type(file) is not str:
                raise ValueError(f"{info.field_name} must be a list of strings")

            if file.startswith(("http://", "https://")):
                files.append(file)
                continue

            files.extend(resolve_glob(root, expand_variables(file, env)))

        if not files:
            raise FileNotFoundError(f"{info.field_name} is empty")

        return files


class AssetDocument(BaseModel):
    filename: _ExpandedStr = Field(min_length=1)
    content: _ExpandedStr = ""


class ReleaseAsset(BaseModel):
    name: str = Field(min_length=1)
    directory: _PackageDir = ""
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


@dataclass(frozen=True)
class Artifact:
    plugin: dict[str, list[Path]] = field(default_factory=dict)
    script: dict[str, list[Path]] = field(default_factory=dict)


class Config:
    _data: dict[str, object]
    _root: Path
    _project: Project

    def __init__(
        self,
        path: Path,
        version: str | None = None,
        defines: dict[str, str] | None = None,
    ) -> None:
        path = path.resolve()

        try:
            with open(path, "rb") as f:
                self._data = tomllib.load(f)
        except FileNotFoundError as e:
            raise FileNotFoundError(f"Config file missing at '{path}': {e}") from e
        except tomllib.TOMLDecodeError as e:
            raise ValueError(f"Failed to parse TOML in '{path.name}': {e}") from e
        except OSError as e:
            raise OSError(f"System error occurred while accessing '{path}': {e}") from e

        self._root = path.parent
        self._load_astra()
        self._load_project(version, defines or {})

    def load_build(self) -> Build:
        build = self._data.get("build")
        if build is None or type(build) is not dict:
            raise ValueError("[build] section is required")

        return Build(
            self._root,
            self._load_plugins(cast(dict[str, object], build)),
            self._load_scripts(cast(dict[str, object], build)),
        )

    def load_release(self, artifact: Artifact) -> Release:
        release = self._data.get("release")
        if release is None or type(release) is not dict:
            raise ValueError("[release] section is required")

        return Release(
            self._load_release_package(cast(dict[str, object], release)),
            self._load_release_contents(cast(dict[str, object], release), artifact),
        )

    def load_install(self, artifact: Artifact) -> Install:
        release = self._data.get("release")
        if release is None or type(release) is not dict:
            raise ValueError("[release] section is required")

        contents = cast(dict[str, object], release).get("contents")
        if contents is None or type(contents) is not dict:
            raise ValueError("[release.contents] section is required")

        extensions = self._load_release_extension(
            cast(dict[str, object], contents), artifact
        )

        return Install(
            [
                extention
                for extention in extensions
                if extention.directory.startswith(_PACKAGE_HIERARCHIES)
            ]
        )

    def _load_astra(self) -> None:
        astra = self._data.get("astra")
        if astra is None or type(astra) is not dict:
            return

        if type(version := cast(dict[str, object], astra).get("requires-astra")) is str:
            try:
                __version__ = metadata.version("astra")
            except metadata.PackageNotFoundError as e:
                raise ValueError("Version not found") from e

            if Version(__version__) not in SpecifierSet(version):
                raise ValueError(
                    f"Requires astra {version} but {__version__} is installed"
                )

    def _load_project(self, version: str | None, defines: dict[str, str]) -> None:
        project = self._data.get("project")
        if project is None or type(project) is not dict:
            raise ValueError("[project] section is required")

        ctx = Context({"version": version}, {"defines": defines}, {})

        self._project = Project.model_validate(project, context=ctx)

    def _load_plugins(self, build: dict[str, object]) -> list[Plugin]:
        plugins = build.get("plugins")
        if plugins is None or type(plugins) is not list:
            return []

        configs: list[Plugin] = []
        for plugin in cast(list[object], plugins):
            if type(plugin) is not dict:
                raise ValueError("plugins entries must be a dictionary")

            enabled = cast(dict[str, object], plugin).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            env = cast(dict[str, object], plugin).get("variables")
            if type(env) is dict:
                ctx = Context(
                    {},
                    {
                        "variables": {
                            **self._project.variables,
                            **{
                                k: v
                                for k, v in cast(dict[str, object], env).items()
                                if type(v) is str
                            },
                            "BUILD_DIRECTORY": "${BUILD_DIRECTORY}",
                        }
                    },
                    {},
                )
            else:
                ctx = Context(
                    {},
                    {
                        "variables": {
                            **self._project.variables,
                            "BUILD_DIRECTORY": "${BUILD_DIRECTORY}",
                        }
                    },
                    {},
                )

            configs.append(Plugin.model_validate(plugin, context=ctx))

        return configs

    def _load_scripts(self, build: dict[str, object]) -> list[Script]:
        scripts = build.get("scripts")
        if scripts is None or type(scripts) is not list:
            return []

        configs: list[Script] = []
        for script in cast(list[object], scripts):
            if type(script) is not dict:
                raise ValueError("scripts entries must be a dictionary")

            enabled = cast(dict[str, object], script).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            env = cast(dict[str, object], script).get("variables", {})
            if type(env) is dict:
                ctx = Context(
                    {"root": self._root, "name": self._project.name},
                    {
                        "variables": {
                            **self._project.variables,
                            **{
                                k: v
                                for k, v in cast(dict[str, object], env).items()
                                if type(v) is str
                            },
                        },
                    },
                    {},
                )
            else:
                ctx = Context(
                    {"root": self._root, "name": self._project.name},
                    {"variables": self._project.variables},
                    {},
                )

            configs.append(Script.model_validate(script, context=ctx))

        return configs

    def _load_release_package(self, release: dict[str, object]) -> ReleasePackage:
        pkg = release.get("package")
        if pkg is None or type(pkg) is not dict:
            return ReleasePackage.model_construct(
                filename=self._project.name,
                id=self._project.name,
                name=self._project.name,
            )

        ctx = Context({}, {"variables": self._project.variables}, {})

        return ReleasePackage.model_validate(pkg, context=ctx)

    def _load_release_contents(
        self, release: dict[str, object], artifact: Artifact
    ) -> ReleaseContentContainer:
        contents = release.get("contents")
        if contents is None or type(contents) is not dict:
            raise ValueError("[release.contents] section is required")

        return ReleaseContentContainer(
            self._load_release_extension(cast(dict[str, object], contents), artifact),
            self._load_release_documents(cast(dict[str, object], contents)),
            self._load_release_assets(cast(dict[str, object], contents)),
        )

    def _load_release_extension(
        self, contents: dict[str, object], artifact: Artifact
    ) -> list[ReleaseExtension]:
        extensions = contents.get("extensions")
        if extensions is None or type(extensions) is not list:
            return []

        ctx = Context(
            {"root": self._root},
            {"variables": self._project.variables},
            {"artifact": artifact},
        )

        return [
            ReleaseExtension.model_validate(extension, context=ctx)
            for extension in cast(list[object], extensions)
        ]

    def _load_release_documents(
        self, contents: dict[str, object]
    ) -> list[ReleaseDocument]:
        documents = contents.get("documents")
        if documents is None or type(documents) is not list:
            return []

        ctx = Context(
            {"root": self._root},
            {"variables": self._project.variables},
            {},
        )

        return [
            ReleaseDocument.model_validate(document, context=ctx)
            for document in cast(list[object], documents)
        ]

    def _load_release_assets(self, contents: dict[str, object]) -> list[ReleaseAsset]:
        assets = contents.get("assets")
        if assets is None or type(assets) is not list:
            return []

        ctx = Context(
            {"root": self._root},
            {"variables": self._project.variables},
            {},
        )

        entries: list[ReleaseAsset] = []
        for asset in cast(list[object], assets):
            if type(asset) is not dict:
                raise ValueError("assets entries must be a dictionary")

            enabled = cast(dict[str, object], asset).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            entry = ReleaseAsset.model_validate(asset, context=ctx)
            if entry.sources or entry.documents:
                entries.append(entry)

        return entries


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
                    Path(p) for p in cast("list[object]", v) if isinstance(p, str)
                ]

        for k, v in artifacts.object("scripts", Json()).items():
            if isinstance(v, list):
                scripts[k] = [
                    Path(p) for p in cast("list[object]", v) if isinstance(p, str)
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
