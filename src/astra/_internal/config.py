from __future__ import annotations

import importlib.metadata as metadata
import json
import tomllib
from collections.abc import ItemsView
from dataclasses import dataclass, field
from logging import getLogger
from pathlib import Path
from typing import ClassVar, TypeVar, cast, overload

from packaging.specifiers import SpecifierSet
from packaging.version import Version
from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    ValidationInfo,
    field_validator,
    model_validator,
)

from astra._internal.utils import expand_variables


logger = getLogger(__name__)
T = TypeVar("T", str, bool)


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


class Project(BaseModel):
    model_config: ClassVar[ConfigDict] = ConfigDict(populate_by_name=True)

    name: str
    version: str | None = None
    author: str | None = None
    requires_aviutl2: str | None = Field(default=None, alias="requires-aviutl2")
    variables: dict[str, str] = Field(default_factory=dict)

    @field_validator("version", mode="after")
    @classmethod
    def _init_version(cls, v: str | None, info: ValidationInfo) -> str | None:
        ctx = cast(dict[str, object], info.context)
        version = cast(str | None, ctx.get("version"))

        return v if version is None else version

    @model_validator(mode="after")
    def _init_variables(self, info: ValidationInfo) -> Project:
        ctx = cast(dict[str, object], info.context)
        variables = self.variables

        variables["PROJECT_NAME"] = self.name

        if (version := self.version) is not None:
            variables["PROJECT_VERSION"] = version

        if (author := self.author) is not None:
            variables["PROJECT_AUTHOR"] = author

        if (requires_aviutl2 := self.requires_aviutl2) is not None:
            variables["PROJECT_REQUIRES_AVIUTL2"] = requires_aviutl2

        variables |= cast(dict[str, str], ctx.get("defines", {}))

        return self


class Command(BaseModel):
    commands: list[str] = Field(default_factory=list)
    artifacts: list[str] = Field(default_factory=list)

    @field_validator("commands", "artifacts", mode="before")
    @classmethod
    def _init_command(cls, v: object, info: ValidationInfo) -> list[str]:
        if type(v) is not list:
            raise ValueError("commands and artifacts must be a list of strings")

        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))

        return [
            expand_variables(text, env)
            for text in cast(list[object], v)
            if type(text) is str
        ]


class Plugin(BaseModel):
    id: str
    release: Command
    debug: Command = Field(default_factory=Command)

    @model_validator(mode="after")
    def _init_debug(self) -> Plugin:
        if not (self.debug.commands or self.debug.artifacts):
            self.debug = self.release

        return self


@dataclass(frozen=True)
class ScriptSource:
    files: list[Path]
    variables: dict[str, str] = field(default_factory=dict)


class Script(BaseModel):
    model_config: ClassVar[ConfigDict] = ConfigDict(populate_by_name=True)

    id: str
    name: str = Field(default="", validate_default=True)
    prefix: str = ""
    suffix: str = ""
    newline: str = "\r\n"
    source_encoding: str = Field(default="utf-8", alias="source-encoding")
    target_encoding: str = Field(default="utf-8", alias="target-encoding")
    variables: dict[str, str] = Field(default_factory=dict)
    include_directories: list[Path] = Field(
        default_factory=list, alias="include-directories"
    )
    sources: list[ScriptSource] = Field(default_factory=list)
    artifacts: list[Path] = Field(default_factory=list)

    @field_validator("name", mode="after")
    @classmethod
    def _init_name(cls, v: str, info: ValidationInfo) -> str:
        ctx = cast(dict[str, object], info.context)
        name = cast(str, ctx.get("name", ""))

        return v if v else name

    @field_validator("variables", mode="before")
    @classmethod
    def _init_variables(cls, _: object) -> dict[str, str]:
        return {}

    @field_validator("sources", mode="before")
    @classmethod
    def _init_sources(cls, v: object, info: ValidationInfo) -> list[ScriptSource]:
        if type(v) is not list:
            raise ValueError("build.scripts.sources must be a list of dicts")

        ctx = cast(dict[str, object], info.context)
        root = cast(Path, ctx.get("root", Path().cwd()))
        env = cast(dict[str, str], ctx.get("variables", {}))

        sources: list[ScriptSource] = []
        for src in cast(list[object], v):
            if type(src) is not dict:
                raise ValueError("build.scripts.sources must be a list of dicts")

            file = cast(dict[str, object], src).get("file")
            if type(file) is not str:
                raise ValueError("build.scripts.sources.file is required")

            matched = sorted(root.glob(expand_variables(file, env)))

            sources.append(
                ScriptSource(
                    matched if matched else [root / file],
                    {
                        k: v
                        for k, v in cast(dict[str, object], src).items()
                        if type(v) is str and k != "file"
                    },
                )
            )

        return sources

    @field_validator("artifacts", "include_directories", mode="before")
    @classmethod
    def _init_paths(cls, v: object, info: ValidationInfo) -> list[Path]:
        if type(v) is not list:
            raise ValueError(
                "artifacts and include-directories must be a list of strings"
            )

        ctx = cast(dict[str, object], info.context)
        root = cast(Path, ctx.get("root", Path().cwd()))
        env = cast(dict[str, str], ctx.get("variables", {}))

        paths: list[Path] = []
        for path in cast(list[object], v):
            if type(path) is not str:
                raise ValueError(
                    "artifacts and include-directories must be a list of strings"
                )

            matched = sorted(root.glob(expand_variables(path, env)))
            paths.extend(matched if matched else [root / path])

        return paths

    @model_validator(mode="after")
    def _set_variables(self, info: ValidationInfo) -> Script:
        ctx = cast(dict[str, object], info.context)
        self.variables = {
            **cast(dict[str, str], ctx.get("variables", {})),
            "SCRIPT_NAME": self.name,
        }

        return self


