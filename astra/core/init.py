import logging
from pathlib import Path

logger = logging.getLogger(__name__)

_DEFAULT_CONFIG = """[project]
name = "Project"
version = "0.1.0"
author = "Author"
requires-aviutl2 = "2003600"

[build]
[[build.plugins]]
id = "core"
variables = { SOURCE = "./modules" }

[build.plugins.release]
commands = [
    "cmake -S ${SOURCE} -B ${BUILD_DIRECTORY}",
    "cmake --build ${BUILD_DIRECTORY} --config Release",
]
artifacts = ["${BUILD_DIRECTORY}/Release/*.mod2"]

[[build.scripts]]
id = "effect"
prefix = "@"
suffix = ".anm2"
variables = { SOURCE = "./scripts" }
include_directories = ["${SOURCE}/shaders"]
sources = [
    { file = "${SOURCE}/*.lua", LABEL = "Effect" },
]

[release]
[release.package]
information = "${PROJECT_NAME} v${PROJECT_VERSION} by ${PROJECT_AUTHOR}"
license = "MIT"
description = "Example plugin package"

[[release.contents.extensions]]
directory = "Script/${PROJECT_NAME}"
files = ["script:effect", "plugin:core"]

[[release.contents.documents]]
directory = "Script/${PROJECT_NAME}"
files = ["./*.md", "./LICENSE"]

"""


def init(dst: Path) -> None:
    if dst.exists() and not dst.is_dir():
        logger.error("Destination is not a directory: %s", dst)
        return

    path = dst / "astra.toml"
    if path.exists():
        logger.error("Config file already exists: %s", path)
        return

    dst.mkdir(parents=True, exist_ok=True)

    _ = path.write_text(_DEFAULT_CONFIG, encoding="utf-8")
    logger.info("Created config file: %s", path)
