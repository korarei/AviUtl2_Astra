import importlib.metadata as metadata
import logging
import os
import shutil
import sys
import tempfile
from argparse import ArgumentParser
from pathlib import Path
from typing import Callable, Protocol, cast

from filelock import FileLock

from astra._internal import build, config, init, install, release, schema, venv
from astra._internal.utils import find_config


logger = logging.getLogger(__name__)

try:
    __version__ = metadata.version("astra")
except metadata.PackageNotFoundError:
    __version__ = "unknown"


class InitArgs(Protocol):
    command: str
    target: Path
    func: Callable[["InitArgs"], None]


class BuildArgs(Protocol):
    command: str
    build: Path
    config: str
    version: str | None
    define: list[list[str]]
    func: Callable[["BuildArgs"], None]


class ReleaseArgs(Protocol):
    command: str
    target: Path
    version: str | None
    define: list[list[str]]
    func: Callable[["ReleaseArgs"], None]


class InstallArgs(Protocol):
    command: str
    target: Path | None
    build: Path
    editable: bool
    define: list[list[str]]
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


class VenvArgs(Protocol):
    command: str
    target: Path
    aviutl2: str | None
    func: Callable[["VenvArgs"], None]


def _init(args: InitArgs) -> None:
    try:
        init.init(args.target)
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to initialize: {e}")
        sys.exit(1)


def _build(args: BuildArgs) -> None:
    try:
        dst = args.build
        dst.mkdir(parents=True, exist_ok=True)

        env = {k: v for k, v in args.define}
        cfg = config.Config(find_config(), args.version, env).load_build()

        with FileLock(dst / ".astra-lock"):
            artifact = build.build(dst, cfg, args.config)
            config.Cache(dst / "astra.json").save(artifact)
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to build: {e}")
        sys.exit(1)


def _release(args: ReleaseArgs) -> None:
    try:
        dst = args.target
        dst.mkdir(parents=True, exist_ok=True)

        with FileLock(dst / ".astra-lock"):
            for ext in "*.zip", "*.md", "tmp*", "*.au2pkg":
                for file in dst.glob(ext):
                    if file.is_file() or file.is_symlink():
                        file.unlink()
                    else:
                        shutil.rmtree(file, ignore_errors=True)

            env = {k: v for k, v in args.define}
            cfg = config.Config(find_config(), args.version, env)

            with tempfile.TemporaryDirectory(dir=dst) as tmp:
                artifact = build.build(Path(tmp), cfg.load_build(), "release")
                release.release(dst, cfg.load_release(artifact))
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to release: {e}")
        sys.exit(1)


def _install(args: InstallArgs) -> None:
    if sys.platform == "win32":
        default = Path(os.getenv("ProgramData", "C:\\ProgramData")) / "aviutl2"
    else:
        default = None

    if args.target is None:
        venv = Path(".venv")
        if venv.exists():
            target = venv / "aviutl2/data"
        elif default is not None:
            target = default
        else:
            logger.error("Install target not specified")
            sys.exit(1)
    else:
        target = args.target

    target = target.resolve()
    if not target.is_dir():
        logger.error(f"'{target}' is not a directory")
        sys.exit(1)

    if not args.build.is_dir():
        logger.error(f"Build directory not found: {args.build.resolve()}")
        sys.exit(1)

    name = target.name.lower()
    if name not in ("aviutl2", "data"):
        logger.error(f"Install target not valid: {target}")
        sys.exit(1)
    elif name == "aviutl2" and default is not None and target != default:
        logger.error(f"Install target not valid: {target}")
        sys.exit(1)
    elif name == "data" and not (target.parent / "aviutl2.exe").is_file():
        logger.error(f"Install target not valid: {target}")
        sys.exit(1)

    try:
        cache = config.Cache(args.build / "astra.json")
        artifact = cache.load(config.Artifact)
        env = {k: v for k, v in args.define}
        cfg = config.Config(find_config(), defines=env).load_install(artifact)
        cache.save(install.install(target, cfg, args.editable))
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to install: {e}")
        sys.exit(1)


def _uninstall(args: UninstallArgs) -> None:
    if not args.build.is_dir():
        logger.error(f"Build directory not found: {args.build.resolve()}")
        sys.exit(1)

    try:
        cache = config.Cache(args.build / "astra.json")
        install.uninstall(cache.load(config.Extension))
        cache.save(config.Extension())
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to uninstall: {e}")
        sys.exit(1)