@dataclass(frozen=True)
class Build:
    cwd: Path
    plugins: list[Plugin] = field(default_factory=list)
    scripts: list[Script] = field(default_factory=list)


class ReleasePackage(BaseModel):
    model_config: ClassVar[ConfigDict] = ConfigDict(populate_by_name=True)

    filename: str = Field(default="", validate_default=True)
    name: str = Field(default="", validate_default=True)
    id: str = Field(default="", validate_default=True)
    uninstall_subdirectory_files: bool = Field(
        default=False, alias="uninstall-subdirectory-files"
    )
    information: str | None = None
    version: str | None = None
    author: str | None = None
    license: str | None = None
    summary: str | None = None
    description: str | None = None
    website: str | None = None
    report_issue: str | None = Field(default=None, alias="report-issue")

    @field_validator("filename", "name", "id", mode="after")
    @classmethod
    def _init_config(cls, v: str, info: ValidationInfo) -> str:
        ctx = cast(dict[str, object], info.context)
        name = cast(str, ctx.get("name", ""))
        env = cast(dict[str, str], ctx.get("variables", {}))

        if not v:
            return name

        return expand_variables(v, env)

    @field_validator(
        "information",
        "license",
        "summary",
        "description",
        "website",
        "report_issue",
        mode="after",
    )
    @classmethod
    def _init_info(cls, v: str | None, info: ValidationInfo) -> str | None:
        if v is None:
            return None

        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        return expand_variables(v, env)

    @field_validator("version", "author", mode="before")
    @classmethod
    def _init_metadata(cls, _: str | None) -> str | None:
        return None

    @model_validator(mode="after")
    def _set_metadata(self, info: ValidationInfo) -> ReleasePackage:
        ctx = cast(dict[str, object], info.context)
        self.version = cast(str | None, ctx.get("version", None))
        self.author = cast(str | None, ctx.get("author", None))

        return self


class ReleaseExtension(BaseModel):
    directory: str = ""
    files: list[Path] = Field(default_factory=list)

    @field_validator("directory", mode="after")
    @classmethod
    def _init_directory(cls, v: str, info: ValidationInfo) -> str:
        data = (
            "Plugin/",
            "Script/",
            "Language/",
            "Alias/",
            "Default/",
            "Figure/",
            "Preset/",
            "Transition/",
        )

        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        v = expand_variables(v, env).replace("\\", "/")

        if v.startswith(data):
            return v

        logger.warning(f"{v} is not a package hierarchy")
        return ""

    @field_validator("files", mode="before")
    @classmethod
    def _init_files(cls, v: object, info: ValidationInfo) -> list[Path]:
        if type(v) is not list:
            raise ValueError(
                "release.contents.extensions.files must be a list of strings"
            )

        ctx = cast(dict[str, object], info.context)
        root = cast(Path, ctx.get("root", Path().cwd()))
        env = cast(dict[str, str], ctx.get("variables", {}))
        artifact = cast(Artifact, ctx.get("artifact", Artifact()))

        extensions: list[Path] = []
        for extension in cast(list[object], v):
            if type(extension) is not str:
                raise ValueError(
                    "release.contents.extensions.files must be a list of strings"
                )

            prefix, _, identifier = extension.partition(":")

            if prefix == "script" and identifier in artifact.script:
                extensions.extend(artifact.script[identifier])
            elif prefix == "plugin" and identifier in artifact.plugin:
                extensions.extend(artifact.plugin[identifier])
            else:
                matched = sorted(root.glob(expand_variables(extension, env)))
                extensions.extend(matched if matched else [root / extension])

        if not extensions:
            raise ValueError("release.contents.extensions.files is required")

        return extensions


