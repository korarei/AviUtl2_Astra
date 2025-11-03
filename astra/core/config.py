import json
from pathlib import Path
from typing import Any


def get_schema(required: list[str]) -> dict[str, Any]:
    schema: dict[str, Any] = {
        "type": "object",
        "required": required,
        "properties": {
            "project": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string"},
                    "version": {"type": "string"},
                    "author": {"type": "string"}
                },
                "additionalProperties": False
            },
            "build": {
                "type": "object",
                "required": ["directory", "scripts"],
                "properties": {
                    "clean": {"type": "boolean"},
                    "directory": {"type": "string"},
                    "scripts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["suffix", "source"],
                            "properties": {
                                "name": {"type": "string"},
                                "suffix": {"type": "string"},
                                "newline": {"type": "string"},
                                "source": {
                                    "type": "object",
                                    "properties": {
                                        "tag": {"type": "string"},
                                        "include_directories": {
                                            "type": "array",
                                            "items": {"type": "string"}
                                        },
                                        "variables": {
                                            "type": "object",
                                            "additionalProperties": {"type": "string"}
                                        }
                                    },
                                    "additionalProperties": False
                                }
                            },
                            "additionalProperties": False
                        }
                    },
                    "modules": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["path"],
                            "properties": {
                                "path": {"type": "string"}
                            },
                            "additionalProperties": False
                        }
                    }
                },
                "additionalProperties": False
            },
            "install": {
                "type": "object",
                "properties": {
                    "clean": {"type": "boolean"},
                    "directory": {"type": "string"}
                },
                "additionalProperties": False
            },
            "release": {
                "type": "object",
                "required": ["directory", "archive", "notes"],
                "properties": {
                    "clean": {"type": "boolean"},
                    "directory": {"type": "string"},
                    "archive": {
                        "type": "object",
                        "properties": {
                            "files": {
                                "type": "array",
                                "items": {"type": "string"}
                            },
                            "assets": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "required": ["directory"],
                                    "properties": {
                                        "directory": {"type": "string"},
                                        "url": {"type": "string"},
                                        "texts": {
                                            "type": "array",
                                            "items": {
                                                "type": "object",
                                                "required": ["file", "content"],
                                                "properties": {
                                                    "file": {"type": "string"},
                                                    "content": {"type": "string"}
                                                },
                                                "additionalProperties": False
                                            }
                                        }
                                    },
                                    "additionalProperties": False
                                }
                            }
                        },
                        "additionalProperties": False
                    },
                    "notes": {
                        "type": "object",
                        "required": ["source"],
                        "properties": {
                            "source": {"type": "string"}
                        },
                        "additionalProperties": False
                    }
                },
                "additionalProperties": False
            }
        },
        "additionalProperties": False
    }

    return schema


def create_schema(mode: str, dst: Path, force: bool) -> None:
    required: dict[str, list[str]] = {
        "build": ["project", "build"],
        "install": ["project", "build", "install"],
        "release": ["project", "build", "release"]
    }

    schema: dict[str, Any] = get_schema(required[mode])

    path: Path = dst / f"astra.{mode}_schema.json"

    if not force and path.exists():
        return

    with open(path, 'w', encoding="utf-8") as f:
        json.dump(schema, f, indent=4, ensure_ascii=False)


def create_config(dst: Path, force: bool) -> None:
    template: dict[str, Any] = {
        "project": {
            "name": "Project",
            "version": "v0.1.0",
            "author": "name"
        },
        "build": {
            "clean": False,
            "directory": "build",
            "scripts": [
                {
                    "name": "Effect",
                    "suffix": ".anm2",
                    "newline": "\r\n",
                    "source": {
                        "tag": ".in",
                        "include_directories": [
                            "includes"
                        ],
                        "variables": {
                            "LABEL": "アニメーション効果"
                        }
                    }
                }
            ],
            "modules": [
                {
                    "path": "../dll_src/build/Release/*.mod2"
                }
            ]
        },
        "install": {
            "clean": False,
            "directory": "C:/ProgramData/aviutl2/Script"
        },
        "release": {
            "clean": False,
            "directory": "release",
            "archive": {
                "files": [
                    "../README.md",
                    "../LICENSE"
                ],
                "assets": [
                    {
                        "directory": "assets",
                        "url": "https://",
                        "texts": [
                            {
                                "file": "credits.txt",
                                "content": "This is a sample asset."
                            }
                        ]
                    }
                ]
            },
            "notes": {
                "source": "../README.md"
            }
        }
    }

    path: Path = dst / "astra.config.json"

    if not force and path.exists():
        return

    with open(path, 'w', encoding="utf-8") as f:
        json.dump(template, f, indent=4, ensure_ascii=False)
