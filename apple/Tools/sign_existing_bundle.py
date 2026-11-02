#!/usr/bin/env python3
"""TIN-6964: sign one pinned retained carrier without building or installing it."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import stat
import subprocess
import sys

TEAM = "QP994XQKNH"
IDENTITY = "61A8E77C4F3D678921FF0A7DC9D7E317F7754F50"
GROUP = TEAM + ".io.tinyland.dcx2496"
VERSION = ("0.1.7", "8")
COMPONENTS = (
    (
        "dcxctl",
        "Contents/Helpers/dcxctl",
        "",
        None,
        {"com.apple.security.app-sandbox": True, "com.apple.security.inherit": True},
    ),
    (
        "AU",
        "Contents/PlugIns/DCXControlAU.appex",
        "Contents/MacOS/DCXControlAU",
        "io.tinyland.dcx2496.logic.midi",
        {
            "com.apple.security.app-sandbox": True,
            "com.apple.security.application-groups": [GROUP],
            "com.apple.security.files.user-selected.read-only": True,
        },
    ),
    (
        "helper",
        ".",
        "Contents/MacOS/DCXLogicHelper",
        "io.tinyland.dcx2496.logic",
        {
            "com.apple.security.app-sandbox": True,
            "com.apple.security.application-groups": [GROUP],
            "com.apple.security.device.serial": True,
        },
    ),
)


class Refused(Exception):
    pass


def file_digest(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise Refused("regular file required")
        digest = hashlib.sha256()
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            for data in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(data)
        after = os.fstat(descriptor)
        if (before.st_ino, before.st_size, before.st_mtime_ns) != (
            after.st_ino,
            after.st_size,
            after.st_mtime_ns,
        ):
            raise Refused("file changed during read")
        return digest.hexdigest()
    finally:
        os.close(descriptor)


def manifest(root):
    """Lab-compatible JSON: sorted relative file hashes or symlink targets."""
    entries = []
    for path in sorted(root.rglob("*")):
        relative = str(path.relative_to(root))
        if path.is_symlink():
            target = path.resolve(strict=True)
            if root not in target.parents and target != root:
                raise Refused("bundle link escapes input")
            entries.append({"path": relative, "link": str(path.readlink())})
        elif path.is_file():
            entries.append({"path": relative, "sha256": file_digest(path)})
        elif not path.is_dir():
            raise Refused("bundle contains special file")
    encoded = json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()
    return entries, hashlib.sha256(encoded).hexdigest()


def private_directory(path):
    if not path.is_absolute() or path.resolve(strict=True) != path:
        raise Refused("canonical absolute directory required")
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or metadata.st_mode & 0o077
    ):
        raise Refused("caller-owned private directory required")


def validate_paths(source, output):
    private_directory(source.parent)
    if (
        source.suffix != ".bundle"
        or output.suffix != ".bundle"
        or output.parent != source.parent
        or source == output
    ):
        raise Refused("distinct private sibling .bundle paths required")
    if source.resolve(strict=True) != source or not source.is_dir():
        raise Refused("canonical source bundle required")
    if output.exists() or output.is_symlink():
        raise Refused("output must be absent")
    for path in (source, *source.rglob("*")):
        if path.lstat().st_uid != os.getuid():
            raise Refused("foreign bundle owner")


def directory_identity(path):
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or path.resolve(strict=True) != path
    ):
        raise Refused("owned canonical output directory required")
    return metadata.st_dev, metadata.st_ino


def require_reserved_output(output, identity):
    if directory_identity(output) != identity:
        raise Refused("reserved output directory changed")


def validate_structure(bundle):
    for name, relative, binary_relative, identifier, _ in COMPONENTS:
        item = bundle / relative
        binary = item / binary_relative if binary_relative else item
        if binary.is_symlink() or not binary.is_file():
            raise Refused("exact component executable required")
        if name == "dcxctl":
            continue
        info_path = item / "Contents/Info.plist"
        if info_path.is_symlink():
            raise Refused("regular bundle plist required")
        info = plistlib.loads(info_path.read_bytes())
        if (
            info.get("CFBundleIdentifier"),
            info.get("CFBundleShortVersionString"),
            info.get("CFBundleVersion"),
        ) != (identifier, *VERSION):
            raise Refused("bundle identity or version differs")
        if (item / "Contents/embedded.provisionprofile").exists() or (
            item / "Contents/embedded.provisionprofile"
        ).is_symlink():
            raise Refused("unexpected provisioning profile")
        if (
            name == "helper"
            and info.get("TinylandRoomPreparationProtocol") != "dcx.room-preparation/v1"
        ):
            raise Refused("room preparation capability missing")
        if name == "AU":
            extension = info.get("NSExtension", {})
            components = extension.get("NSExtensionAttributes", {}).get(
                "AudioComponents", []
            )
            if len(components) != 1 or any(
                components[0].get(key) != value
                for key, value in (
                    ("type", "aumi"),
                    ("subtype", "DcxC"),
                    ("manufacturer", "TnLd"),
                )
            ):
                raise Refused("Audio Unit component differs")


def command(arguments):
    result = subprocess.run(arguments, capture_output=True, check=False, timeout=45)
    if result.returncode:
        raise Refused(
            "bounded command failed: "
            + arguments[0]
            + ": "
            + result.stderr.decode(errors="replace")[:512]
        )
    return result


def signature_identifier(item, run=command):
    result = run(["/usr/bin/codesign", "-dvvv", str(item)])
    metadata = (result.stdout + result.stderr).decode(errors="replace")
    values = re.findall(
        r"^Identifier=([A-Za-z0-9][A-Za-z0-9._-]{0,127})$", metadata, re.M
    )
    if len(values) != 1:
        raise Refused("existing CLI signing identifier unreadable")
    return values[0], metadata


def signing_arguments(item, identifier, entitlements, keychain):
    return [
        "/usr/bin/codesign",
        "--force",
        "--sign",
        IDENTITY,
        "--keychain",
        str(keychain),
        "--identifier",
        identifier,
        "--options",
        "runtime",
        "--timestamp=none",
        "--entitlements",
        str(entitlements),
        str(item),
    ]


def verify_component(item, binary, identifier, expected, run=command):
    requirement = 'anchor apple generic and certificate leaf = H"' + IDENTITY + '"'
    run(["/usr/bin/codesign", "--verify", "--strict", "-R=" + requirement, str(item)])
    claims = plistlib.loads(
        run(
            [
                "/usr/bin/codesign",
                "--display",
                "--entitlements",
                "-",
                "--xml",
                str(item),
            ]
        ).stdout
    )
    if claims != expected:
        raise Refused("exact signed entitlements differ")
    observed_identifier, metadata = signature_identifier(item, run=run)
    if (
        "TeamIdentifier=" + TEAM + "\n" not in metadata
        or observed_identifier != identifier
    ):
        raise Refused("signature identifier or team differs")
    if run(["/usr/bin/lipo", "-archs", str(binary)]).stdout.decode().strip() != "arm64":
        raise Refused("arm64 component required")
    return {
        "identifier": identifier,
        "binarySha256": file_digest(binary),
        "entitlements": claims,
        "strictCmsLeafVerified": True,
        "signatureMetadata": metadata,
    }


def sign_existing(source, output, expected_digest, run=command):
    if not re.fullmatch("[0-9a-f]{64}", expected_digest):
        raise Refused("exact input SHA256 required")
    validate_paths(source, output)
    before_entries, before = manifest(source)
    if before != expected_digest:
        raise Refused("input manifest differs")
    validate_structure(source)
    keychain = Path.home() / "Library/Keychains/login.keychain-db"
    if not keychain.is_file() or keychain.resolve(strict=True) != keychain:
        raise Refused("exact existing login keychain required")
    identities = run(
        ["/usr/bin/security", "find-identity", "-v", "-p", "codesigning", str(keychain)]
    ).stdout.decode()
    found = [
        (fingerprint, label)
        for fingerprint, label in re.findall(
            r'\b([0-9A-F]{40}) "([^"\n]+)"', identities
        )
        if fingerprint == IDENTITY
        and label.startswith("Developer ID Application:")
        and "(" + TEAM + ")" in label
    ]
    if len(found) != 1:
        raise Refused("exact existing Developer ID identity required")
    for _, relative, binary_relative, _, _ in COMPONENTS:
        item = source / relative
        binary = item / binary_relative if binary_relative else item
        if (
            run(["/usr/bin/lipo", "-archs", str(binary)]).stdout.decode().strip()
            != "arm64"
        ):
            raise Refused("input must be arm64")
    cli_identifier, _ = signature_identifier(
        source / "Contents/Helpers/dcxctl", run=run
    )
    try:
        output.mkdir(mode=0o700, exist_ok=False)
    except FileExistsError as error:
        raise Refused("output appeared during preflight") from error
    output_identity = directory_identity(output)
    run(["/usr/bin/ditto", str(source), str(output)])
    require_reserved_output(output, output_identity)
    if manifest(output)[1] != before or manifest(source)[1] != before:
        raise Refused("copied manifest differs")
    for path in source.rglob("*"):
        if path.is_file() and not path.is_symlink():
            copied = output / path.relative_to(source)
            if os.path.samestat(path.stat(), copied.stat()):
                raise Refused("output shares an input file inode")
    # TIN-6964: committed dictionaries are materialized only under the private
    # operation parent. The artifact's own plist cannot grant extra privileges.
    components = []
    for name, relative, binary_relative, identifier, entitlements in COMPONENTS:
        require_reserved_output(output, output_identity)
        item = output / relative
        binary = item / binary_relative if binary_relative else item
        if name == "dcxctl":
            identifier = cli_identifier
        entitlement_file = output.parent / (output.name + "." + name + ".entitlements")
        with entitlement_file.open("xb") as stream:
            stream.write(plistlib.dumps(entitlements, sort_keys=True))
        require_reserved_output(output, output_identity)
        run(signing_arguments(item, identifier, entitlement_file, keychain))
        require_reserved_output(output, output_identity)
        verified = verify_component(item, binary, identifier, entitlements, run=run)
        verified["name"] = name
        components.append(verified)
    require_reserved_output(output, output_identity)
    run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(output)])
    validate_structure(output)
    entries, digest = manifest(output)
    if manifest(source)[1] != before:
        raise Refused("immutable input changed")
    return {
        "schemaVersion": "dcx.signed-existing-bundle/v1",
        "inputManifestSha256": before,
        "outputManifestSha256": digest,
        "inputManifest": before_entries,
        "outputManifest": entries,
        "components": components,
        "installed": False,
        "launched": False,
        "deviceAccessRequested": False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("manifest_sha256")
    args = parser.parse_args()
    os.umask(0o077)
    if sys.platform != "darwin" or os.getuid() == 0:
        raise Refused("ordinary Darwin user required")
    if command(["/bin/launchctl", "managername"]).stdout.decode().strip() != "Aqua":
        raise Refused("Lab-owned Aqua execution required. No automatic transition.")
    receipt = sign_existing(args.input, args.output, args.manifest_sha256)
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (Refused, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print("sign-existing-bundle refused: " + str(error), file=sys.stderr)
        raise SystemExit(1)
