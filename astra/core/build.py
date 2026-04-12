import re
import subprocess
import textwrap
from logging import getLogger
from pathlib import Path
from typing import Final

from astra.core.config import Artifact, Build, Cache, Plugin, Script
from astra.core.utils import expand_variables


logger = getLogger(__name__)


class Builder:
    _dst: Path
    _root: Path
    _encoding: str

    _SECTION_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"^[^\S\n]*--[^\S\n]*@",
        re.MULTILINE,
    )

    _PROPERTY_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"^[^\n]*?(--\w+@[^\n]*)",
        re.MULTILINE,
    )

    _DEFINE_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"""
        ^[^\S\n]*--[^\S\n]*
        (?:\[\[\s*\#[^\S\n]*define[^\S\n]+(\w+)[^\S\n]+(?s:(.+?))[^\S\n]*\]\]
        [^\n]*(?:\n|$)|
        \#[^\S\n]*define[^\S\n]+(\w+)[^\S\n]+([^\n]+)(?:\n|$))
        """,
        re.MULTILINE | re.VERBOSE,
    )

    _INCLUDE_PATTERN: Final[re.Pattern[str]] = re.compile(
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

    _INCLUDE_HLSL_PATTERN: Final[re.Pattern[str]] = re.compile(
        r'^([^\S\n]*)#[^\S\n]*include[^\S\n]+(?:"([^"\n]+)"|<([^>\n]+)>)[^\n]*(?:\n|$)',
        re.MULTILINE,
    )

    _IF_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"""
        ^[\S\n]*if\s*(?:\(\s*\.\.\.\s*\)|\.\.\.)\s*then\s*
        (?:(?!\s+else(?:if)?\s+).)+?
        \s*end[^\n]*(?:\n|$)
        """,
        re.MULTILINE | re.DOTALL | re.VERBOSE,
    )

    def __init__(self, dst: Path, root: Path) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"Not a directory: {dst}")

        if not root.is_dir():
            raise NotADirectoryError(f"Not a directory: {root}")

        self._dst = dst
        self._root = root
        self._encoding = "utf-8"

    def build_plugin(self, cfg: Plugin, configuration: str) -> list[Path]:
        logger.info("Building plugin '%s' (%s)", cfg.id, configuration)

        config = configuration.lower()
        if config == "release":
            target = cfg.release
        elif config == "debug":
            target = cfg.debug
        else:
            raise ValueError(f"Unknown configuration: {configuration}")

        if not target.commands:
            logger.warning("Plugin '%s' has no commands, skipping", cfg.id)
            return []

        env = {"BUILD_DIRECTORY": str(self._dst / "plugins" / cfg.id)}

        try:
            self._run_commands(target.commands, env)
        except Exception as e:
            cls = e.__class__.__name__
            raise RuntimeError(f"Plugin '{cfg.id}' command failed ({cls}): {e}") from e

        artifacts: list[Path] = []
        for a in target.artifacts:
            path = self._root / expand_variables(a, env)
            matched = sorted(path.parent.glob(path.name))
            artifacts.extend(matched if matched else [path])

        logger.info("Plugin '%s' produced %d artifact(s)", cfg.id, len(artifacts))
        return artifacts

    def build_script(self, cfg: Script) -> list[Path]:
        logger.info("Building script '%s'", cfg.id)

        if not cfg.sources:
            logger.warning("Script '%s' has no sources, skipping", cfg.id)
            return []

        self._encoding = cfg.source_encoding

        script = ""
        for source in cfg.sources:
            for src in source.files:
                if not src.is_file():
                    logger.warning("Script source not found: %s", src)
                    continue

                try:
                    content = src.read_text(encoding=self._encoding)
                except Exception as e:
                    cls = e.__class__.__name__
                    raise RuntimeError(
                        f"Failed to read script source ({cls}): {src}"
                    ) from e

                env = {**cfg.variables, **source.variables}
                includes = [src.parent, *cfg.include_directories]

                content = self._gather_defines(content, env)
                content = self._restore_section_directives(content)
                content = self._normalize_properties(content)
                content = expand_variables(content, env)
                content = self._expand_includes(content, includes)

                script += content + "\n"

        target = self._dst / "scripts" / cfg.id / cfg.name
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            _ = target.write_text(
                script, encoding=cfg.target_encoding, newline=cfg.newline
            )
        except Exception as e:
            cls = e.__class__.__name__
            raise RuntimeError(f"Failed to write script ({cls}): {target}") from e

        logger.info("Script '%s' written to %s", cfg.id, target)

        artifacts = [target, *cfg.artifacts]

        logger.info("Script '%s' produced %d artifact(s)", cfg.id, len(artifacts))

        return artifacts

    def _run_commands(self, commands: list[str], env: dict[str, str]) -> None:
        for cmd in commands:
            cmd = expand_variables(cmd, env)
            logger.info("Running: %s", cmd)
            _ = subprocess.run(
                cmd,
                shell=True,
                cwd=self._root,
                check=True,
            )

    def _gather_defines(self, text: str, env: dict[str, str]) -> str:
        def _replacer(match: re.Match[str]) -> str:
            key = match.group(1) or match.group(3)
            val = match.group(2) or match.group(4)
            env[key] = val
            return ""

        return self._DEFINE_PATTERN.sub(_replacer, text)

    def _restore_section_directives(self, text: str) -> str:
        return self._SECTION_PATTERN.sub("@", text)

    def _normalize_properties(self, text: str) -> str:
        return self._PROPERTY_PATTERN.sub(r"\1", text)

    def _expand_includes(self, text: str, includes: list[Path]) -> str:
        def _replacer(match: re.Match[str]) -> str:
            indent = match.group(1) or ""
            quoted = match.group(2)
            angled = match.group(3)
            module = match.group(4) or match.group(5)

            path = quoted or angled
            if path is None:
                logger.warning("Malformed include: %s", match.group(0).strip())
                return match.group(0)

            candidates = [d / path for d in includes]
            if angled:
                candidates = candidates[1:]

            for candidate in candidates:
                if not candidate.is_file():
                    continue

                if module and (m := re.search(r'(["\'])(.*?)\1', module)):
                    if m.group(2) != candidate.stem:
                        continue

                try:
                    content = candidate.read_text(encoding=self._encoding)
                except Exception as e:
                    cls = e.__class__.__name__
                    raise RuntimeError(
                        f"Failed to read include ({cls}): {candidate}"
                    ) from e

                suffix = candidate.suffix.lower()
                if suffix == ".hlsl":
                    content = self._expand_includes_hlsl(content, includes)
                elif suffix == ".lua" and module:
                    content = self._IF_PATTERN.sub("", content)

                return textwrap.indent(content, indent)

            logger.warning("Include not found: %s", match.group(0).strip())
            return match.group(0)

        return self._INCLUDE_PATTERN.sub(_replacer, text)

    def _expand_includes_hlsl(self, text: str, includes: list[Path]) -> str:
        def _replacer(match: re.Match[str]) -> str:
            indent = match.group(1) or ""
            quoted = match.group(2)
            angled = match.group(3)

            path = quoted or angled
            if path is None:
                logger.warning("Malformed include: %s", match.group(0).strip())
                return match.group(0)

            candidates = [d / path for d in includes]
            if angled:
                candidates = candidates[1:]

            for candidate in candidates:
                if not candidate.is_file():
                    continue

                try:
                    content = candidate.read_text(encoding=self._encoding)
                except Exception as e:
                    cls = e.__class__.__name__
                    raise RuntimeError(
                        f"Failed to read include ({cls}): {candidate}"
                    ) from e

                return textwrap.indent(content, indent)

            logger.warning("Include not found: %s", match.group(0).strip())
            return match.group(0)

        return self._INCLUDE_HLSL_PATTERN.sub(_replacer, text)


def build(dst: Path, cfg: Build, configuration: str = "release") -> Artifact:
    if not cfg.plugins and not cfg.scripts:
        logger.warning("No plugins or scripts to build")
        return Artifact()

    logger.info(
        "Building plugins and scripts to: %s (Configuration=%s)",
        dst,
        configuration,
    )

    dst.mkdir(parents=True, exist_ok=True)
    dst = dst.resolve()

    builder = Builder(dst, cfg.root)

    plugins: dict[str, list[Path]] = {}
    for plugin in cfg.plugins:
        plugins[plugin.id] = builder.build_plugin(plugin, configuration)

    scripts: dict[str, list[Path]] = {}
    for script in cfg.scripts:
        scripts[script.id] = builder.build_script(script)

    artifact = Artifact(plugins, scripts)

    Cache(dst / "astra.json").save_artifacts(artifact)

    logger.info(
        "Build completed: %d plugin(s), %d script(s)",
        len(plugins),
        len(scripts),
    )

    return artifact
