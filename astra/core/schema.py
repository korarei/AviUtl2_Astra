import json
from logging import getLogger
from pathlib import Path
from typing import ClassVar


logger = getLogger(__name__)


class Schema:
    data: ClassVar[dict[str, object]] = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Astra Config File",
        "description": "JSON schema for astra.toml configuration file.",
        "type": "object",
        "properties": {
            "astra": {
                "type": "object",
                "properties": {
                    "requires-astra": {
                        "type": "string",
                        "description": "Required Astra version",
                    },
                },
                "additionalProperties": False,
            },
            "project": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Project name",
                    },
                    "version": {
                        "type": "string",
                        "description": "Project version",
                    },
                    "author": {
                        "type": "string",
                        "description": "Project author",
                    },
                    "requires-aviutl2": {
                        "type": "string",
                        "description": "Required AviUtl2 version",
                    },
                    "variables": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                    },
                },
                "required": ["name"],
                "additionalProperties": False,
            },
            "build": {
                "type": "object",
                "properties": {
                    "plugins": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean"},
                                "id": {
                                    "type": "string",
                                    "description": "Plugin ID",
                                },
                                "variables": {
                                    "type": "object",
                                    "additionalProperties": {"type": "string"},
                                },
                                "release": {
                                    "type": "object",
                                    "properties": {
                                        "commands": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                            "default": [],
                                        },
                                        "artifacts": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                            "default": [],
                                        },
                                    },
                                    "additionalProperties": False,
                                },
                                "debug": {
                                    "type": "object",
                                    "properties": {
                                        "commands": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                            "default": [],
                                        },
                                        "artifacts": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                            "default": [],
                                        },
                                    },
                                    "additionalProperties": False,
                                },
                            },
                            "required": ["id", "release"],
                            "additionalProperties": False,
                        },
                    },
                    "scripts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "enabled": {"type": "boolean"},
                                "id": {
                                    "type": "string",
                                    "description": "Script ID",
                                },
                                "name": {"type": "string"},
                                "prefix": {"type": "string"},
                                "suffix": {"type": "string"},
                                "newline": {"type": "string"},
                                "source-encoding": {"type": "string"},
                                "target-encoding": {"type": "string"},
                                "include_directories": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                },
                                "variables": {
                                    "type": "object",
                                    "additionalProperties": {"type": "string"},
                                },
                                "sources": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "file": {
                                                "type": "string",
                                                "description": "File path",
                                            }
                                        },
                                        "required": ["file"],
                                        "additionalProperties": {"type": "string"},
                                    },
                                },
                                "artifacts": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                },
                            },
                            "required": ["id"],
                            "additionalProperties": False,
                        },
                    },
                },
                "additionalProperties": False,
            },
            "release": {
                "type": "object",
                "properties": {
                    "package": {
                        "type": "object",
                        "properties": {
                            "filename": {"type": "string"},
                            "name": {"type": "string"},
                            "id": {"type": "string"},
                            "information": {"type": "string"},
                            "version": {"type": "string"},
                            "author": {"type": "string"},
                            "license": {"type": "string"},
                            "summary": {"type": "string"},
                            "description": {"type": "string"},
                            "website": {"type": "string"},
                            "report-issue": {"type": "string"},
                        },
                        "additionalProperties": False,
                    },
                    "contents": {
                        "type": "object",
                        "properties": {
                            "extensions": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "directory": {"type": "string"},
                                        "files": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                        },
                                    },
                                    "required": ["directory", "files"],
                                    "additionalProperties": False,
                                },
                            },
                            "documents": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "directory": {"type": "string"},
                                        "files": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                        },
                                    },
                                    "required": ["directory", "files"],
                                    "additionalProperties": False,
                                },
                            },
                            "assets": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "enabled": {"type": "boolean"},
                                        "name": {"type": "string"},
                                        "directory": {"type": "string"},
                                        "sources": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "directory": {"type": "string"},
                                                    "files": {
                                                        "type": "array",
                                                        "items": {"type": "string"},
                                                    },
                                                },
                                                "additionalProperties": False,
                                            },
                                        },
                                        "documents": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "filename": {"type": "string"},
                                                    "content": {"type": "string"},
                                                },
                                                "required": ["filename"],
                                                "additionalProperties": False,
                                            },
                                        },
                                    },
                                    "required": ["name", "directory"],
                                    "additionalProperties": False,
                                },
                            },
                        },
                        "additionalProperties": False,
                    },
                },
                "required": ["contents"],
                "additionalProperties": False,
            },
        },
        "required": ["project"],
        "additionalProperties": False,
    }

    def dumps(self, indent: int = 4) -> str:
        return json.dumps(self.data, ensure_ascii=False, indent=indent)

    def save(self, dst: Path, indent: int = 4) -> None:
        if not dst.is_dir():
            raise NotADirectoryError(f"Destination is not a directory: {dst}")

        dst.mkdir(parents=True, exist_ok=True)
        path = dst / "astra.schema.json"
        with path.open("w", encoding="utf-8") as f:
            json.dump(self.data, f, ensure_ascii=False, indent=indent)


def schema(dst: Path | None = None, indent: int = 4) -> None:
    s = Schema()
    if dst:
        s.save(dst, indent)
        logger.info("Saved schema to %s", dst)
    else:
        print(s.dumps(indent))