class ReleaseDocument(BaseModel):
    directory: str = ""
    files: list[Path] = Field(default_factory=list)

    @field_validator("directory", mode="after")
    @classmethod
    def _init_directory(cls, v: str, info: ValidationInfo) -> str:
        data = (
            "Plugin/",
            "Script/",
            "Language/",
            "Alias/",
            "Default/",
            "Figure/",
            "Preset/",
            "Transition/",
        )

        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        v = expand_variables(v, env).replace("\\", "/")

        if v.startswith(data):
            return v

        logger.warning(f"{v} is not a package hierarchy")
        return ""

    @field_validator("files", mode="before")
    @classmethod
    def _init_files(cls, v: object, info: ValidationInfo) -> list[Path]:
        if type(v) is not list:
            raise ValueError(
                "release.contents.documents.files must be a list of strings"
            )

        ctx = cast(dict[str, object], info.context)
        root = cast(Path, ctx.get("root", Path().cwd()))
        env = cast(dict[str, str], ctx.get("variables", {}))

        documents: list[Path] = []
        for document in cast(list[object], v):
            if type(document) is not str:
                raise ValueError(
                    "release.contents.documents.files must be a list of strings"
                )

            matched = sorted(root.glob(expand_variables(document, env)))
            documents.extend(matched if matched else [root / document])

        if not documents:
            raise ValueError("release.contents.documents.files is required")

        return documents


class AssetSource(BaseModel):
    directory: str = ""
    files: list[Path | str] = Field(default_factory=list)

    @field_validator("directory", mode="after")
    @classmethod
    def _init_directory(cls, v: str, info: ValidationInfo) -> str:
        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        return expand_variables(v, env)

    @field_validator("files", mode="before")
    @classmethod
    def _init_files(cls, v: object, info: ValidationInfo) -> list[Path | str]:
        if type(v) is not list:
            raise ValueError(
                "release.contents.assets.sources.files must be a list of strings"
            )

        ctx = cast(dict[str, object], info.context)
        root = cast(Path, ctx.get("root", Path().cwd()))
        env = cast(dict[str, str], ctx.get("variables", {}))

        files: list[Path | str] = []
        for file in cast(list[object], v):
            if type(file) is not str:
                raise ValueError(
                    "release.contents.assets.sources.files must be a list of strings"
                )

            if file.startswith(("http://", "https://")):
                files.append(file)
                continue

            matched = sorted(root.glob(expand_variables(file, env)))
            files.extend(matched if matched else [root / file])

        if not files:
            raise ValueError("release.contents.assets.sources.files is required")

        return files


class AssetDocument(BaseModel):
    filename: str
    content: str = ""

    @field_validator("filename", "content", mode="after")
    @classmethod
    def _init_document(cls, v: str, info: ValidationInfo) -> str:
        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        return expand_variables(v, env)


