"""Console entry point that runs the Rust CLI implementation."""
import sys
from ._native import cli_entry


def main():
    try:
        cli_entry()
    except RuntimeError as error:
        print(error, file=sys.stderr)
        raise SystemExit(2) from None
