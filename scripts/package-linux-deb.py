#!/usr/bin/env python3
"""Build and verify an unsigned native Linux QC Remote Debian package."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import lzma
import os
from pathlib import Path
import posixpath
import re
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from typing import Iterator, Mapping, Sequence


ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "apps/windows/src-tauri"
RULES = ROOT / "services/device-broker/linux/70-qc-remote.rules"
PACKAGE_RULE_PATH = "/usr/share/qc-remote/70-qc-remote.rules"
MAX_TIMEOUT_SECONDS = 3600


class PackageError(RuntimeError):
    pass


def run_owned(
    command: Sequence[str], *, cwd: Path, timeout: float, env: Mapping[str, str] | None = None
) -> str:
    """Run a build command in an owned process group and always reap it."""
    if os.name != "posix":
        raise PackageError("Linux packaging must run on Linux")
    if timeout <= 0 or timeout > MAX_TIMEOUT_SECONDS:
        raise ValueError(f"timeout must be between 0 and {MAX_TIMEOUT_SECONDS} seconds")

    process = subprocess.Popen(
        list(command),
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
        env=env,
    )
    output = ""
    group_stopped = False
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as exc:
        _stop_process_group(process)
        group_stopped = True
        output, _ = process.communicate()
        raise PackageError(
            f"command timed out after {timeout:g}s: {' '.join(command)}\n{output[-8000:]}"
        ) from exc
    finally:
        # Commands in this entrypoint are not allowed to leave persistent helpers.
        # Their process group is unique to this invocation.
        if not group_stopped:
            _stop_process_group(process)
        if process.poll() is None:
            process.wait()

    if process.returncode:
        raise PackageError(
            f"command exited {process.returncode}: {' '.join(command)}\n{output[-8000:]}"
        )
    return output


def _stop_process_group(process: subprocess.Popen[str]) -> None:
    if process.pid <= 0:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 0.4
    while time.monotonic() < deadline:
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.025)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def linux_host_triple(rustc_output: str) -> str:
    match = re.search(r"(?m)^host:\s*(\S+)\s*$", rustc_output)
    if not match:
        raise PackageError("could not read the Linux target triple from rustc -vV")
    target = match.group(1)
    if not target.endswith("-unknown-linux-gnu"):
        raise PackageError(f"Debian packaging requires a native GNU/Linux target, got {target}")
    return target


def active_rust_toolchain() -> str:
    # Resolve from the repository root, whose active Linux toolchain applies to
    # the package command. The app directory contains a Windows-only pin.
    active = run_owned(["rustup", "show", "active-toolchain"], cwd=ROOT, timeout=15)
    match = re.search(r"(?m)^\s*(\S+)", active)
    if not match:
        raise PackageError("rustup did not report an active host toolchain")
    return match.group(1)


@contextlib.contextmanager
def stage_sidecar(source: Path, destination: Path) -> Iterator[Path]:
    """Install one exclusive, temporary executable input."""
    source.lstat()
    if not source.is_file() or source.is_symlink():
        raise PackageError(f"executable input must be a regular file, not a symlink: {source}")
    source = source.resolve(strict=True)
    metadata = source.stat()
    if metadata.st_size == 0 or not os.access(source, os.X_OK):
        raise PackageError(f"executable input must be a nonempty executable regular file: {source}")
    created_parent = False
    try:
        destination.parent.mkdir(mode=0o755)
        created_parent = True
    except FileExistsError:
        destination.parent.lstat()
        if not destination.parent.is_dir() or destination.parent.is_symlink():
            raise PackageError(f"Tauri sidecar directory is not a real directory: {destination.parent}")
    # Tauri's normal binaries directory may already exist. Never replace a path
    # that could belong to another invocation or a developer.
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd: int | None = None
    identity: tuple[int, int] | None = None
    try:
        fd = os.open(destination, flags, 0o700)
        st = os.fstat(fd)
        identity = (st.st_dev, st.st_ino)
        with source.open("rb") as input_file, os.fdopen(fd, "wb", closefd=False) as output_file:
            shutil.copyfileobj(input_file, output_file, length=1024 * 1024)
            output_file.flush()
            os.fsync(output_file.fileno())
        os.chmod(destination, 0o700, follow_symlinks=False)
        yield destination
    finally:
        if fd is not None:
            os.close(fd)
        if identity is not None:
            try:
                current = destination.lstat()
                if (current.st_dev, current.st_ino) == identity and stat.S_ISREG(current.st_mode):
                    destination.unlink()
            except FileNotFoundError:
                pass
        if created_parent:
            try:
                destination.parent.rmdir()
            except OSError:
                pass


@contextlib.contextmanager
def stage_sidecar_with_digest(
    source: Path, destination: Path
) -> Iterator[tuple[Path, str]]:
    """Yield the staged external binary and the digest of those exact bytes."""
    with stage_sidecar(source, destination) as staged:
        yield staged, sha256_file(staged)


def expected_deb_app_sha256(source: Path, destination: Path) -> str:
    """Hash Tauri 2.11.5's DEB marker image without changing the app binary.

    Tauri CLI 2.11.5 is pinned in package-lock.json and the matching
    tauri-v2.11.5 source (crates/tauri-bundler/src/bundle.rs) replaces the
    27-byte UNK marker with the equal-length DEB marker for a Debian bundle.
    """
    token = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
    replacement = b"__TAURI_BUNDLE_TYPE_VAR_DEB"
    if len(token) != len(replacement):
        raise PackageError("Tauri bundle marker replacement changed binary length")
    with stage_sidecar(source, destination) as patched:
        matches: list[int] = []
        overlap = b""
        processed = 0
        with patched.open("rb") as binary:
            while block := binary.read(1024 * 1024):
                combined = overlap + block
                base = processed - len(overlap)
                min_new_start = max(0, processed - len(token) + 1)
                search_at = max(0, min_new_start - base)
                while True:
                    found = combined.find(token, search_at)
                    if found < 0:
                        break
                    matches.append(base + found)
                    search_at = found + 1
                processed += len(block)
                overlap = combined[-(len(token) - 1):]
        if len(matches) != 1:
            raise PackageError(f"expected one Tauri bundle marker in app binary; found {len(matches)}")
        with patched.open("r+b") as binary:
            binary.seek(matches[0])
            binary.write(replacement)
            binary.flush()
            os.fsync(binary.fileno())
        return sha256_file(patched)


def run_owned_to_file(
    command: Sequence[str],
    *,
    cwd: Path,
    timeout: float,
    output: Path,
    env: Mapping[str, str] | None = None,
) -> None:
    """Stream a bounded native command's binary stdout to one owned file."""
    if os.name != "posix":
        raise PackageError("Debian package inspection requires Linux")
    if timeout <= 0 or timeout > MAX_TIMEOUT_SECONDS:
        raise ValueError(f"timeout must be between 0 and {MAX_TIMEOUT_SECONDS} seconds")
    process: subprocess.Popen[bytes] | None = None
    identity: tuple[int, int] | None = None
    try:
        with output.open("xb") as output_file:
            created = os.fstat(output_file.fileno())
            identity = (created.st_dev, created.st_ino)
            process = subprocess.Popen(
                list(command),
                cwd=cwd,
                stdin=subprocess.DEVNULL,
                stdout=output_file,
                stderr=subprocess.PIPE,
                start_new_session=True,
                env=env,
            )
            try:
                _stdout, stderr = process.communicate(timeout=timeout)
            except subprocess.TimeoutExpired as error:
                _stop_process_group(process)
                _stdout, stderr = process.communicate()
                detail = (stderr or b"").decode("utf-8", errors="replace")[-4000:]
                raise PackageError(
                    f"binary command timed out after {timeout:g}s: {' '.join(command)}\n{detail}"
                ) from error
            finally:
                if process.poll() is None:
                    _stop_process_group(process)
                    process.wait()
                else:
                    _stop_process_group(process)
        if process.returncode:
            detail = (stderr or b"").decode("utf-8", errors="replace")[-4000:]
            raise PackageError(
                f"binary command exited {process.returncode}: {' '.join(command)}\n{detail}"
            )
        if output.stat().st_size == 0:
            raise PackageError(f"binary command produced empty output: {' '.join(command)}")
    except BaseException:
        if identity is not None:
            try:
                current = output.lstat()
                if (current.st_dev, current.st_ino) == identity and stat.S_ISREG(current.st_mode):
                    output.unlink()
            except FileNotFoundError:
                pass
        raise


