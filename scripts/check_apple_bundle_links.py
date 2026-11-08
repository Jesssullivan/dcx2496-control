#!/usr/bin/env python3
"""TIN-6964: admit only Apple-system or contained-bundle Mach-O linkage."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import time

MAGIC = {bytes.fromhex(x) for x in (
    "feedface", "cefaedfe", "feedfacf", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca")}
DYLIB = {"LC_LOAD_DYLIB", "LC_LOAD_WEAK_DYLIB", "LC_REEXPORT_DYLIB", "LC_LOAD_UPWARD_DYLIB", "LC_LAZY_LOAD_DYLIB"}
OTHER = {"LC_SEGMENT", "LC_SEGMENT_64", "LC_SYMTAB", "LC_DYSYMTAB", "LC_DYLD_INFO", "LC_DYLD_INFO_ONLY",
         "LC_UUID", "LC_VERSION_MIN_MACOSX", "LC_BUILD_VERSION", "LC_MAIN", "LC_SOURCE_VERSION",
         "LC_FUNCTION_STARTS", "LC_DATA_IN_CODE", "LC_CODE_SIGNATURE", "LC_LINKER_OPTIMIZATION_HINT",
         "LC_NOTE", "LC_DYLD_EXPORTS_TRIE", "LC_DYLD_CHAINED_FIXUPS", "LC_TWOLEVEL_HINTS",
         "LC_UNIXTHREAD", "LC_THREAD", "LC_PREBIND_CKSUM", "LC_ROUTINES", "LC_ROUTINES_64",
         "LC_SEGMENT_SPLIT_INFO", "LC_LINKER_OPTION", "LC_ATOM_INFO"}
SYSTEM_RPATHS = {"/usr/lib", "/usr/lib/swift", "/System/Library/Frameworks", "/System/Library/PrivateFrameworks"}
BUILD_PARTS = {"build", "target", ".build", "DerivedData", "CMakeFiles"}


class Refused(Exception):
    """A malformed or nonportable candidate, never repaired by this checker."""


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def clean(value):
    require(bool(value) and all(ord(x) >= 32 and ord(x) != 127 for x in value), "malformed linkage path")
    parts = value.split("/")
    require(not value.endswith("/") and "//" not in value, "malformed linkage components")
    require("/nix/store" not in value and "/nix/var/nix" not in value, "Nix runtime reference")
    require(not any(x in BUILD_PARTS or x.startswith(".derived") for x in parts), "build-tree runtime reference")


def apple(value):
    return value.startswith(("/usr/lib/", "/System/Library/"))


def normalize_architecture(cpu, subtype, label):
    aliases = {
        ("ARM64", "ALL"): "arm64", ("ARM64", "ARM64_ALL"): "arm64",
        ("ARM64", "ARM64E"): "arm64e", ("ARM64", "V8"): "arm64v8",
        ("ARM64", "ARM64_V8"): "arm64v8", ("X86_64", "ALL"): "x86_64",
        ("X86_64", "X86_64_ALL"): "x86_64", ("X86_64", "X86_64_H"): "x86_64h",
        ("X86_64", "H"): "x86_64h", ("I386", "ALL"): "i386",
    }
    result = aliases.get((cpu.upper(), subtype.upper()))
    require(result is not None, "unknown Mach-O CPU/subtype")
    require(label is None or label.lower() == result, "architecture label differs from Mach header")
    return result


def parse_otool(text, image):
    """Parse -h -v -l, requiring complete declared load-command coverage."""
    require(isinstance(text, str) and len(text.encode()) <= 2 * 1024 * 1024, "otool output exceeds bound")
    lines = [x.strip() for x in text.splitlines() if x.strip()]
    header = re.compile(re.escape(str(image)) + r"(?: \(architecture ([A-Za-z0-9_]+)\))?:$")
    starts = [i for i, line in enumerate(lines) if header.fullmatch(line)]
    require(starts and starts[0] == 0 and len(starts) <= 8, "missing or malformed Mach-O image header")
    result = []
    for section, stop in zip(starts, starts[1:] + [len(lines)]):
        chunk = lines[section:stop]
        require(len(chunk) >= 4 and chunk[1] == "Mach header", "missing Mach header")
        columns = chunk[2].split()
        require(columns[:7] == ["magic", "cputype", "cpusubtype", "caps", "filetype", "ncmds", "sizeofcmds"], "malformed Mach header columns")
        values = chunk[3].split()
        require(len(values) >= 7 and values[0] in {"MH_MAGIC", "MH_MAGIC_64", "MH_CIGAM", "MH_CIGAM_64"}, "invalid Mach header values")
        require(values[4] in {"EXECUTE", "DYLIB", "BUNDLE"}, "unsupported shipping Mach-O type")
        try:
            count, size = int(values[5]), int(values[6])
        except ValueError as error:
            raise Refused("invalid load-command counts") from error
        require(0 < count <= 4096 and 8 * count <= size <= 2 * 1024 * 1024, "load-command bounds differ")
        boundaries = [i for i in range(4, len(chunk)) if re.fullmatch(r"Load command [0-9]+", chunk[i])]
        require(len(boundaries) == count and boundaries[0] == 4, "incomplete declared load commands")
        loads, rpaths, identities, total_size = [], [], [], 0
        for number, (begin, end) in enumerate(zip(boundaries, boundaries[1:] + [len(chunk)])):
            block = chunk[begin:end]
            require(block[0] == f"Load command {number}", "unordered load command")
            commands = [x for x in block[1:] if x == "cmd" or x.startswith("cmd ")]
            sizes = [x for x in block[1:] if x == "cmdsize" or x.startswith("cmdsize ")]
            require(len(commands) == len(sizes) == 1, "ambiguous load command")
            require(re.fullmatch(r"cmd \S+", commands[0]) and re.fullmatch(r"cmdsize [0-9]+", sizes[0]),
                    "malformed load-command fields")
            command, command_size = commands[0].split()[1], int(sizes[0].split()[1])
            require(command_size >= 8 and command_size % 4 == 0, "invalid load-command size")
            total_size += command_size
            if command in DYLIB | {"LC_ID_DYLIB", "LC_LOAD_DYLINKER", "LC_ID_DYLINKER", "LC_RPATH"}:
                field, offset = ("path", 12) if command == "LC_RPATH" else ("name", 12 if "DYLINKER" in command else 24)
                names = [line for line in block[1:] if line == field or line.startswith(field + " ")]
                require(len(names) == 1, "missing or ambiguous linkage field")
                match = re.fullmatch(field + r" (.+) \(offset " + str(offset) + r"\)", names[0])
                require(match is not None, "malformed linkage field")
                name = match.group(1)
                clean(name)
                require(command_size >= offset + len(name.encode()) + 1, "linkage field exceeds command")
                if command == "LC_RPATH":
                    rpaths.append(name)
                elif command in {"LC_ID_DYLIB", "LC_ID_DYLINKER"}:
                    identities.append(name)
                else:
                    loads.append(name)
            else:
                require(command in OTHER, "unknown or unsafe load command: " + command)
        require(total_size == size, "declared load-command size differs")
        result.append({"architecture": normalize_architecture(values[1], values[2], header.fullmatch(chunk[0]).group(1)),
                       "filetype": values[4], "loads": loads, "rpaths": rpaths, "identities": identities})
    require(len({x["architecture"] for x in result}) == len(result), "duplicate architecture metadata")
    require(len({x["filetype"] for x in result}) == 1, "inconsistent Mach-O slice types")
    return result


class BundleChecker:
    def __init__(self, bundle, run_tool=None):
        original = Path(bundle)
        require(not original.is_symlink() and original.is_dir(), "bundle must be a real directory")
        self.root = original.resolve(strict=True)
        self.deadline = time.monotonic() + 60
        self.run_tool = run_tool or self.otool
        self.images = {}
        self.files = {}
        self.links = {}
        self.edges = []
        self.visited = set()
        self.total_bytes = 0

    def bounded(self):
        require(time.monotonic() < self.deadline, "bundle linkage deadline exceeded")

    def contained(self, path, exists=False):
        try:
            resolved = path.resolve(strict=exists)
        except (OSError, RuntimeError) as error:
            raise Refused("unresolved bundle path") from error
        require(resolved == self.root or self.root in resolved.parents, "path escapes bundle")
        return resolved

    def otool(self, image):
        self.bounded()
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            child = subprocess.Popen(["/usr/bin/otool", "-h", "-v", "-l", str(image)], stdout=output, stderr=errors)
            try:
                status = child.wait(timeout=min(5, max(0.01, self.deadline - time.monotonic())))
            except subprocess.TimeoutExpired as error:
                child.kill()
                child.wait(timeout=5)
                raise Refused("owned otool command timed out") from error
            require(status == 0, "otool refused a Mach-O image")
            require(output.tell() <= 2 * 1024 * 1024 and errors.tell() <= 64 * 1024, "otool metadata exceeds bound")
            output.seek(0)
            try:
                return output.read().decode("utf-8")
            except UnicodeDecodeError as error:
                raise Refused("invalid otool text") from error

    def inventory(self):
        pending, entries = [(self.root, 0)], 0
        while pending:
            directory, depth = pending.pop()
            require(depth <= 64, "bundle directory depth exceeds bound")
            for item in sorted(os.scandir(directory), key=lambda x: x.name):
                self.bounded()
                entries += 1
                require(entries <= 16384, "bundle entry count exceeds bound")
                require(all(ord(x) >= 32 and ord(x) != 127 for x in item.name), "control character in bundle path")
                path = Path(item.path)
                info = path.lstat()
                if stat.S_ISLNK(info.st_mode):
                    resolved = self.contained(path, exists=True)
                    self.links[path] = (info.st_dev, info.st_ino, os.readlink(path), resolved)
                elif stat.S_ISDIR(info.st_mode):
                    pending.append((path, depth + 1))
                elif stat.S_ISREG(info.st_mode):
                    with path.open("rb") as stream:
                        magic = stream.read(4)
                    if magic in MAGIC:
                        require(len(self.images) < 512 and info.st_size <= 256 * 1024 * 1024, "Mach-O size/count exceeds bound")
                        self.total_bytes += info.st_size
                        require(self.total_bytes <= 512 * 1024 * 1024, "total Mach-O bytes exceed bound")
                        digest = hashlib.sha256(path.read_bytes()).hexdigest()
                        self.files[path] = (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, digest)
                        self.images[path] = {"sha256": digest, "architectures": parse_otool(self.run_tool(path), path)}
                else:
                    raise Refused("unsupported bundle filesystem object")
        require(self.images, "bundle contains no Mach-O images")

    @staticmethod
    def suffix(value, prefix, parent_allowed):
        require(value.startswith(prefix + "/"), "unknown relative linkage token")
        tail = value[len(prefix) + 1:]
        parts = tail.split("/")
        require(all(x and x != "." and (parent_allowed or x != "..") for x in parts), "malformed token path")
        return tail

    def rpath(self, value, image, executable):
        clean(value)
        if value.startswith("/"):
            require(value in SYSTEM_RPATHS, "unknown absolute RPATH")
            return value
        if value.startswith("@loader_path/"):
            path = image.parent / self.suffix(value, "@loader_path", True)
        elif value.startswith("@executable_path/"):
            path = executable / self.suffix(value, "@executable_path", True)
        else:
            raise Refused("unknown RPATH token")
        return str(self.contained(path))

    def dependency(self, value, image, executable, rpaths):
        clean(value)
        if value.startswith("/"):
            require(apple(value) and all(x not in {".", ".."} for x in value.split("/")), "unknown absolute dependency")
            return value
        if value.startswith("@loader_path/"):
            path = image.parent / self.suffix(value, "@loader_path", True)
        elif value.startswith("@executable_path/"):
            path = executable / self.suffix(value, "@executable_path", True)
        elif value.startswith("@rpath/"):
            tail = self.suffix(value, "@rpath", False)
            for root in rpaths:
                if root in SYSTEM_RPATHS:
                    return root + "/" + tail
                path = self.contained(Path(root) / tail)
                if path.is_file():
                    return self.require_image(path)
            raise Refused("unresolved @rpath dependency")
        else:
            raise Refused("unknown relative dependency")
        return self.require_image(self.contained(path, exists=True))

    def require_image(self, path):
        path = self.contained(path, exists=True)
        require(path in self.images, "bundled dependency is not an admitted Mach-O image")
        return str(path)

    def visit(self, image, executable, inherited=(), requested_architecture=None):
        self.bounded()
        slices = self.images[image]["architectures"]
        require(requested_architecture is None or any(x["architecture"] == requested_architecture for x in slices),
                "contained dependency lacks requesting architecture")
        for metadata in slices:
            if requested_architecture is not None and metadata["architecture"] != requested_architecture:
                continue
            rpaths = tuple(dict.fromkeys([self.rpath(x, image, executable) for x in metadata["rpaths"]] + list(inherited)))
            require(len(rpaths) <= 128, "runpath chain exceeds bound")
            key = (image, executable, metadata["architecture"], rpaths)
            if key in self.visited:
                continue
            require(len(self.visited) < 4096, "dependency traversal exceeds bound")
            self.visited.add(key)
            for identity in metadata["identities"]:
                if identity.startswith("@rpath/"):
                    require(Path(self.suffix(identity, "@rpath", False)).name == image.name, "dylib identity differs from image")
                else:
                    resolved = self.dependency(identity, image, executable, rpaths)
                    require(apple(resolved) or Path(resolved) == image, "dylib identity differs from image")
            for value in metadata["loads"]:
                resolved = self.dependency(value, image, executable, rpaths)
                self.edges.append({"image": str(image.relative_to(self.root)), "architecture": metadata["architecture"],
                                   "reference": value, "resolved": resolved, "scope": "apple-system" if apple(resolved) else "contained-bundle"})
                if not apple(resolved):
                    self.visit(Path(resolved), executable, rpaths, metadata["architecture"])

    def check(self):
        self.inventory()
        executables = sorted(
            [p for p, x in self.images.items() if any(a["filetype"] == "EXECUTE" for a in x["architectures"])],
            key=lambda p: (len(p.relative_to(self.root).parts), str(p)),
        )
        require(executables, "bundle has no executable context")
        for image in executables:
            self.visit(image, image.parent)
        for image, value in self.images.items():
            for metadata in value["architectures"]:
                if not any(key[0] == image and key[2] == metadata["architecture"] for key in self.visited):
                    self.visit(image, executables[0].parent, requested_architecture=metadata["architecture"])
        for path, expected in self.links.items():
            self.bounded()
            info = path.lstat()
            require(stat.S_ISLNK(info.st_mode), "bundle symlink changed during inspection")
            observed = (info.st_dev, info.st_ino, os.readlink(path), self.contained(path, exists=True))
            require(observed == expected, "bundle symlink changed during inspection")
        for image, expected in self.files.items():
            self.bounded()
            require(self.contained(image, exists=True) == image, "Mach-O image path changed during inspection")
            info = image.lstat()
            observed = (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, hashlib.sha256(image.read_bytes()).hexdigest())
            require(observed == expected, "Mach-O image changed during inspection")
        return {"schemaVersion": "dcx.apple-bundle-linkage/v1", "status": "pass", "bundle": str(self.root),
                "policy": "apple-system-or-strictly-contained-bundle", "images": {
                    str(p.relative_to(self.root)): value for p, value in self.images.items()}, "dependencies": self.edges,
                "scope": "Portable linkage metadata only. No app execution or OS-library-presence proof."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args()
    try:
        result = BundleChecker(args.bundle).check()
    except (Refused, OSError, ValueError) as error:
        print(json.dumps({"schemaVersion": "dcx.apple-bundle-linkage/v1", "status": "refused", "reason": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
