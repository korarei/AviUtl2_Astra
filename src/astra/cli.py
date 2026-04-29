from __future__ import annotations

import logging
import os
import shutil
import sys
import tempfile
from argparse import ArgumentParser
from pathlib import Path
from typing import Callable, Protocol, cast, final, override

from filelock import FileLock

from astra._internal import build, init, install, release, run, schema, venv
from astra._internal.config import (
    Artifact,
    Build,
    Cache,
    Config,
    Extension,
    Install,
    Project,
    Release,
)
from astra._internal.utils import fetch_version, find_config


logger = logging.getLogger(__name__)


class CommandArgs(Protocol):
    command: str
    venv: Path | None
    func: Callable[[CommandArgs], None]


class InitArgs(CommandArgs, Protocol):
    target: Path
    func: Callable[[InitArgs], None]


class BuildArgs(CommandArgs, Protocol):
    build: Path
    config: str
    version: str | None
    define: list[list[str]]
    func: Callable[[BuildArgs], None]


class ReleaseArgs(CommandArgs, Protocol):
    target: Path
    version: str | None
    define: list[list[str]]
    func: Callable[[ReleaseArgs], None]


class InstallArgs(CommandArgs, Protocol):
    target: Path | None
    build: Path
    editable: bool
    define: list[list[str]]
    func: Callable[[InstallArgs], None]


class UninstallArgs(CommandArgs, Protocol):
    build: Path
    func: Callable[[UninstallArgs], None]


class CleanArgs(CommandArgs, Protocol):
    build: Path
    func: Callable[[CleanArgs], None]


class SchemaArgs(CommandArgs, Protocol):
    target: Path | None
    func: Callable[[SchemaArgs], None]


class VenvArgs(CommandArgs, Protocol):
    target: Path
    aviutl2: str | None
    func: Callable[[VenvArgs], None]


class RunArgs(CommandArgs, Protocol):
    target: Path
    build: Path
    config: str
    version: str | None
    define: list[list[str]]
    aviutl2: str | None
    func: Callable[[RunArgs], None]