def _clean(args: CleanArgs) -> None:
    venv = Path(".venv")
    if venv.exists():
        shutil.rmtree(venv)

    if not args.build.exists():
        logger.info(f"Already clean: {args.build.resolve()}")
        return

    try:
        logger.info(f"Cleaning: {args.build.resolve()}")

        if args.build.is_dir():
            if not (args.build / "astra.json").is_file():
                logger.error(f"Not a target directory: {args.build}")
                sys.exit(1)

            cache = config.Cache(args.build / "astra.json")
            install.uninstall(cache.load(config.Extension))

            shutil.rmtree(args.build)
        else:
            args.build.unlink()
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to clean: {e}")
        sys.exit(1)


def _schema(args: SchemaArgs) -> None:
    try:
        schema.schema(args.target)
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to generate schema: {e}")
        sys.exit(1)


def _venv(args: VenvArgs) -> None:
    try:
        venv.venv(args.target, args.aviutl2)
    except Exception as e:
        logger.error(f"[{type(e).__name__}] Failed to create virtual environment: {e}")
        sys.exit(1)


def create_parser() -> ArgumentParser:
    parser = ArgumentParser(
        prog="astra",
        description="A build and deployment tool for AviUtl2 scripts.",
    )

    _ = parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
        help="Show the version of the program.",
    )

    sub = parser.add_subparsers(
        dest="command",
        required=True,
        help="The command to execute.",
    )

    p_init = sub.add_parser(
        "init",
        help="Initialize a new project with a default astra.toml.",
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
        help="Build the project from the config file.",
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
        help='Override the project version. (e.g., "1.0.0")',
    )
    _ = p_build.add_argument(
        "--define",
        "-d",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable. (e.g., '-d DEBUG 1')",
    )
    p_build.set_defaults(func=_build)

    p_release = sub.add_parser(
        "release",
        help="Package the project for release.",
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
        help='Override the project version. (e.g., "1.0.0")',
    )
    _ = p_release.add_argument(
        "--define",
        "-d",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable. (e.g., '-d DEBUG 1')",
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
        default=None,
        help="Target directory for installation (default: .venv/aviutl2/data "
        + "if .venv exists, otherwise %%ProgramData%%/aviutl2)",
    )
    _ = p_install.add_argument(
        "--build",
        "-b",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    _ = p_install.add_argument(
        "--editable",
        "-e",
        action="store_true",
        help="Install the built artifacts in editable mode.",
    )
    _ = p_install.add_argument(
        "--define",
        "-d",
        metavar=("KEY", "VALUE"),
        action="append",
        nargs=2,
        default=[],
        help="Define a variable. (e.g., '-d DEBUG 1')",
    )
    p_install.set_defaults(func=_install)

    p_uninstall = sub.add_parser(
        "uninstall",
        help="Uninstall the built artifacts and modules from a target dir.",
    )
    _ = p_uninstall.add_argument(
        "--build",
        "-b",
        type=Path,
        default=Path("build"),
        help="Build directory (default: build)",
    )
    p_uninstall.set_defaults(func=_uninstall)

    p_clean = sub.add_parser(
        "clean",
        help="Clean the build directory.",
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
        help="Output the JSON schema for astra.toml.",
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
        help="Setup virtual environment.",
    )
    _ = p_venv.add_argument(
        "target",
        type=Path,
        nargs="?",
        default=Path("."),
        help="Target directory for virtual environment (default: .)",
    )
    _ = p_venv.add_argument(
        "--aviutl2",
        type=str,
        default=None,
        help='AviUtl2 version (e.g., "beta40a"). [default: latest]',
    )
    p_venv.set_defaults(func=_venv)

    return parser


class CommandArgs(Protocol):
    func: Callable[["CommandArgs"], None]


def main() -> None:
    logger = logging.getLogger()
    logger.handlers.clear()
    logger.setLevel(logging.INFO)

    handler = logging.StreamHandler(sys.stdout)
    handler.setLevel(logging.INFO)
    handler.addFilter(lambda record: record.levelno < logging.WARNING)
    handler.setFormatter(logging.Formatter("[astra] %(levelname)-7s: %(message)s"))
    logger.addHandler(handler)

    handler = logging.StreamHandler(sys.stderr)
    handler.setLevel(logging.WARNING)
    handler.setFormatter(logging.Formatter("[astra] %(levelname)-7s: %(message)s"))
    logger.addHandler(handler)

    parser = create_parser()
    args = cast(CommandArgs, cast(object, parser.parse_args()))
    args.func(args)


if __name__ == "__main__":
    main()
