import ctypes
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
from logging import getLogger
from pathlib import Path
from typing import cast, final

from astra._internal.config import Artifact, Build, Plugin, Script
from astra._internal.utils import expand_variables, resolve_glob


logger = getLogger(__name__)


@final
class Builder:
    _dst: Path
    _root: Path
    _configuration: str

    _SECTION_PATTERN = re.compile(
        r"^[^\S\n]*--[^\S\n]*@",
        re.MULTILINE,
    )

    _PROPERTY_PATTERN = re.compile(
        r"^[^\n]*?(--\w+@[^\n]*)",
        re.MULTILINE,
    )

    _DEFINE_PATTERN = re.compile(
        r"""
        ^[^\S\n]*--[^\S\n]*
        (?:\[\[\s*\#[^\S\n]*define[^\S\n]+(\w+)[^\S\n]+(?s:(.+?))[^\S\n]*\]\]
        [^\n]*(?:\n|$)|
        \#[^\S\n]*define[^\S\n]+(\w+)[^\S\n]+([^\n]+)(?:\n|$))
        """,
        re.MULTILINE | re.VERBOSE,
    )

    _SCRIPT_INCLUDE_PATTERN = re.compile(
        r"""
        ^([^\S\n]*)--[^\S\n]*\#[^\S\n]*include[^\S\n]+(?:"([^"\n]+)"|<([^>\n]+)>)
        [^\n]*
        (?:\n[^\n]*?require\s*
        (?:\(\s*([^\n]+?)\s*\)|([^\s]+))
        [^\n]*
        (?:\n[^\n]*)?)?(?:\n|$)
        """,
        re.MULTILINE | re.VERBOSE,
    )

    _SHADER_INCLUDE_PATTERN = re.compile(
        r'^([^\S\n]*)#[^\S\n]*include[^\S\n]+(?:"([^"\n]+)"|<([^>\n]+)>)[^\n]*(?:\n|$)',
        re.MULTILINE,
    )

    _IF_PATTERN = re.compile(
        r"""
        ^[\S\n]*if\s*(?:\(\s*\.\.\.\s*\)|\.\.\.)\s*then\s*
        (?:(?!\s+else(?:if)?\s+).)+?
        \s*end[^\n]*(?:\n|$)
        """,
        re.MULTILINE | re.DOTALL | re.VERBOSE,
    )

    def __init__(self, dst: Path, root: Path, configuration: str) -> None:
        root = root.resolve()
        if not root.is_dir():
            raise NotADirectoryError(f"'{root}' is not a directory")

        configuration = configuration.lower()
        if configuration not in ("release", "debug"):
            raise ValueError(f"{configuration.capitalize()} is not supported")

        dst = dst.resolve()
        if dst.is_file() or dst.is_symlink():
            raise NotADirectoryError(f"'{dst}' is not a directory")

        dst.mkdir(parents=True, exist_ok=True)

        self._dst = dst
        self._root = root
        self._configuration = configuration

    def build(self, cfg: Plugin | Script) -> list[str]:
        if isinstance(cfg, Plugin):
            logger.info(f"Building plugin '{cfg.id}'")

            if self._configuration == "release":
                target = cfg.release
            else:
                target = cfg.debug

            dst = self._dst / "plugins" / cfg.id
            dst.mkdir(parents=True, exist_ok=True)
            try:
                dst = dst.relative_to(self._root, walk_up=True)
                root = self._root
                variables = {**cfg.variables, "BUILD_DIRECTORY": str(dst)}
            except ValueError:
                root = dst
                variables = {**cfg.variables, "BUILD_DIRECTORY": "."}

            if len(target.commands) > 0:
                try:
                    self._run_commands(target.commands, dst, cfg.shell, cfg.variables)
                except Exception:
                    logger.error(f"Plugin '{cfg.id}' commands failed")
                    raise
            else:
                logger.warning(f"Plugin '{cfg.id}' has no commands")

            return [
                str(path)
                for artifact in target.artifacts
                for path in resolve_glob(root, expand_variables(artifact, variables))
            ]
        else:
            logger.info(f"Building script '{cfg.id}'")

            artifacts = list(map(str, cfg.artifacts))

            if len(cfg.sources) == 0:
                logger.warning(f"Script '{cfg.id}' has no sources")
                return artifacts

            filename = f"{cfg.prefix}{cfg.name}{cfg.suffix}"

            script = ""
            try:
                for source in cfg.sources:
                    for src in source.files:
                        if not src.is_file():
                            raise FileNotFoundError(f"'{src}' is not found")

                        content = src.read_text(encoding=cfg.source_encoding)

                        variables = {**cfg.variables, **source.variables}
                        includes = [src.parent, *cfg.include_directories]

                        content = self._gather_defines(content, variables)
                        content = self._restore_section_directives(content)
                        content = self._normalize_properties(content)
                        content = expand_variables(content, variables)
                        content = self._expand_script_includes(
                            content, includes, cfg.source_encoding
                        )

                        script += f"{content}\n"

                target = self._dst / "scripts" / cfg.id / filename
                target.parent.mkdir(parents=True, exist_ok=True)

                logger.info(f"Writing script to '{target}'")

                _ = target.write_text(
                    f"{script.strip()}\n",
                    encoding=cfg.target_encoding,
                    newline=cfg.newline,
                )
            except Exception:
                logger.error(f"Script '{cfg.id}' build failed")
                raise

            return [str(target), *artifacts]

    def _run_commands(
        self,
        commands: list[str],
        dst: Path,
        shell: str | None,
        variables: dict[str, str],
    ) -> None:
        if shell is None:
            logger.info("Using default shell")

            env = {**variables, "BUILD_DIRECTORY": str(dst)}

            for cmd in commands:
                cmd = expand_variables(cmd, env)
                logger.info(f"Executing '{cmd}' in '{self._root}'")
                _ = subprocess.run(cmd, cwd=self._root, check=True, shell=True)
        elif (shell := shutil.which(shell)) is not None:
            env = variables.copy()

            name = shell.rpartition("/")[-1].rpartition("\\")[-1].rsplit(".", 1)[0]
            name = name.lower()
            if name == "cmd" and sys.platform == "win32":
                logger.info(f"Using cmd ({shell})")
                logger.info("Options: /d /e:on /v:off /s /c")

                ctypes.windll.kernel32.GetACP.restype = ctypes.c_uint
                cp = cast(int, ctypes.windll.kernel32.GetACP())

                env["BUILD_DIRECTORY"] = dst.as_posix().replace("/", "\\")
                args = [shell, "/d", "/e:on", "/v:off", "/s", "/c"]
                suffix = ".bat"
                encoding = f"cp{cp if cp != 0 else 437}"
                newline = "\r\n"
                script = "@echo off\nsetlocal\n{content}\nendlocal\n"

                logger.info(f"Codepage: {encoding}")
            elif name == "powershell":
                logger.info(f"Using powershell ({shell})")
                logger.info("Options: -NoProfile -ExecutionPolicy Bypass -File")

                env["BUILD_DIRECTORY"] = dst.as_posix()
                args = [shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
                suffix = ".ps1"
                encoding = "utf-8-sig"
                newline = "\n"
                script = (
                    "$ErrorActionPreference='Stop'\n"
                    "Set-StrictMode -Version Latest\n"
                    "{content}\n"
                )
            elif name == "pwsh":
                logger.info(f"Using pwsh ({shell})")
                logger.info("Options: -NoProfile -ExecutionPolicy Bypass -File")

                env["BUILD_DIRECTORY"] = dst.as_posix()
                args = [shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
                suffix = ".ps1"
                encoding = "utf-8"
                newline = "\n"
                script = (
                    "$ErrorActionPreference='Stop'\n"
                    "Set-StrictMode -Version Latest\n"
                    "$PSNativeCommandUseErrorActionPreference = $true\n"
                    "{content}\n"
                )
            elif name.endswith("bash"):
                logger.info(f"Using bash ({shell})")
                logger.info("Options: --noprofile --norc -euo pipefail")

                env["BUILD_DIRECTORY"] = dst.as_posix()
                args = [shell, "--noprofile", "--norc", "-euo", "pipefail"]
                suffix = ".sh"
                encoding = "utf-8"
                newline = "\n"
                script = "{content}\n"
            elif name.endswith("zsh"):
                logger.info(f"Using zsh ({shell})")
                logger.info("Options: -f -euo pipefail")

                env["BUILD_DIRECTORY"] = dst.as_posix()
                args = [shell, "-f", "-euo", "pipefail"]
                suffix = ".sh"
                encoding = "utf-8"
                newline = "\n"
                script = "{content}\n"
            elif name.endswith("sh"):
                logger.info(f"Using sh ({shell})")
                logger.info("Options: -eu")

                env["BUILD_DIRECTORY"] = dst.as_posix()
                args = [shell, "-eu"]
                suffix = ".sh"
                encoding = "utf-8"
                newline = "\n"
                script = "{content}\n"
            else:
                raise NotImplementedError(f"'{shell}' is not supported")

            env |= os.environ

            tmp = self._dst / "plugins"
            tmp.mkdir(parents=True, exist_ok=True)

            for cmd in commands:
                with tempfile.NamedTemporaryFile(
                    mode="w",
                    encoding=encoding,
                    newline=newline,
                    suffix=suffix,
                    dir=tmp,
                    delete=False,
                ) as f:
                    content = script.format(content=cmd)
                    _ = f.write(content)
                    path = f.name

                    logger.info(f"Created script file: {path}")
                    logger.info(f"Script content:\n{content}")

                try:
                    logger.info(f"Executing '{path}' in '{self._root}'")
                    _ = subprocess.run(
                        [*args, path],
                        env=env,
                        cwd=self._root,
                        check=True,
                    )
                finally:
                    Path(path).unlink()
        else:
            raise RuntimeError("Failed to find any shell")

    def _gather_defines(self, text: str, variables: dict[str, str]) -> str:
        def _replacer(match: re.Match[str]) -> str:
            key = match.group(1) or match.group(3)
            val = match.group(2) or match.group(4)

            if key in ("", None) or val in ("", None):
                logger.warning(f"'{match.group(0)}' is not a valid define")
                return match.group(0)

            variables[key] = val
            return ""

        return self._DEFINE_PATTERN.sub(_replacer, text)

    def _restore_section_directives(self, text: str) -> str:
        return self._SECTION_PATTERN.sub("@", text)

    def _normalize_properties(self, text: str) -> str:
        return self._PROPERTY_PATTERN.sub(r"\1", text)

    def _find_include(
        self, quoted: str | None, angled: str | None, includes: list[Path]
    ) -> Path | None:
        path = quoted or angled
        if path in (None, ""):
            return None

        if quoted is not None and (file := includes[0] / path).is_file():
            return file

        for d in includes[1:]:
            if (file := d / path).is_file():
                return file

        return None

    def _expand_script_includes(
        self, text: str, includes: list[Path], encoding: str
    ) -> str:
        def _replacer(match: re.Match[str]) -> str:
            candidate = self._find_include(match.group(2), match.group(3), includes)

            if candidate is None:
                logger.warning(f"'{match.group(0)}' is not found")
                return match.group(0)

            indent = match.group(1) or ""
            module = match.group(4) or match.group(5)
            if (
                module not in (None, "")
                and (m := re.search(r'(["\'])(.*?)\1', module)) is not None
            ):
                module = m.group(2)
            else:
                module = None

            suffix = candidate.suffix.lower()
            if suffix == ".lua":
                if module is not None and module != candidate.stem:
                    logger.warning(f"'{match.group(0)}' does not match the module name")
                    return match.group(0)

                content = candidate.read_text(encoding=encoding)
                content = self._IF_PATTERN.sub("", content)
            elif suffix == ".hlsl":
                content = candidate.read_text(encoding=encoding)
                content = self._expand_shader_includes(content, includes, encoding)
            else:
                content = candidate.read_text(encoding=encoding)

            return textwrap.indent(content, indent)

        return self._SCRIPT_INCLUDE_PATTERN.sub(_replacer, text)

    def _expand_shader_includes(
        self, text: str, includes: list[Path], encoding: str
    ) -> str:
        def _replacer(match: re.Match[str]) -> str:
            candidate = self._find_include(match.group(2), match.group(3), includes)

            if candidate is None:
                logger.warning(f"'{match.group(0)}' is not found")
                return match.group(0)

            indent = match.group(1) or ""
            content = candidate.read_text(encoding=encoding)
            return textwrap.indent(content, indent)

        return self._SHADER_INCLUDE_PATTERN.sub(_replacer, text)


def build(dst: Path, cfg: Build, configuration: str = "release") -> Artifact:
    if len(cfg.plugins) == 0 and len(cfg.scripts) == 0:
        logger.warning("Build target is empty")
        return Artifact()

    builder = Builder(dst, cfg.root, configuration)

    plugins: dict[str, list[str]] = {}
    if len(cfg.plugins) > 0:
        for plugin in cfg.plugins:
            plugins[plugin.id] = builder.build(plugin)

        logger.info(f"{len(plugins)} plugin(s) completed")

    scripts: dict[str, list[str]] = {}
    if len(cfg.scripts) > 0:
        for script in cfg.scripts:
            scripts[script.id] = builder.build(script)

        logger.info(f"{len(scripts)} script(s) completed")

    return Artifact(plugins, scripts)
