"""Validate and synchronize the PiTools Flux activation bundle."""

from __future__ import annotations

import argparse
import re
import shutil
import sys
from collections.abc import Iterable
from pathlib import Path

SOURCE_BUNDLE_NAME = "pitools"
AGGREGATE_NAME = "kustomization.yaml"
REQUIRED_FILES = frozenset(
    {"kustomization.yaml", "app.yaml", "route.yaml", "networkpolicy.yaml"}
)
CORE_IMAGE_PREFIX = "ghcr.io/titanicar-us/pitools@sha256:"
RUNNER_IMAGE_PREFIX = "ghcr.io/titanicar-us/pitools-runner@sha256:"
IMAGE_LINE_RE = re.compile(r"^\s*image:\s*(\S+)\s*$", re.MULTILINE)
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
RESOURCE_LINE_RE = re.compile(r"^(\s*)-\s*([^\s#]+)\s*(?:#.*)?$")
KIND_RE = re.compile(r"^\s*kind:\s*([^\s#]+)\s*$", re.MULTILINE)


class PromotionError(ValueError):
    """Raised when a source or target promotion contract is invalid."""


def _read_text(path: Path) -> str:
    """Read a UTF-8 file or raise a promotion-specific error."""

    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise PromotionError(f"unable to read {path}: {error}") from error


def _require_directory(path: Path, *, name: str) -> Path:
    """Return a real directory and reject symlink or missing path inputs."""

    if path.is_symlink():
        raise PromotionError(f"{name} must not be a symlink: {path}")
    if not path.is_dir():
        raise PromotionError(f"{name} is not a directory: {path}")
    return path.resolve()


def _require_file(path: Path, *, description: str) -> None:
    """Require a regular, non-symlink file."""

    if path.is_symlink() or not path.is_file():
        raise PromotionError(f"{description} is not a regular file: {path}")


def _resource_entries(text: str, *, description: str) -> list[tuple[str, str]]:
    """Extract a Kustomization resources list with its indentation metadata."""

    lines = text.splitlines()
    resources_index = -1
    resources_indent = 0
    for index, line in enumerate(lines):
        if re.fullmatch(r"(\s*)resources:\s*", line):
            resources_index = index
            resources_indent = len(line) - len(line.lstrip())
            break
    if resources_index < 0:
        raise PromotionError(f"{description} has no resources list")

    entries: list[tuple[str, str]] = []
    for line in lines[resources_index + 1 :]:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())
        if indent < resources_indent:
            break
        match = RESOURCE_LINE_RE.fullmatch(line)
        if match is None:
            raise PromotionError(f"{description} has an invalid resources entry: {line}")
        entries.append((match.group(1), match.group(2)))
    if not entries:
        raise PromotionError(f"{description} has an empty resources list")
    return entries


def _validate_resource_path(resource: str, *, description: str) -> None:
    """Reject traversal and unsupported resource path syntax."""

    if (
        not re.fullmatch(r"[A-Za-z0-9._/-]+", resource)
        or resource.startswith(("/", "../"))
        or "/../" in resource
        or resource.endswith("/..")
        or "//" in resource
    ):
        raise PromotionError(f"{description} contains an unsafe resource path: {resource}")


def _validate_kustomization(path: Path, *, expected_resources: set[str]) -> None:
    """Validate a Kustomization and require exactly the expected resources."""

    text = _read_text(path)
    if "apiVersion: kustomize.config.k8s.io/v1beta1" not in text:
        raise PromotionError(f"{path} has an unexpected Kustomization apiVersion")
    if not re.search(r"^\s*kind:\s*Kustomization\s*$", text, re.MULTILINE):
        raise PromotionError(f"{path} is not a Kustomization")
    entries = _resource_entries(text, description=str(path))
    resources = {resource for _, resource in entries}
    if resources != expected_resources or len(entries) != len(resources):
        raise PromotionError(
            f"{path} resources must be exactly {sorted(expected_resources)}"
        )
    for resource in resources:
        _validate_resource_path(resource, description=str(path))


