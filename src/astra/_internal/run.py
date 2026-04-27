import subprocess
from logging import getLogger
from pathlib import Path


logger = getLogger(__name__)


def run(target: Path) -> None:
    logger.info(f"Launching AviUtl ExEdit2 at '{target}'")
    _ = subprocess.Popen([str(target / "aviutl2.exe")])