class ReleaseAsset(BaseModel):
    name: str
    directory: str = ""
    sources: list[AssetSource] = Field(default_factory=list)
    documents: list[AssetDocument] = Field(default_factory=list)

    @field_validator("directory", mode="after")
    @classmethod
    def _init_directory(cls, v: str, info: ValidationInfo) -> str:
        data = (
            "Plugin/",
            "Script/",
            "Language/",
            "Alias/",
            "Default/",
            "Figure/",
            "Preset/",
            "Transition/",
        )

        ctx = cast(dict[str, object], info.context)
        env = cast(dict[str, str], ctx.get("variables", {}))
        v = expand_variables(v, env).replace("\\", "/")

        if v.startswith(data):
            return v

        logger.warning(f"{v} is not a package hierarchy")
        return ""


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

        return Install(
            self._load_release_extension(cast(dict[str, object], contents), artifact)
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
                raise ValueError("Version mismatch")

    def _load_project(self, version: str | None, defines: dict[str, str]) -> None:
        project = self._data.get("project")
        if project is None or type(project) is not dict:
            raise ValueError("[project] section is required")

        ctx = {"version": version, "defines": defines}

        self._project = Project.model_validate(project, context=ctx)

    def _load_plugins(self, build: dict[str, object]) -> list[Plugin]:
        plugins = build.get("plugins")
        if plugins is None or type(plugins) is not list:
            return []

        configs: list[Plugin] = []
        for plugin in cast(list[object], plugins):
            if type(plugin) is not dict:
                continue

            enabled = cast(dict[str, object], plugin).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            env = cast(dict[str, object], plugin).get("variables")
            if type(env) is dict:
                ctx = {
                    "variables": {
                        **self._project.variables,
                        **{
                            k: v
                            for k, v in cast(dict[str, object], env).items()
                            if type(v) is str
                        },
                        "BUILD_DIRECTORY": "${BUILD_DIRECTORY}",
                    }
                }
            else:
                ctx = {
                    "variables": {
                        **self._project.variables,
                        "BUILD_DIRECTORY": "${BUILD_DIRECTORY}",
                    }
                }

            configs.append(Plugin.model_validate(plugin, context=ctx))

        return configs

    def _load_scripts(self, build: dict[str, object]) -> list[Script]:
        scripts = build.get("scripts")
        if scripts is None or type(scripts) is not list:
            return []

        configs: list[Script] = []
        for script in cast(list[object], scripts):
            if type(script) is not dict:
                continue

            enabled = cast(dict[str, object], script).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            env = cast(dict[str, object], script).get("variables", {})
            if type(env) is dict:
                ctx = {
                    "root": self._root,
                    "name": self._project.name,
                    "variables": {
                        **self._project.variables,
                        **{
                            k: v
                            for k, v in cast(dict[str, object], env).items()
                            if type(v) is str
                        },
                    },
                }
            else:
                ctx = {
                    "root": self._root,
                    "name": self._project.name,
                    "variables": self._project.variables,
                }

            configs.append(Script.model_validate(script, context=ctx))

        return configs

    def _load_release_package(self, release: dict[str, object]) -> ReleasePackage:
        pkg = release.get("package")
        if pkg is None or type(pkg) is not dict:
            return ReleasePackage.model_construct(
                filename=self._project.name,
                name=self._project.name,
                id=self._project.name,
            )

        ctx = {
            "name": self._project.name,
            "version": self._project.version,
            "author": self._project.author,
            "variables": self._project.variables,
        }

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

        ctx = {
            "root": self._root,
            "variables": self._project.variables,
            "artifact": artifact,
        }

        items: list[ReleaseExtension] = []
        for extension in cast(list[object], extensions):
            item = ReleaseExtension.model_validate(extension, context=ctx)
            if item.directory:
                items.append(item)

        return items

    def _load_release_documents(
        self, contents: dict[str, object]
    ) -> list[ReleaseDocument]:
        documents = contents.get("documents")
        if documents is None or type(documents) is not list:
            return []

        ctx = {
            "root": self._root,
            "variables": self._project.variables,
        }

        items: list[ReleaseDocument] = []
        for document in cast(list[object], documents):
            item = ReleaseDocument.model_validate(document, context=ctx)
            if item.files and item.directory:
                items.append(item)

        return items

    def _load_release_assets(self, contents: dict[str, object]) -> list[ReleaseAsset]:
        assets = contents.get("assets")
        if assets is None or type(assets) is not list:
            return []

        ctx = {
            "root": self._root,
            "variables": self._project.variables,
        }

        items: list[ReleaseAsset] = []
        for asset in cast(list[object], assets):
            if type(asset) is not dict:
                continue

            enabled = cast(dict[str, object], asset).get("enabled", True)
            if type(enabled) is not bool or not enabled:
                continue

            item = ReleaseAsset.model_validate(asset, context=ctx)
            if item.sources and item.directory:
                items.append(item)

        return items


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