def _validate_manifest(path: Path) -> str:
    """Validate one Kubernetes manifest file and return its content."""

    text = _read_text(path)
    if not re.search(r"^\s*apiVersion:\s*\S+\s*$", text, re.MULTILINE):
        raise PromotionError(f"{path} has no apiVersion")
    if not re.search(r"^\s*kind:\s*\S+\s*$", text, re.MULTILINE):
        raise PromotionError(f"{path} has no kind")
    if not re.search(r"^\s*metadata:\s*$", text, re.MULTILINE):
        raise PromotionError(f"{path} has no metadata")
    if re.search(r"^\s*kind:\s*Secret\s*$", text, re.MULTILINE):
        raise PromotionError(f"Secret manifests are not permitted: {path}")
    if re.search(r"^\s*stringData:\s*$", text, re.MULTILINE):
        raise PromotionError(f"stringData is not permitted: {path}")
    if "-----BEGIN" in text or re.search(r"(?i)private[ _-]+key\s*:\s*[^$<{\s]", text):
        raise PromotionError(f"private-key material is not permitted: {path}")
    if re.search(r"(?i)(gh[pous]_|sk-[A-Za-z0-9]|postgres://[^$<{\" ]+:[^$<{\" ]+@)", text):
        raise PromotionError(f"credential-shaped literal detected: {path}")
    return text


def _validate_images(texts: Iterable[str]) -> None:
    """Require exactly the release-owned, digest-pinned core and runner images."""

    images: list[str] = []
    for text in texts:
        images.extend(IMAGE_LINE_RE.findall(text))
    if not images:
        raise PromotionError("the Flux bundle contains no container images")

    for image in images:
        if image.startswith(CORE_IMAGE_PREFIX):
            digest = image.removeprefix(CORE_IMAGE_PREFIX)
        elif image.startswith(RUNNER_IMAGE_PREFIX):
            digest = image.removeprefix(RUNNER_IMAGE_PREFIX)
        else:
            raise PromotionError(f"image is not a PiTools release image: {image}")
        if SHA256_RE.fullmatch(digest) is None:
            raise PromotionError(f"image digest is not a 64-character SHA-256: {image}")

    if not any(image.startswith(CORE_IMAGE_PREFIX) for image in images):
        raise PromotionError("the core PiTools image is missing")
    if not any(image.startswith(RUNNER_IMAGE_PREFIX) for image in images):
        raise PromotionError("the Pi worker image is missing")


def validate_bundle(source: Path) -> Path:
    """Validate and return the resolved source Flux bundle directory."""

    resolved_source = _require_directory(source, name="source bundle")
    if resolved_source.name != SOURCE_BUNDLE_NAME:
        raise PromotionError(
            f"source bundle must end in {SOURCE_BUNDLE_NAME}: {resolved_source}"
        )

    children = list(resolved_source.iterdir())
    if any(child.is_symlink() for child in children):
        raise PromotionError("source bundle must not contain symlinks")
    if any(child.is_dir() for child in children):
        raise PromotionError("source bundle must not contain nested directories")
    actual_files = {child.name for child in children if child.is_file()}
    if actual_files != set(REQUIRED_FILES):
        raise PromotionError(
            f"source bundle files must be exactly {sorted(REQUIRED_FILES)}; "
            f"found {sorted(actual_files)}"
        )
    for filename in REQUIRED_FILES:
        _require_file(resolved_source / filename, description="source bundle file")

    _validate_kustomization(
        resolved_source / "kustomization.yaml",
        expected_resources={"app.yaml", "route.yaml", "networkpolicy.yaml"},
    )
    manifest_texts = [
        _validate_manifest(resolved_source / filename)
        for filename in ("app.yaml", "route.yaml", "networkpolicy.yaml")
    ]
    _validate_images(manifest_texts)
    return resolved_source