def _init(args: InitArgs) -> None:
    try:
        init.init(args.target)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _build(args: BuildArgs) -> None:
    try:
        dst = args.build
        defines = {k: v for k, v in args.define}

        cfg = Config(find_config(), args.version, defines).load(Build)

        with FileLock(dst / ".astra-lock"):
            artifact = build.build(dst, cfg, args.config)
            Cache(dst / "astra.json").save(artifact)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _release(args: ReleaseArgs) -> None:
    try:
        dst = args.target.resolve()

        if dst.is_file() or dst.is_symlink():
            logger.error(f"'{dst}' is not a directory")
            sys.exit(1)

        dst.mkdir(parents=True, exist_ok=True)

        defines = {k: v for k, v in args.define}

        cfg = Config(find_config(), args.version, defines)

        with tempfile.TemporaryDirectory(dir=dst) as tmp:
            artifact = build.build(Path(tmp), cfg.load(Build), "release")
            release.release(dst, cfg.load(Release, artifact))
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _install(args: InstallArgs) -> None:
    build_dir = args.build

    if not build_dir.is_dir() or not (build_dir / "astra.json").is_file():
        logger.error(f"'{build_dir}' is not a valid build directory")
        sys.exit(1)

    if args.venv is not None:
        default = args.venv / "aviutl2/data"
    elif sys.platform == "win32":
        default = Path(os.getenv("ProgramData", "C:\\ProgramData")) / "aviutl2"
    else:
        default = None

    dst = args.target or default
    if dst is None:
        logger.error("Target not specified")
        sys.exit(1)

    dst = dst.resolve()
    if not dst.is_dir():
        logger.error(f"'{dst}' is not a directory")
        sys.exit(1)

    name = dst.name.lower()
    if name not in ("aviutl2", "data"):
        logger.error(f"'{dst}' is not an AviUtl ExEdit2 data directory")
        sys.exit(1)
    elif name == "aviutl2" and default is not None and dst != default:
        logger.error(f"'{dst}' is not an AviUtl ExEdit2 data directory")
        sys.exit(1)
    elif name == "data" and not (dst.parent / "aviutl2.exe").is_file():
        logger.error(f"'{dst}' is not an AviUtl ExEdit2 data directory")
        sys.exit(1)

    try:
        defines = {k: v for k, v in args.define}

        with FileLock(build_dir / ".astra-lock"):
            cache = Cache(build_dir / "astra.json")
            artifact = cache.load(Artifact)
            cfg = Config(find_config(), defines=defines).load(Install, artifact)
            extension = install.install(dst, cfg, args.editable)
            cache.save(extension)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _uninstall(args: UninstallArgs) -> None:
    build_dir = args.build

    if not build_dir.is_dir() or not (build_dir / "astra.json").is_file():
        logger.error(f"'{build_dir}' is not a valid build directory")
        sys.exit(1)

    try:
        with FileLock(build_dir / ".astra-lock"):
            cache = Cache(build_dir / "astra.json")
            install.uninstall(cache.load(Extension))
            cache.save(Extension())
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _clean(args: CleanArgs) -> None:
    try:
        if (path := args.venv) is not None and path.is_dir():
            path = path.resolve()
            if input(f"Remove virtual environment '{path}'? (y/N): ").lower() == "y":
                if (path / "aviutl2").is_dir():
                    shutil.rmtree(path)
                    logger.info(f"'{path}' is removed")

        target = args.build.resolve()
        if not target.is_dir():
            logger.info(f"'{target}' does not exist")
            return

        logger.info(f"Cleaning '{target}'")

        if target.is_dir() and (target / "astra.json").is_file():
            cache = Cache(target / "astra.json")
            install.uninstall(cache.load(Extension))
            shutil.rmtree(target)
        else:
            logger.warning(f"'{target}' is not a valid build directory")
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _schema(args: SchemaArgs) -> None:
    try:
        schema.schema(args.target)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _venv(args: VenvArgs) -> None:
    try:
        cfg = Config(find_config()).load(Project)
        venv.venv(args.target, cfg, args.aviutl2)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def _run(args: RunArgs) -> None:
    try:
        venv_dir = args.venv
        build_dir = args.build
        defines = {k: v for k, v in args.define}
        cfg = Config(find_config(), args.version, defines)

        if venv_dir is None:
            venv.venv(args.target, cfg.load(Project), args.aviutl2)
            venv_dir = args.target

        data_dir = (venv_dir / "aviutl2/data").resolve()
        if not data_dir.is_dir():
            logger.error(f"'{data_dir}' is not a directory")
            sys.exit(1)

        with FileLock(build_dir / ".astra-lock"):
            cache = Cache(build_dir / "astra.json")
            install.uninstall(cache.load(Extension))
            artifact = build.build(build_dir, cfg.load(Build), args.config)
            cache.save(artifact)
            extension = install.install(data_dir, cfg.load(Install, artifact), True)
            cache.save(extension)
            run.run(data_dir.parent)
    except Exception as e:
        logger.error(f"{type(e).__name__} - {e}")
        sys.exit(1)


