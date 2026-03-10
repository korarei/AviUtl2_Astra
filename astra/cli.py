import os
import shutil
import sys
from argparse import ArgumentParser
from logging import getLogger
from pathlib import Path
from tempfile import mkdtemp
from typing import Callable, Protocol, cast

from astra.core import build, config, init, install, release, schema
from astra.core.utils import find_config

_DEFAULT_INSTALL_TARGET = (
    Path(os.getenv("ProgramData", "C:\\ProgramData")) / "aviutl2"
    if sys.platform == "win32"
    else None
)

logger = getLogger(__name__)


class InitArgs(Protocol):
    command: str
    target: Path
    func: Callable[["InitArgs"], None]


class BuildArgs(Protocol):
    command: str
    build: Path
    config: str | None
    version: str | None
    func: Callable[["BuildArgs"], None]


class ReleaseArgs(Protocol):
    command: str
    target: Path
    version: str | None
    func: Callable[["ReleaseArgs"], None]


class InstallArgs(Protocol):
    command: str
    target: Path | None
    build: Path
    editable: bool
    func: Callable[["InstallArgs"], None]


class UninstallArgs(Protocol):
    command: str
    build: Path
    func: Callable[["UninstallArgs"], None]


class CleanArgs(Protocol):
    command: str
    build: Path
    func: Callable[["CleanArgs"], None]


class SchemaArgs(Protocol):
    command: str
    target: Path | None
    func: Callable[["SchemaArgs"], None]


def _init(args: InitArgs) -> None:
    init.init(args.target)


def _build(args: BuildArgs) -> None:
    cfg = config.Config(find_config(), args.version).load_build()

    _ = build.build(args.build, cfg, args.config or "debug")


def _release(args: ReleaseArgs) -> None:
    if args.target.exists():
        if args.target.is_dir():
            shutil.rmtree(args.target)
        else:
            args.target.unlink()

    args.target.mkdir(parents=True, exist_ok=True)
    cfg = config.Config(find_config(), args.version)
    tmp = Path(mkdtemp(dir=args.target))

    artifact = build.build(tmp, cfg.load_build(), "release")
    release.release(args.target, cfg.load_release(artifact))

    shutil.rmtree(tmp, ignore_errors=True)


def _install(args: InstallArgs) -> None:
    if not args.target:
        logger.error("Install target not specified.")
        sys.exit(1)

    name = args.target.name.lower()
    if name not in ("aviutl2", "data"):
        logger.error("Install target not valid: %s", args.target)
        sys.exit(1)
    elif name == "aviutl2" and args.target != _DEFAULT_INSTALL_TARGET:
        logger.error("Install target not valid: %s", args.target)
        sys.exit(1)
    elif name == "data" and not (args.target.parent / "aviutl2.exe").is_file():
        logger.error("Install target not valid: %s", args.target)
        sys.exit(1)

    if not args.build.is_dir():
        logger.error("Build directory not found: %s", args.build)
        sys.exit(1)

    cache = config.Cache(args.build / "astra.json")
    artifact = cache.load_artifacts()
    if artifact is None:
        logger.error("No artifacts found. Please run 'astra build' first.")
        sys.exit(1)

    cfg = config.Config(find_config()).load_install(artifact)

    installations = install.install(args.target, cfg, args.editable)

    cache.save_installations(installations)


def _uninstall(args: UninstallArgs) -> None:
    if not args.build.is_dir():
        logger.error("Build directory not found: %s", args.build)
        sys.exit(1)

    cache = config.Cache(args.build / "astra.json")
    installations = cache.load_installations()
    if installations is None:
        logger.error("No installation records found.")
        sys.exit(1)

    install.uninstall(installations)

    cache.save_installations([])


def _clean(args: CleanArgs) -> None:
    if not args.build.exists():
        logger.info("Already clean: %s", args.build)
        return

    if args.build.is_dir():
        if not (args.build / "astra.json").is_file():
            logger.error("Not a target directory: %s", args.build)
            sys.exit(1)

        shutil.rmtree(args.build)
    else:
        args.build.unlink()

    logger.info("Cleaned: %s", args.build)


def _schema(args: SchemaArgs) -> None:
    schema.schema(args.target)


def create_parser() -> ArgumentParser:
    parser = ArgumentParser(
        prog="astra",
        description="A build and deployment tool for AviUtl2 scripts.",
    )

    sub = parser.add_subparsers(
        dest="command", required=True, help="The command to execute."
    )

    p_init = sub.add_parser(
        "init", help="Initialize a new project with a default astra.toml."
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
        "build", help="Build the project from the config file."
    )
    _ = p_build.add_argument(
        "build",
        type=Path,
        nargs="?",
        default=Path("build"),
        help="Build destination directory (default: build)",
    )
    _ = p_build.add_argument(
        "--config",
        "-c",
        type=str,
        default="Debug",
        help="Build configuration (e.g. Release, Debug).",
    )
    _ = p_build.add_argument(
        "--version",
        "-v",
        type=str,
        default=None,
        help='Override the project version. (e.g. "1.0.0")',
    )
    p_build.set_defaults(func=_build)

    p_release = sub.add_parser(
        "release", help="Package the project for release."
    )
    _ = p_release.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path("release"),
        help="Build directory (default: release)",
    )
    _ = p_release.add_argument(
        "--version",
        "-v",
        type=str,
        default=None,
        help='Override the project version. (e.g. "1.0.0")',
    )
    p_release.set_defaults(func=_release)

    p_install = sub.add_parser(
        "install",
        help="Install the built artifacts and modules to a target dir.",
    )
    _ = p_install.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=_DEFAULT_INSTALL_TARGET,
        help="Target directory for installation.",
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
        help="Install the built artifacts in editable mode.",
    )
    p_install.set_defaults(func=_install)

    p_uninstall = sub.add_parser(
        "uninstall",
        help="Uninstall the built artifacts and modules from a target dir.",
    )
    _ = p_uninstall.add_argument(
        "-b",
        "--build",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    p_uninstall.set_defaults(func=_uninstall)

    p_clean = sub.add_parser("clean", help="Clean the build directory.")
    _ = p_clean.add_argument(
        "build",
        type=Path,
        nargs="?",
        default=Path("build"),
        help="Build directory to clean (default: build)",
    )
    p_clean.set_defaults(func=_clean)

    p_schema = sub.add_parser(
        "schema", help="Output the JSON schema for astra.toml."
    )
    _ = p_schema.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=None,
        help="Output directory path (default: stdout)",
    )
    p_schema.set_defaults(func=_schema)

    return parser


class CommandArgs(Protocol):
    func: Callable[["CommandArgs"], None]


def main() -> None:
    parser = create_parser()
    args = cast(CommandArgs, cast(object, parser.parse_args()))
    args.func(args)


if __name__ == "__main__":
    main()
