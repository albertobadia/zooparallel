#!/bin/bash
cargo fmt
uv run ruff check . --fix
uv run ruff format .