def create_parser() -> ArgumentParser:
    parser = ArgumentParser(
        prog="astra",
        description="A build and deployment tool for AviUtl ExEdit2 extensions",
    )

    _ = parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"%(prog)s {fetch_version()}",
        help="Show the version of the program",
    )

    _ = parser.add_argument(
        "--venv",
        type=Path,
        default=None,
        help="Virtual environment path (default: None)",
    )

    sub = parser.add_subparsers(
        dest="command",
        required=True,
        help="The command to execute",
    )

    p_init = sub.add_parser(
        "init",
        help="Initialize a new project with a default astra.toml",
    )
    _ = p_init.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path("."),
        help="Project directory to initialize (default: .)",
    )
    p_init.set_defaults(func=_init)

    p_build = sub.add_parser(
        "build",
        help="Build the project from the config file",
    )
    _ = p_build.add_argument(
        "build",
        type=Path,
        nargs="?",
        default=Path("build"),
        help="Build destination directory (default: build)",
    )
    _ = p_build.add_argument(
        "-c",
        "--config",
        type=str,
        default="Debug",
        help="Build configuration (e.g. Release, Debug)",
    )
    _ = p_build.add_argument(
        "-v",
        "--version",
        type=str,
        default=None,
        help='Override the project version (e.g., "1.0.0")',
    )
    _ = p_build.add_argument(
        "-d",
        "--define",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable (e.g., '-d DEBUG 1')",
    )
    p_build.set_defaults(func=_build)

    p_release = sub.add_parser(
        "release",
        help="Package the project for release",
    )
    _ = p_release.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path("release"),
        help="Build directory (default: release)",
    )
    _ = p_release.add_argument(
        "-v",
        "--version",
        type=str,
        default=None,
        help='Override the project version (e.g., "1.0.0")',
    )
    _ = p_release.add_argument(
        "-d",
        "--define",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable (e.g., '-d DEBUG 1')",
    )
    p_release.set_defaults(func=_release)

    p_install = sub.add_parser(
        "install",
        help="Install the extensions to a target directory",
    )
    _ = p_install.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=None,
        help=(
            "Target directory for installation (Defaults to %%ProgramData%%/aviutl2, "
            "unless a virtual environment exists, in which case it uses the "
            "venv's data directory)"
        ),
    )
    _ = p_install.add_argument(
        "-b",
        "--build",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    _ = p_install.add_argument(
        "-e",
        "--editable",
        action="store_true",
        help="Install the extensions in editable mode",
    )
    _ = p_install.add_argument(
        "-d",
        "--define",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable (e.g., '-d DEBUG 1')",
    )
    p_install.set_defaults(func=_install)

    p_uninstall = sub.add_parser(
        "uninstall",
        help="Uninstall the extensions from a target directory",
    )
    _ = p_uninstall.add_argument(
        "-b",
        "--build",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    p_uninstall.set_defaults(func=_uninstall)

    p_clean = sub.add_parser(
        "clean",
        help="Clean the build directory",
    )
    _ = p_clean.add_argument(
        "build",
        type=Path,
        nargs="?",
        default=Path("build"),
        help="Build directory to clean (default: build)",
    )
    p_clean.set_defaults(func=_clean)

    p_schema = sub.add_parser(
        "schema",
        help="Output the JSON schema for astra.toml",
    )
    _ = p_schema.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=None,
        help="Output directory path (default: stdout)",
    )
    p_schema.set_defaults(func=_schema)

    p_venv = sub.add_parser(
        "venv",
        help="Setup virtual environment",
    )
    _ = p_venv.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path(".venv"),
        help="Target directory for virtual environment (default: .venv)",
    )
    _ = p_venv.add_argument(
        "--aviutl2",
        type=str,
        default=None,
        help='AviUtl ExEdit2 version (e.g., "beta40a", default: latest)',
    )
    p_venv.set_defaults(func=_venv)

    p_run = sub.add_parser(
        "run",
        help="Run a command in the virtual environment",
    )
    _ = p_run.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path(".venv"),
        help="Target directory for virtual environment (default: .venv)",
    )
    _ = p_run.add_argument(
        "-b",
        "--build",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    _ = p_run.add_argument(
        "-c",
        "--config",
        type=str,
        default="Debug",
        help="Build configuration (e.g. Release, Debug)",
    )
    _ = p_run.add_argument(
        "-v",
        "--version",
        type=str,
        default=None,
        help='Override the project version (e.g., "1.0.0")',
    )
    _ = p_run.add_argument(
        "-d",
        "--define",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable (e.g., '-d DEBUG 1')",
    )
    _ = p_run.add_argument(
        "--aviutl2",
        type=str,
        default=None,
        help='AviUtl ExEdit2 version (e.g., "beta40a", default: latest)',
    )
    p_run.set_defaults(func=_run)

    return parser


@final
class Formatter(logging.Formatter):
    COLOR_CODES = {logging.WARNING: "\033[33m", logging.ERROR: "\033[31m"}
    RESET = "\033[0m"

    @override
    def format(self, record: logging.LogRecord) -> str:
        msg = super().format(record)
        color = self.COLOR_CODES.get(record.levelno)
        return f"{color}{msg}{self.RESET}" if color is not None else msg


def main() -> None:
    formatter = Formatter("[astra] %(levelname)-7s: %(message)s")

    logger = logging.getLogger()
    logger.handlers.clear()
    logger.setLevel(logging.INFO)

    handler = logging.StreamHandler(sys.stdout)
    handler.setLevel(logging.INFO)
    handler.addFilter(lambda record: record.levelno < logging.WARNING)
    handler.setFormatter(formatter)
    logger.addHandler(handler)

    handler = logging.StreamHandler(sys.stderr)
    handler.setLevel(logging.WARNING)
    handler.setFormatter(formatter)
    logger.addHandler(handler)

    parser = create_parser()
    args = cast(CommandArgs, cast(object, parser.parse_args()))
    args.func(args)


if __name__ == "__main__":
    main()