def _extract_data_tar(deb: Path, output: Path, *, timeout: float = 120) -> None:
    data_offset: int | None = None
    data_size = 0
    data_name: str | None = None
    with deb.open("rb") as archive:
        if archive.read(8) != b"!<arch>\n":
            raise PackageError(f"not a Debian ar archive: {deb}")
        has_version = False
        while True:
            header = archive.read(60)
            if not header:
                break
            if len(header) != 60 or header[58:60] != b"`\n":
                raise PackageError(f"malformed ar member header in {deb}")
            name = header[:16].decode("ascii", errors="strict").strip().rstrip("/")
            try:
                size = int(header[48:58].decode("ascii").strip())
            except ValueError as error:
                raise PackageError(f"malformed ar member size in {deb}") from error
            if size < 0:
                raise PackageError(f"negative ar member size in {deb}")
            member_offset = archive.tell()
            if name == "debian-binary":
                if archive.read(size) != b"2.0\n":
                    raise PackageError(f"unsupported Debian package format in {deb}")
                has_version = True
                if size & 1:
                    archive.seek(1, os.SEEK_CUR)
                continue
            if name.startswith("data.tar."):
                if not has_version:
                    raise PackageError(f"Debian archive is missing debian-binary: {deb}")
                data_offset = member_offset
                data_size = size
                data_name = name
                break
            archive.seek(size, os.SEEK_CUR)
            if size & 1:
                archive.seek(1, os.SEEK_CUR)
    if data_offset is None or data_name is None:
        raise PackageError(f"Debian archive has no data.tar.* member: {deb}")

    if data_name == "data.tar.xz":
        compressed = output.with_name(output.name + ".xz")
        compressed_identity: tuple[int, int] | None = None
        try:
            with deb.open("rb") as archive, compressed.open("xb") as compressed_file:
                created = os.fstat(compressed_file.fileno())
                compressed_identity = (created.st_dev, created.st_ino)
                archive.seek(data_offset)
                remaining = data_size
                while remaining:
                    block = archive.read(min(1024 * 1024, remaining))
                    if not block:
                        raise PackageError(f"truncated data archive in {deb}")
                    compressed_file.write(block)
                    remaining -= len(block)
            with lzma.open(compressed, "rb") as source, output.open("xb") as destination:
                shutil.copyfileobj(source, destination, length=1024 * 1024)
        except lzma.LZMAError as error:
            raise PackageError(f"cannot decompress Debian data archive: {error}") from error
        finally:
            if compressed_identity is not None:
                try:
                    current = compressed.lstat()
                    if (current.st_dev, current.st_ino) == compressed_identity and stat.S_ISREG(current.st_mode):
                        compressed.unlink()
                except FileNotFoundError:
                    pass
        return

    # dpkg-deb normalizes zstd and other Debian compression formats to a plain
    # tar stream without buffering package contents in memory.
    run_owned_to_file(
        ["dpkg-deb", "--fsys-tarfile", str(deb)],
        cwd=deb.parent,
        timeout=timeout,
        output=output,
    )


