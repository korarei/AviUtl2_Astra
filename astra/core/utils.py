import re
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

    raise FileNotFoundError("astra.toml not found.")


def expand_variables(text: str, variables: dict[str, str]) -> str:
    def _replacer(match: re.Match[str]) -> str:
        key = match.group(1)
        if val := variables.get(key):
            return val
        else:
            logger.warning("Variable %s not found", key)
            return match.group(0)

    return _VAR_PATTERN.sub(_replacer, text)
