import re
import shutil
from logging import getLogger
from pathlib import Path
from tempfile import mkdtemp
from typing import Self

from astra._internal.config import (
    Release,
    ReleaseAsset,
    ReleaseDocument,
)
from astra._internal.utils import download


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
    _dst: Path
    _cfg: Release

    def __init__(self, dst: Path, cfg: Release) -> None:
        if dst.is_file():
            raise NotADirectoryError(f"'{dst}' is not a directory")

        dst = dst.resolve()
        dst.mkdir(parents=True, exist_ok=True)

        self._dst = dst
        self._cfg = cfg

    def __enter__(self) -> Self:
        self.mkdir()
        return self

    def __exit__(self, *_) -> None:
        self.cleanup()

    def copy_contents(self) -> None:
        contents = self._cfg.contents

        for extension in contents.extensions:
            dst = self._dst / extension.directory
            dst.mkdir(parents=True, exist_ok=True)

            files = extension.files

            logger.info(f"Copying {len(files)} extension(s) to '{dst}'")

            for file in files:
                self._copy_file(file, dst)

        for doc in contents.documents:
            dst = self._dst / doc.directory
            dst.mkdir(parents=True, exist_ok=True)

            files = doc.files

            logger.info(f"Copying {len(files)} document(s) to '{dst}'")

            for file in files:
                self._copy_file(file, dst)

        for asset in contents.assets:
            dst = self._dst / asset.directory / asset.name
            dst.mkdir(parents=True, exist_ok=True)

            logger.info(f"Making asset to '{dst}'")

            self._copy_asset(dst, asset)

    def write_manifest(self) -> None:
        logger.info("Writing 'package.txt'")

        package = self._cfg.package

        manifest = f"[ {package.name} ]\n\n"

        if package.summary is not None:
            manifest += f"{package.summary}\n\n"

        if package.version is not None:
            manifest += f"Version: {package.version}\n"

        if package.license is not None:
            manifest += f"License: {package.license}\n"

        if package.author is not None:
            manifest += f"Author: {package.author}\n"

        if package.website is not None:
            manifest += f"Website: {package.website}\n"

        if package.report_issue is not None:
            manifest += f"Report Issue: {package.report_issue}\n"

        if package.description is not None:
            manifest += f"\n{package.description}\n"

        path = self._dst / "package.txt"
        _ = path.write_text(manifest, encoding="utf-8", newline="\r\n")

    def write_config(self) -> None:
        logger.info("Writing 'package.ini'")

        package = self._cfg.package

        config = (
            f"[package]\nid={package.id}\nname={package.name}\n"
            f"uninstallSubFolderFile={int(package.uninstall_subdirectory_files)}\n"
        )

        if package.information is not None:
            config += f"information={package.information}\n"

        path = self._dst / "package.ini"
        _ = path.write_text(config, encoding="utf-8", newline="\r\n")

    def make_archive(self) -> None:
        path = self._dst.parent / self._cfg.package.filename

        logger.info(f"Making archive to '{path}'")

        _ = shutil.make_archive(
            str(path.with_suffix("")),
            path.suffix[1:],
            root_dir=self._dst,
            base_dir=".",
        )

    def mkdir(self) -> None:
        self._dst = Path(mkdtemp(dir=self._dst))

    def cleanup(self) -> None:
        shutil.rmtree(self._dst, ignore_errors=True)

    def _copy_file(self, src: Path, dst: Path) -> None:
        if not src.is_file():
            logger.warning(f"'{src}' is not found")
            return

        _ = shutil.copy2(src, dst)

    def _copy_asset(self, dst: Path, asset: ReleaseAsset) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"'{dst}' is not a directory")

        for source in asset.sources:
            target = dst / source.directory
            target.mkdir(parents=True, exist_ok=True)

            for item in source.files:
                if isinstance(item, str):
                    try:
                        download(item, target)
                    except Exception:
                        logger.warning(f"Failed to download asset: '{item}'")
                else:
                    self._copy_file(item, target)

        if asset.documents:
            for doc in asset.documents:
                path = dst / doc.filename
                _ = path.write_text(doc.content, encoding="utf-8")


def create_release_notes(dst: Path, documents: list[ReleaseDocument]) -> None:
    if dst.is_file():
        raise NotADirectoryError(f"'{dst}' is not a directory")

    dst = dst.resolve()
    dst.mkdir(parents=True, exist_ok=True)
    path = dst / "release_notes.md"

    logger.info(f"Writing release notes to '{path}'")

    changelog: Path | None = None
    readme: Path | None = None

    for doc in documents:
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
    if target in (None, ""):
        logger.warning("CHANGELOG or README is not found")
        return

    if not target.is_file():
        raise FileNotFoundError(f"'{target}' is not found")

    text = target.read_text(encoding="utf-8")

    if target == readme:
        match = _CHANGELOG_HEADER_PATTERN.search(text)
        if not match:
            raise ValueError("Missing required section: '## Change Log'")

        text = text[match.end() :]

    match = _CHANGELOG_SECTION_PATTERN.search(text)
    if match is None:
        return

    section = match.group(0).strip()
    changes = re.sub(r"^[^\S\n]*-", "-", section, flags=re.MULTILINE).split("\n", 1)[1]
    content = f"## What's Changed\n{changes}\n"

    _ = path.write_text(content, encoding="utf-8", newline="\n")


def release(dst: Path, cfg: Release) -> None:
    with Releaser(dst, cfg) as releaser:
        releaser.copy_contents()
        releaser.write_manifest()
        releaser.write_config()
        releaser.make_archive()

    documents = cfg.contents.documents
    if not documents:
        return

    create_release_notes(dst, documents)