def _validate_target_layout(
    source: Path,
    destination: Path,
    aggregate: Path,
) -> tuple[Path, Path, Path]:
    """Validate the source/target paths before any target file is removed."""

    resolved_source = validate_bundle(source)
    if destination.name != SOURCE_BUNDLE_NAME:
        raise PromotionError(f"destination must end in {SOURCE_BUNDLE_NAME}: {destination}")
    if aggregate.name != AGGREGATE_NAME or aggregate.parent.name != "apps":
        raise PromotionError(f"aggregate is outside the apps layout: {aggregate}")

    resolved_destination = destination.resolve()
    resolved_aggregate = aggregate.resolve()
    if resolved_destination == resolved_source:
        raise PromotionError("source and destination must be different directories")
    if resolved_destination.parent != resolved_aggregate.parent:
        raise PromotionError("destination and aggregate must share the apps directory")
    if aggregate.is_symlink() or not aggregate.is_file():
        raise PromotionError(f"target aggregate is not a regular file: {aggregate}")
    _validate_kustomization_for_target(aggregate)
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise PromotionError(f"target destination is not a regular directory: {destination}")
    return resolved_source, resolved_destination, resolved_aggregate


def _validate_kustomization_for_target(path: Path) -> list[tuple[str, str]]:
    """Validate the target apps aggregate and return its resource entries."""

    text = _read_text(path)
    if not re.search(r"^\s*kind:\s*Kustomization\s*$", text, re.MULTILINE):
        raise PromotionError(f"target aggregate is not a Kustomization: {path}")
    entries = _resource_entries(text, description=str(path))
    for _, resource in entries:
        _validate_resource_path(resource, description=str(path))
    return entries


def _ensure_resource(aggregate: Path, resource: str) -> None:
    """Add one exact resource to a target aggregate without rewriting it."""

    entries = _validate_kustomization_for_target(aggregate)
    if any(existing == resource for _, existing in entries):
        if sum(existing == resource for _, existing in entries) > 1:
            raise PromotionError(f"target aggregate contains duplicate resource: {resource}")
        return

    text = _read_text(aggregate)
    lines = text if text.endswith("\n") else f"{text}\n"
    split_lines = lines.splitlines(keepends=True)
    resources_index = next(
        index
        for index, line in enumerate(split_lines)
        if re.fullmatch(r"(\s*)resources:\s*\n?", line)
    )
    last_resource_index = resources_index
    resource_indent: str | None = None
    for index in range(resources_index + 1, len(split_lines)):
        line = split_lines[index]
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = RESOURCE_LINE_RE.fullmatch(line.rstrip("\n"))
        if match is None:
            break
        last_resource_index = index
        resource_indent = match.group(1)
    if last_resource_index == resources_index or resource_indent is None:
        raise PromotionError(f"target aggregate resources list cannot be extended: {aggregate}")
    split_lines.insert(last_resource_index + 1, f"{resource_indent}- {resource}\n")
    aggregate.write_text("".join(split_lines), encoding="utf-8")


def synchronize_bundle(source: Path, destination: Path, aggregate: Path) -> None:
    """Copy a validated bundle and register it in the target apps aggregate."""

    _, resolved_destination, resolved_aggregate = _validate_target_layout(
        source, destination, aggregate
    )
    resolved_destination.parent.mkdir(parents=True, exist_ok=True)
    if resolved_destination.exists():
        for child in resolved_destination.iterdir():
            if child.is_symlink():
                raise PromotionError(f"target bundle contains a symlink: {child}")
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()
    else:
        resolved_destination.mkdir()

    source_path = Path(source).resolve()
    for child in source_path.iterdir():
        shutil.copy2(child, resolved_destination / child.name)
    _ensure_resource(resolved_aggregate, SOURCE_BUNDLE_NAME)


def _parser() -> argparse.ArgumentParser:
    """Build the command-line parser."""

    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate", help="validate a source bundle")
    validate.add_argument("--source", required=True, type=Path)
    sync = subparsers.add_parser("sync", help="copy a bundle into a target layout")
    sync.add_argument("--source", required=True, type=Path)
    sync.add_argument("--destination", required=True, type=Path)
    sync.add_argument("--aggregate", required=True, type=Path)
    return parser


def main(argv: list[str] | None = None) -> int:
    """Run the requested validation or synchronization operation."""

    args = _parser().parse_args(argv)
    try:
        if args.command == "validate":
            validate_bundle(args.source)
        else:
            synchronize_bundle(args.source, args.destination, args.aggregate)
    except PromotionError as error:
        print(f"Flux promotion rejected: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
