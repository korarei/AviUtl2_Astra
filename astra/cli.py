import argparse
from pathlib import Path
from typing import Any

from astra.core import build, install, release, config


def _build(args: argparse.Namespace) -> None:
    build.build(args.source / args.config, args.version)


def _install(args: argparse.Namespace) -> None:
    install.install(args.source / args.config, args.destination)


def _release(args: argparse.Namespace) -> None:
    release.release(args.source / args.config)


def _init(args: argparse.Namespace) -> None:
    config.create_config(args.output, args.force)


def _schema(args: argparse.Namespace) -> None:
    if args.build:
        config.create_schema("build", args.output, args.force)

    if args.install:
        config.create_schema("install", args.output, args.force)

    if args.release:
        config.create_schema("release", args.output, args.force)


def add_common_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "-s", "--source",
        nargs='?',
        type=Path,
        default=Path.cwd(),
        help="Source directory containing the config file (default: current directory)"
    )
    parser.add_argument(
        "-c", "--config",
        nargs='?',
        type=str,
        default="astra.config.json",
        help="Name of the configuration file (default: astra.config.json)"
    )


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="astra",
        description="A build and deployment tool for AviUtl ExEdit2 scripts."
    )

    sub: Any = parser.add_subparsers(
        dest="command",
        required=True,
        help="The command to execute."
    )

    p_build = sub.add_parser(
        "build",
        help="Build the project from the config file."
    )
    add_common_args(p_build)
    p_build.add_argument(
        "-v", "--version",
        nargs='?',
        type=str,
        default=None,
        help="Override the project version."
    )
    p_build.set_defaults(func=_build)

    p_install = sub.add_parser(
        "install",
        help="Install the built artifacts to a destination."
    )
    add_common_args(p_install)
    p_install.add_argument(
        "-d", "--destination",
        nargs='?',
        type=Path,
        default=None,
        help="Override a destination directory for installation."
    )
    p_install.set_defaults(func=_install)

    p_release = sub.add_parser(
        "release",
        help="Package the project for release."
    )
    add_common_args(p_release)
    p_release.set_defaults(func=_release)

    p_init = sub.add_parser(
        "init",
        help="Initialize a new astra.config.json."
    )
    p_init.add_argument(
        "-o", "--output",
        nargs='?',
        type=Path,
        default=Path.cwd(),
        help="Output directory for the config file (default: current directory)"
    )
    p_init.add_argument(
        "-f", "--force",
        action="store_true",
        help="Overwrite an existing config file."
    )
    p_init.set_defaults(func=_init)

    p_schema = sub.add_parser(
        "schema",
        help="Generate JSON schema for configuration files."
    )
    p_schema.add_argument(
        "-o", "--output",
        nargs='?',
        type=Path,
        default=Path.cwd(),
        help="Output directory for the schema file (default: current directory)"
    )
    p_schema.add_argument(
        "-f", "--force",
        action="store_true",
        help="Overwrite an existing schema file."
    )
    p_schema.add_argument(
        "-b", "--build",
        action="store_true",
        help="Generate the build schema."
    )
    p_schema.add_argument(
        "-i", "--install",
        action="store_true",
        help="Generate the install schema."
    )
    p_schema.add_argument(
        "-r", "--release",
        action="store_true",
        help="Generate the release schema."
    )
    p_schema.set_defaults(func=_schema)

    return parser


def main() -> None:
    parser = create_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