@contextlib.contextmanager
def open_deb_data_tar(deb: Path, *, timeout: float = 120) -> Iterator[Path]:
    with tempfile.TemporaryDirectory(prefix="qc-deb-inspect-") as temporary:
        data_tar = Path(temporary) / "data.tar"
        _extract_data_tar(deb, data_tar, timeout=timeout)
        yield data_tar


def verify_deb(
    deb: Path,
    *,
    target: str,
    expected_hashes: Mapping[str, str],
) -> None:
    if not deb.is_file() or deb.stat().st_size == 0:
        raise PackageError(f"Tauri did not produce a Debian package at {deb}")
    try:
        package_files: dict[str, tuple[str, int]] = {}
        with open_deb_data_tar(deb) as data_tar:
            with tarfile.open(data_tar, mode="r:") as package_archive:
                for member in package_archive.getmembers():
                    path = "/" + posixpath.normpath(member.name).removeprefix("./").lstrip("/")
                    if path in ("/usr/bin/qc-remote", "/usr/bin/qc-device-broker", PACKAGE_RULE_PATH):
                        if not member.isfile():
                            raise PackageError(f"package member is not a regular file: {path}")
                        if path != PACKAGE_RULE_PATH and not member.mode & 0o111:
                            raise PackageError(f"package executable is not executable: {path}")
                        if path in package_files:
                            raise PackageError(f"Debian data archive contains duplicate member: {path}")
                        stream = package_archive.extractfile(member)
                        if stream is None:
                            raise PackageError(f"cannot read packaged executable: {path}")
                        digest = hashlib.sha256()
                        with stream:
                            shutil.copyfileobj(stream, _HashSink(digest))
                        package_files[path] = (digest.hexdigest(), member.mode)
    except (tarfile.TarError, lzma.LZMAError, EOFError) as error:
        raise PackageError(f"cannot inspect Debian data archive: {error}") from error

    required_paths = ("/usr/bin/qc-remote", "/usr/bin/qc-device-broker", PACKAGE_RULE_PATH)
    missing = [path for path in required_paths if path not in package_files]
    if missing:
        raise PackageError(f"{deb.name} is missing exact package members {missing}; target {target}")
    mismatches = [
        path for path, expected in expected_hashes.items()
        if path not in package_files or package_files[path][0] != expected
    ]
    if mismatches:
        raise PackageError(f"{deb.name} contains unexpected binary bytes at {mismatches}")


