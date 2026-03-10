import re
from logging import getLogger
from pathlib import Path
from subprocess import run
from typing import Final

from astra.core.config import Artifact, Build, Cache, Plugin, Script
from astra.core.utils import expand_variables

logger = getLogger(__name__)


class Builder:
    _dst: Path
    _root: Path

    _SECTION_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"^\s*--\s*@",
        re.MULTILINE,
    )

    _PROPERTY_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"^.*?(--[^@]*@)",
        re.MULTILINE,
    )

    _INCLUDE_PATTERN: Final[re.Pattern[str]] = re.compile(
        r"""
        ^\s*--\#include\s+(?:"([^"]+)"|<([^>]+)>)
        [^\n]*
        (?:\n\s*require\s*
        (?:\(\s*([^\n]+?)\s*\)|([^\s\n]+))
        [^\n]*
        \n[^\n]*)?
        """,
        re.MULTILINE | re.VERBOSE,
    )

    def __init__(self, dst: Path, root: Path) -> None:
        self._dst = dst
        self._root = root

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

        self._run_commands(target.commands, env, cfg.id)

        artifacts: list[Path] = []
        for a in target.artifacts:
            path = self._root / expand_variables(a, env)
            matched = sorted(path.parent.glob(path.name))
            artifacts.extend(matched if matched else [path])

        logger.info(
            "Plugin '%s' produced %d artifact(s)", cfg.id, len(artifacts)
        )
        return artifacts

    def build_script(self, cfg: Script) -> list[Path]:
        logger.info("Building script '%s'", cfg.id)

        if not cfg.sources:
            logger.warning("Script '%s' has no sources, skipping", cfg.id)
            return []

        script = ""
        for source in cfg.sources:
            for src in source.files:
                if not src.is_file():
                    raise FileNotFoundError(f"Script source not found: {src}")

                try:
                    content = src.read_text(encoding="utf-8")
                except (OSError, UnicodeDecodeError) as e:
                    logger.warning(
                        "Failed to read script source (%s): %s",
                        e.__class__.__name__,
                        src,
                    )
                    continue

                env = {**cfg.variables, **source.variables}
                includes = [src.parent, *cfg.include_directories]

                content = self._restore_section_directives(content)
                content = self._normalize_properties(content)
                content = expand_variables(content, env)
                content = self._expand_includes(content, includes)

                script += content + "\n"

        target = self._dst / "scripts" / cfg.id / cfg.name
        target.parent.mkdir(parents=True, exist_ok=True)
        _ = target.write_text(script, encoding="utf-8", newline=cfg.newline)

        logger.info("Script '%s' written to %s", cfg.id, target)

        artifacts = [target, *cfg.artifacts]

        logger.info(
            "Script '%s' produced %d artifact(s)", cfg.id, len(artifacts)
        )

        return artifacts

    def _run_commands(
        self, commands: list[str], variables: dict[str, str], plugin_id: str
    ) -> None:
        for cmd in commands:
            cmd = expand_variables(cmd, variables)
            logger.info("Running: %s", cmd)
            result = run(
                cmd,
                shell=True,
                cwd=self._root,
                capture_output=True,
                text=True,
                check=False,
            )

            if result.returncode != 0:
                stdout = result.stdout.rstrip() if result.stdout else ""
                stderr = result.stderr.rstrip() if result.stderr else ""
                msg = (
                    f"Plugin '{plugin_id}' command failed\n"
                    f"  Command:   {cmd}\n"
                    f"  Exit code: {result.returncode}\n"
                    f"  Stdout:\n{stdout}\n"
                    f"  Stderr:\n{stderr}"
                )
                logger.error(msg)
                raise RuntimeError(msg)

    def _restore_section_directives(self, text: str) -> str:
        return self._SECTION_PATTERN.sub("@", text)

    def _normalize_properties(self, text: str) -> str:
        return self._PROPERTY_PATTERN.sub(r"\1", text)

    def _expand_includes(self, text: str, includes: list[Path]) -> str:
        def _replacer(match: re.Match[str]) -> str:
            quoted = match.group(1)
            angled = match.group(2)
            module = match.group(3) or match.group(4)

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
                    return candidate.read_text(encoding="utf-8")
                except (OSError, UnicodeDecodeError) as e:
                    logger.warning(
                        "Failed to read include (%s): %s",
                        e.__class__.__name__,
                        candidate,
                    )
                    break

            logger.warning("Include not found: %s", match.group(0).strip())
            return match.group(0)

        return self._INCLUDE_PATTERN.sub(_replacer, text)


def build(dst: Path, cfg: Build, configuration: str = "release") -> Artifact:
    if not cfg.plugins and not cfg.scripts:
        logger.warning("No plugins or scripts to build")
        return Artifact()

    logger.info(
        "Build started: Destination=%s, Configuration=%s", dst, configuration
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
