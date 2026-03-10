import re
import shutil
from io import BytesIO
from logging import getLogger
from pathlib import Path
from tempfile import mkdtemp
from typing import BinaryIO, Self, cast
from urllib.parse import urlparse
from urllib.request import urlopen
from zipfile import ZipFile, is_zipfile

from astra.core.config import (
    Release,
    ReleaseAsset,
    ReleaseContentContainer,
    ReleasePackage,
)

_CHANGELOG_HEADER_PATTERN = re.compile(
    r"^\s*\#+\s*change\s*logs?\b", re.IGNORECASE | re.MULTILINE
)
_CHANGELOG_SECTION_PATTERN = re.compile(
    r"""
    ^\s*(?:\#+|-+\s*\**)\s*(?:v|\[)?\d+(?:\.\d+)*\]?[^\n]*(?:\n|\Z)
    (?:(?!^\s*(?:\#+|-+\s*\**)\s*(?:v|\[)?\d)[^\n]*(?:\n|\Z))*
    """,
    re.MULTILINE | re.VERBOSE,
)

logger = getLogger(__name__)


class Releaser:
    _base: Path
    _dst: Path

    def __init__(self, dst: Path) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"Not a directory: {dst}")

        self._base = dst
        self._dst = self._base

    def __enter__(self) -> Self:
        self.mkdir()
        return self

    def __exit__(self, *_) -> None:
        self.cleanup()

    def copy_contents(self, cfg: Release) -> None:
        logger.info("Copying contents")

        contents = cfg.contents

        for category in contents.categories:
            for item in category:
                dst = self._dst / item.directory
                dst.mkdir(parents=True, exist_ok=True)
                for file in item.files:
                    self._copy_file(file, dst)

        for doc in contents.documents:
            dst = self._dst / doc.directory
            dst.mkdir(parents=True, exist_ok=True)
            for file in doc.files:
                self._copy_file(file, dst)

        for asset in contents.assets:
            dst = self._dst / asset.directory / asset.name
            dst.mkdir(parents=True, exist_ok=True)
            self._copy_asset(dst, asset)

        logger.info("Contents copied")

    def create_manifest(self, package: ReleasePackage) -> None:
        logger.info("Creating manifest")

        manifest = f"[ {package.name} ]\n\n"

        if package.version:
            manifest += f"Version: {package.version}\n"

        if package.author:
            manifest += f"Author: {package.author}\n"

        if package.license:
            manifest += f"License: {package.license}\n"

        if package.description:
            manifest += f"\n{package.description}\n"

        path = self._dst / "package.txt"
        _ = path.write_text(manifest, encoding="utf-8", newline="\r\n")

        logger.info("Manifest created: %s", path)

    def create_config(self, package: ReleasePackage) -> None:
        logger.info("Creating config")

        config = f"name={package.name}\nid={package.id}\n"

        if package.information:
            config += f"information={package.information}\n"

        path = self._dst / "package.ini"
        _ = path.write_text(config, encoding="utf-8", newline="\r\n")

        logger.info("Config created: %s", path)

    def create_archive(self, filename: str) -> None:
        logger.info("Creating archive")

        _ = shutil.make_archive(
            str(self._base / (filename + ".au2pkg")),
            "zip",
            root_dir=self._dst,
            base_dir=".",
        )

        logger.info("Archive created: %s.au2pkg.zip", filename)

    def mkdir(self) -> None:
        self._dst = Path(mkdtemp(dir=self._base))

    def cleanup(self) -> None:
        shutil.rmtree(self._dst, ignore_errors=True)

    def _copy_file(self, src: Path, dst: Path) -> None:
        if not src.is_file():
            logger.warning("File not found, skipping: %s", src)
            return

        _ = shutil.copy2(src, dst)

    def _copy_asset(self, dst: Path, asset: ReleaseAsset) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"Not a directory: {dst}")

        logger.info("Copying asset: %s", asset.name)

        for source in asset.sources:
            target = dst / source.directory
            target.mkdir(parents=True, exist_ok=True)

            for item in source.files:
                if isinstance(item, str):
                    self._download(item, target)
                else:
                    self._copy_file(item, target)

        if asset.documents:
            for doc in asset.documents:
                path = dst / doc.filename
                _ = path.write_text(doc.content, encoding="utf-8")

    def _download(self, url: str, dst: Path) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"Not a directory: {dst}")

        logger.info("Downloading: %s", url)

        try:
            req = cast(BinaryIO, urlopen(url))
            with req as response:
                data = BytesIO(response.read())
        except Exception:
            logger.warning("Download failed: %s", url)
            return

        _ = data.seek(0)
        if is_zipfile(data):
            _ = data.seek(0)
            with ZipFile(data) as zf:
                zf.extractall(dst)
            logger.info("Extracted zip to: %s", dst)
        else:
            name = Path(urlparse(url).path).name or "download"
            path = dst / name
            _ = data.seek(0)
            _ = path.write_bytes(data.read())
            logger.info("Saved file: %s", path)


def create_release_notes(dst: Path, contents: ReleaseContentContainer) -> None:
    if not dst.is_dir():
        raise NotADirectoryError(f"Not a directory: {dst}")

    logger.info("Creating release notes")

    changelog: Path | None = None
    readme: Path | None = None

    for doc in contents.documents:
        for file in doc.files:
            stem = file.stem.upper()
            if "CHANGELOG" in stem:
                changelog = file
                break
            elif "README" in stem:
                readme = file
        else:
            continue

        break

    target = changelog or readme
    if not target:
        logger.warning("Changelog or readme not found")
        return

    if not target.is_file():
        logger.warning("Target is not a file: %s", target)
        return

    try:
        text = target.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as e:
        logger.warning(
            "Failed to read %s (%s): %s",
            "changelog" if target == changelog else "readme",
            e.__class__.__name__,
            target,
        )
        return

    if target == readme:
        match = _CHANGELOG_HEADER_PATTERN.search(text)
        if not match:
            raise ValueError("Missing required section: '## Change Log'")
        text = text[match.end() :]

    match = _CHANGELOG_SECTION_PATTERN.search(text)
    if match:
        section = match.group(0).strip()
        changes = re.sub(r"^\s*-", "-", section, flags=re.MULTILINE).split(
            "\n", 1
        )[1]
        content = f"## What's Changed\n{changes}"
    else:
        return

    path = dst / "release_notes.md"
    _ = path.write_text(content, encoding="utf-8", newline="\n")

    logger.info("Release notes created: %s", path)


def release(dst: Path, cfg: Release) -> None:
    logger.info("Release started: Destination=%s", dst)

    dst.mkdir(parents=True, exist_ok=True)
    dst = dst.resolve()

    with Releaser(dst) as releaser:
        releaser.copy_contents(cfg)
        releaser.create_manifest(cfg.package)
        releaser.create_config(cfg.package)
        releaser.create_archive(cfg.package.filename)

    create_release_notes(dst, cfg.contents)

    logger.info("Release completed")
