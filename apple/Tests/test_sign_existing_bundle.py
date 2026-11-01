#!/usr/bin/env python3
"""TIN-6964: filesystem and injected-command signing contract. No native tools."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import random
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SIGNER = Path(sys.argv.pop(1)).resolve()
spec = importlib.util.spec_from_file_location("signer", SIGNER)
signer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(signer)


class Backend:
    def __init__(self):
        self.calls = []
        self.signed = {}
        self.failure_at = None
        self.architecture = "arm64"
        self.extra_entitlement = False
        self.wrong_team = False
        self.duplicate_identity = False
        self.hardlink_copy = False
        self.create_output_during_identity = None
        self.replace_output_after_copy = False

    def run(self, args):
        self.calls.append(args)
        tool = Path(args[0]).name
        if tool == "security":
            if self.create_output_during_identity is not None:
                self.create_output_during_identity.mkdir()
            line = (
                "1) "
                + signer.IDENTITY
                + ' "Developer ID Application: SYNTHETIC ('
                + signer.TEAM
                + ')"\n'
            )
            return subprocess.CompletedProcess(
                args, 0, (line * (2 if self.duplicate_identity else 1)).encode(), b""
            )
        if tool == "lipo":
            return subprocess.CompletedProcess(
                args, 0, (self.architecture + "\n").encode(), b""
            )
        if tool == "ditto":
            shutil.copytree(
                args[1],
                args[2],
                symlinks=True,
                copy_function=os.link if self.hardlink_copy else shutil.copy2,
                dirs_exist_ok=True,
            )
            if self.replace_output_after_copy:
                output = Path(args[2])
                output.rename(output.with_name("retained-original.bundle"))
                output.mkdir()
            return subprocess.CompletedProcess(args, 0, b"", b"")
        if tool != "codesign":
            raise AssertionError("unexpected command: " + repr(args))
        item = Path(args[-1])
        if "--force" in args:
            if self.failure_at == len(self.signed):
                raise signer.Refused("SYNTHETIC key-use failure")
            identifier = args[args.index("--identifier") + 1]
            entitlements = plistlib.loads(
                Path(args[args.index("--entitlements") + 1]).read_bytes()
            )
            self.signed[str(item)] = (identifier, entitlements)
            binary = (
                item
                if item.is_file()
                else item
                / (
                    "Contents/MacOS/DCXLogicHelper"
                    if identifier.endswith(".logic")
                    else "Contents/MacOS/DCXControlAU"
                )
            )
            binary.write_bytes(binary.read_bytes() + b"SYNTHETIC signature")
        elif "-dvvv" in args:
            identifier = self.signed.get(str(item), ("dcxctl-synthetic-existing", {}))[
                0
            ]
            team = "WRONG" if self.wrong_team else signer.TEAM
            return subprocess.CompletedProcess(
                args,
                0,
                b"",
                (
                    "Identifier=" + identifier + "\nTeamIdentifier=" + team + "\n"
                ).encode(),
            )
        elif "--entitlements" in args:
            claims = dict(self.signed[str(item)][1])
            if self.extra_entitlement:
                claims["com.apple.security.network.client"] = True
            return subprocess.CompletedProcess(args, 0, plistlib.dumps(claims), b"")
        elif "--verify" not in args:
            raise AssertionError("unexpected codesign operation")
        return subprocess.CompletedProcess(args, 0, b"", b"")


class SigningContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(
            prefix="dcx-sign-existing-contract-"
        )
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.root.chmod(0o700)
        self.source = self.root / "input.bundle"
        self.output = self.root / "output.bundle"
        self.source.mkdir()
        for name, relative, binary_relative, identifier, _ in signer.COMPONENTS:
            item = self.source / relative
            binary = item / binary_relative if binary_relative else item
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_bytes(("SYNTHETIC " + name).encode())
            if name != "dcxctl":
                info = {
                    "CFBundleIdentifier": identifier,
                    "CFBundleShortVersionString": "0.1.3",
                    "CFBundleVersion": "4",
                }
                if name == "helper":
                    info["TinylandRoomPreparationProtocol"] = "dcx.room-preparation/v1"
                else:
                    info["NSExtension"] = {
                        "NSExtensionAttributes": {
                            "AudioComponents": [
                                {
                                    "type": "aumi",
                                    "subtype": "DcxC",
                                    "manufacturer": "TnLd",
                                }
                            ]
                        }
                    }
                (item / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
        self.digest = signer.manifest(self.source)[1]
        self.home = self.root / "home"
        keychain = self.home / "Library/Keychains/login.keychain-db"
        keychain.parent.mkdir(parents=True)
        keychain.write_bytes(b"SYNTHETIC metadata-only keychain fixture")
        self.backend = Backend()

    def sign(self, digest=None):
        with patch.object(signer.Path, "home", return_value=self.home):
            return signer.sign_existing(
                self.source,
                self.output,
                self.digest if digest is None else digest,
                run=self.backend.run,
            )

    def change_info(self, relative, key, value):
        path = self.source / relative / "Contents/Info.plist"
        info = plistlib.loads(path.read_bytes())
        info[key] = value
        path.write_bytes(plistlib.dumps(info))
        self.digest = signer.manifest(self.source)[1]

    def testExactCopySignsChildThenAUThenHelperWithoutBuilding(self):
        result = self.sign()
        self.assertEqual(result["inputManifestSha256"], self.digest)
        self.assertEqual(signer.manifest(self.source)[1], self.digest)
        self.assertNotEqual(result["outputManifestSha256"], self.digest)
        self.assertEqual(
            [item["identifier"] for item in result["components"]],
            [
                "dcxctl-synthetic-existing",
                "io.tinyland.dcx2496.logic.midi",
                "io.tinyland.dcx2496.logic",
            ],
        )
        self.assertEqual(
            {Path(call[0]).name for call in self.backend.calls},
            {"security", "lipo", "ditto", "codesign"},
        )
        self.assertFalse(result["installed"])
        self.assertFalse(result["launched"])
        self.assertFalse(result["deviceAccessRequested"])
        signed = [call for call in self.backend.calls if "--force" in call]
        self.assertEqual(len(signed), 3)
        for call in signed:
            self.assertIn(signer.IDENTITY, call)
            self.assertIn(str(self.home / "Library/Keychains/login.keychain-db"), call)
        self.assertEqual(
            signed[0][signed[0].index("--identifier") + 1], "dcxctl-synthetic-existing"
        )
        self.assertEqual(
            sum(
                any(
                    arg.startswith(
                        '-R=anchor apple generic and certificate leaf = H"'
                        + signer.IDENTITY
                    )
                    for arg in call
                )
                for call in self.backend.calls
            ),
            3,
        )

    def testWrongManifestRefusesBeforeAnyNativeCommand(self):
        with self.assertRaises(signer.Refused):
            self.sign("0" * 64)
        self.assertEqual(self.backend.calls, [])
        self.assertFalse(self.output.exists())

    def testMalformedDigestRefuses(self):
        for value in (
            "",
            "sha256/" + self.digest,
            self.digest.upper(),
            "g" * 64,
            "0" * 63,
        ):
            with self.assertRaises(signer.Refused):
                self.sign(value)
        self.assertEqual(self.backend.calls, [])

    def testInputVersionMismatchRefuses(self):
        self.change_info(Path("."), "CFBundleVersion", "3")
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testMissingCapabilityRefuses(self):
        self.change_info(Path("."), "TinylandRoomPreparationProtocol", "old")
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testWrongAUIdentityRefuses(self):
        self.change_info(
            Path("Contents/PlugIns/DCXControlAU.appex"), "CFBundleIdentifier", "other"
        )
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testProvisioningProfileRefuses(self):
        (self.source / "Contents/embedded.provisionprofile").write_bytes(
            b"SYNTHETIC unexpected profile"
        )
        self.digest = signer.manifest(self.source)[1]
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testDuplicateIdentityRefusesBeforeCopy(self):
        self.backend.duplicate_identity = True
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertFalse(self.output.exists())

    def testWrongArchitectureRefusesBeforeCopy(self):
        self.backend.architecture = "x86_64"
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertFalse(self.output.exists())

    def testFirstKeyUseFailureRetainsPartialOutputAndImmutableInput(self):
        self.backend.failure_at = 0
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertTrue(self.output.is_dir())
        self.assertEqual(signer.manifest(self.source)[1], self.digest)
        self.assertEqual(
            len([call for call in self.backend.calls if "--force" in call]), 1
        )

    def testExtraSignedEntitlementRefusesBeforeAUSigning(self):
        self.backend.extra_entitlement = True
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(len(self.backend.signed), 1)
        self.assertEqual(signer.manifest(self.source)[1], self.digest)

    def testWrongSignedTeamRefusesBeforeAUSigning(self):
        self.backend.wrong_team = True
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(len(self.backend.signed), 1)

    def testHardlinkedCopyRefusesBeforeSigning(self):
        self.backend.hardlink_copy = True
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(len(self.backend.signed), 0)
        self.assertEqual(signer.manifest(self.source)[1], self.digest)

    def testExistingOutputRefuses(self):
        self.output.mkdir()
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testOutputCreatedDuringPreflightRefusesBeforeCopyOrSign(self):
        self.backend.create_output_during_identity = self.output
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertTrue(self.output.is_dir())
        self.assertFalse(
            any(
                Path(call[0]).name == "ditto" or "--force" in call
                for call in self.backend.calls
            )
        )
        self.assertEqual(signer.manifest(self.source)[1], self.digest)

    def testReplacedReservedOutputRefusesBeforeSigning(self):
        self.backend.replace_output_after_copy = True
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertFalse(any("--force" in call for call in self.backend.calls))
        self.assertEqual(signer.manifest(self.source)[1], self.digest)

    def testAppSuffixOutputRefuses(self):
        self.output = self.root / "output.app"
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testDifferentOutputParentRefuses(self):
        parent = self.root / "other"
        parent.mkdir()
        self.output = parent / "output.bundle"
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testUnprivateParentRefuses(self):
        self.root.chmod(0o755)
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testExternalSymlinkRefuses(self):
        (self.source / "external").symlink_to(self.home)
        with self.assertRaises(signer.Refused):
            self.sign()
        self.assertEqual(self.backend.calls, [])

    def testSymlinkSourceRefuses(self):
        link = self.root / "alias.bundle"
        link.symlink_to(self.source)
        with self.assertRaises(signer.Refused):
            signer.validate_paths(link, self.output)

    def testManifestMatchesIndependentLabAlgorithm(self):
        entries = []
        for path in sorted(self.source.rglob("*")):
            if path.is_file():
                entries.append(
                    {
                        "path": str(path.relative_to(self.source)),
                        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                    }
                )
        expected = hashlib.sha256(
            json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.assertEqual(signer.manifest(self.source), (entries, expected))

    def testGeneratedByteChangesAlwaysInvalidateBaselineBeforeCommands(self):
        generator = random.Random(6964)
        files = [path for path in self.source.rglob("*") if path.is_file()]
        for _ in range(64):
            path = generator.choice(files)
            original = path.read_bytes()
            payload = bytes(
                generator.randrange(256) for _ in range(generator.randrange(1, 128))
            )
            path.write_bytes(original + payload)
            with self.assertRaises(signer.Refused):
                self.sign()
            self.assertEqual(self.backend.calls, [])
            path.write_bytes(original)
            self.assertEqual(signer.manifest(self.source)[1], self.digest)

    def testEntitlementDictionariesMatchCommittedProductFiles(self):
        apple = SIGNER.parent.parent
        paths = {
            "dcxctl": "DCXLogicHelper/dcxctl.entitlements",
            "AU": "DCXControlAU/DCXControlAU.entitlements",
            "helper": "DCXLogicHelper/DCXLogicHelper.entitlements",
        }
        for name, _, _, _, expected in signer.COMPONENTS:
            self.assertEqual(
                plistlib.loads((apple / paths[name]).read_bytes()), expected
            )

    def testNonDarwinFrontdoorRefusesWithoutExecutingNativeTools(self):
        if sys.platform == "darwin":
            self.skipTest(
                "Linux refusal check only. Native command behavior is injected above."
            )
        result = subprocess.run(
            [
                sys.executable,
                str(SIGNER),
                str(self.source),
                str(self.output),
                self.digest,
            ],
            capture_output=True,
            timeout=5,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"ordinary Darwin user required", result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