class _HashSink:
    def __init__(self, digest) -> None:
        self.digest = digest

    def write(self, block: bytes) -> int:
        self.digest.update(block)
        return len(block)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def publish_package(source: Path, output: Path) -> Path:
    output = output.absolute()
    if not output.parent.is_dir() or output.parent.is_symlink():
        raise PackageError(f"output parent must be an existing real directory: {output.parent}")
    source_hash = sha256_file(source)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(output, flags, 0o600)
    identity: tuple[int, int] | None = None
    try:
        st = os.fstat(fd)
        identity = (st.st_dev, st.st_ino)
        with source.open("rb") as input_file, os.fdopen(fd, "wb", closefd=False) as output_file:
            shutil.copyfileobj(input_file, output_file, length=1024 * 1024)
            output_file.flush()
            os.fsync(output_file.fileno())
        if sha256_file(output) != source_hash:
            raise PackageError("published package bytes do not match the verified package")
    except BaseException:
        if identity is not None:
            try:
                current = output.lstat()
                if (current.st_dev, current.st_ino) == identity and stat.S_ISREG(current.st_mode):
                    output.unlink()
            except FileNotFoundError:
                pass
        raise
    finally:
        os.close(fd)
    return output


def package(
    *,
    timeout: float,
    broker_binary: Path | None = None,
    app_binary: Path | None = None,
    output: Path | None = None,
) -> Path:
    if sys.platform != "linux":
        raise PackageError("package-linux-deb.py must run on Linux")
    if not RULES.is_file():
        raise PackageError(f"the scoped udev rules file is missing: {RULES}")
    if (app_binary is None) != (broker_binary is None):
        raise PackageError("--prebuilt-app and --broker-binary must be supplied together")
    if app_binary is not None and output is None:
        raise PackageError("--output is required when packaging paired prebuilt binaries")
    if output is not None and app_binary is None:
        raise PackageError("--output is only used with --prebuilt-app and --broker-binary")

    rustc = run_owned(["rustc", "-vV"], cwd=ROOT, timeout=timeout)
    target = linux_host_triple(rustc)
    toolchain = os.environ.get("RUSTUP_TOOLCHAIN") or active_rust_toolchain()
    staged = APP / "binaries" / f"qc-device-broker-{target}"

    if app_binary is not None and broker_binary is not None:
        if app_binary.is_symlink() or broker_binary.is_symlink():
            raise PackageError("prebuilt executable paths cannot be symlinks")
        app_binary = app_binary.resolve(strict=True)
        broker_binary = broker_binary.resolve(strict=True)
        for label, binary in (("app", app_binary), ("broker", broker_binary)):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise PackageError(f"prebuilt {label} must be a regular executable file: {binary}")
        with tempfile.TemporaryDirectory(prefix="qc-linux-deb-bundle-") as staging_name:
            target_dir = Path(staging_name)
            debug_dir = target_dir / target / "debug"
            debug_dir.mkdir(parents=True)
            staged_app = debug_dir / "qc-remote"
            shutil.copy2(app_binary, staged_app)
            if not os.access(staged_app, os.X_OK):
                raise PackageError("staged prebuilt app lost its executable mode")
            with stage_sidecar(broker_binary, staged):
                run_owned([str(broker_binary), "--self-check"], cwd=ROOT, timeout=timeout)
                package_env = os.environ.copy()
                package_env["CARGO_TARGET_DIR"] = str(target_dir)
                package_env["RUSTUP_TOOLCHAIN"] = toolchain
                package_env.pop("TAURI_CONFIG", None)
                # `tauri bundle` packages an already prepared Cargo target.
                # Unlike `tauri build`, it does not copy configured externalBin
                # inputs into that target, so place the same exact sidecar there
                # for this scoped prebuilt-bundle invocation.
                with stage_sidecar(broker_binary, debug_dir / "qc-device-broker"):
                    run_owned(
                        [
                            "npm", "exec", "--workspace", "@qc-remote/windows", "--", "tauri", "bundle",
                            "--bundles", "deb", "--features", "tray-sidecar,custom-protocol",
                            "--target", target, "--debug", "--ci",
                        ],
                        cwd=ROOT,
                        timeout=timeout,
                        env=package_env,
                    )
            deb_dir = debug_dir / "bundle/deb"
            packages = list(deb_dir.glob("*.deb")) if deb_dir.is_dir() else []
            if len(packages) != 1:
                raise PackageError(f"Tauri produced {len(packages)} Debian packages in {deb_dir}")
            expected_app_hash = expected_deb_app_sha256(
                staged_app, debug_dir / "qc-remote-expected-deb"
            )
            expected_hashes = {
                # Tauri patches its bundle copy with a fixed marker and restores
                # the target binary afterwards. Compare against a private copy
                # transformed with the exact same equal-length marker update.
                "/usr/bin/qc-remote": expected_app_hash,
                "/usr/bin/qc-device-broker": sha256_file(broker_binary),
                PACKAGE_RULE_PATH: sha256_file(RULES),
            }
            verify_deb(packages[0], target=target, expected_hashes=expected_hashes)
            return publish_package(packages[0], output)

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", APP / "target")).resolve()
    build_env = os.environ.copy()
    build_env["RUSTUP_TOOLCHAIN"] = toolchain
    build_env["TAURI_CONFIG"] = json.dumps({"bundle": {"externalBin": []}}, separators=(",", ":"))
    run_owned(["npm", "run", "build", "--workspace", "@qc-remote/windows"], cwd=ROOT, timeout=timeout)
    run_owned(
        [
            "cargo", "build", "--locked", "--release",
            "--manifest-path", str(APP / "Cargo.toml"),
            "--features", "tray-sidecar,custom-protocol",
            "--bin", "qc-device-broker", "--target", target,
        ],
        cwd=ROOT,
        timeout=timeout,
        env=build_env,
    )
    broker_binary = target_dir / target / "release/qc-device-broker"
    run_owned([str(broker_binary), "--self-check"], cwd=ROOT, timeout=timeout)

    deb_dir = target_dir / target / "release/bundle/deb"
    prior_packages = {
        path: (path.stat().st_dev, path.stat().st_ino, path.stat().st_size, path.stat().st_mtime_ns)
        for path in deb_dir.glob("*.deb")
    } if deb_dir.is_dir() else {}
    with stage_sidecar_with_digest(broker_binary, staged) as (_staged, expected_broker_hash):
        package_env = {key: value for key, value in os.environ.items() if key != "TAURI_CONFIG"}
        package_env["RUSTUP_TOOLCHAIN"] = toolchain
        run_owned(
            [
                "npm", "run", "tauri:build", "--workspace", "@qc-remote/windows", "--",
                "--bundles", "deb", "--features", "tray-sidecar,custom-protocol",
                "--target", target, "--ci",
            ],
            cwd=ROOT,
            timeout=timeout,
            env=package_env,
        )

    packages = [
        path for path in deb_dir.glob("*.deb")
        if prior_packages.get(path)
        != (path.stat().st_dev, path.stat().st_ino, path.stat().st_size, path.stat().st_mtime_ns)
    ]
    packages.sort(key=lambda item: item.stat().st_mtime_ns, reverse=True)
    if not packages:
        raise PackageError(f"Tauri reported success but no .deb exists in {deb_dir}")
    expected_hashes = {
        "/usr/bin/qc-remote": expected_deb_app_sha256(
            target_dir / target / "release/qc-remote",
            target_dir / target / "release/qc-remote-expected-deb",
        ),
        # Tauri builds the Cargo-discovered broker binary while bundling, then
        # copies the staged externalBin over that same Debian destination. The
        # target output can therefore differ from the staged package input by
        # verification time; pin the digest before invoking Tauri.
        "/usr/bin/qc-device-broker": expected_broker_hash,
        PACKAGE_RULE_PATH: sha256_file(RULES),
    }
    verify_deb(packages[0], target=target, expected_hashes=expected_hashes)
    return packages[0]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--timeout-seconds", type=float, default=1800,
        help="per-command timeout (default: 1800; maximum: 3600)",
    )
    parser.add_argument(
        "--broker-binary", type=Path,
        help="with --prebuilt-app, use an existing native broker binary",
    )
    parser.add_argument(
        "--prebuilt-app", type=Path,
        help="bundle an existing QC Remote executable without recompiling the app",
    )
    parser.add_argument(
        "--output", type=Path,
        help="write a verified prebuilt-pair package to this new path",
    )
    args = parser.parse_args(argv)
    try:
        package_path = package(
            timeout=args.timeout_seconds,
            broker_binary=args.broker_binary,
            app_binary=args.prebuilt_app,
            output=args.output,
        )
    except (PackageError, OSError, ValueError) as error:
        print(f"package-linux-deb: {error}", file=sys.stderr)
        return 1
    print(f"Verified unsigned Debian package: {package_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
