import re
import sys
from logging import getLogger
from pathlib import Path

_VAR_PATTERN = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")

logger = getLogger(__name__)


def find_config() -> Path:
    candidates = [
        Path("astra.toml"),
        Path("astra.tml"),
        Path(".config/astra.toml"),
        Path(".config/astra.tml"),
        Path(".astra/astra.toml"),
        Path(".astra/astra.tml"),
    ]

    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()

    logger.error("astra.toml not found.")
    sys.exit(1)


def expand_variables(text: str, variables: dict[str, str]) -> str:
    def _replacer(match: re.Match[str]) -> str:
        key = match.group(1)
        return variables.get(key, match.group(0))

    return _VAR_PATTERN.sub(_replacer, text)
