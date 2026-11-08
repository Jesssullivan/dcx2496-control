"""TIN-6964: portable-linkage admission with injected otool, no native process."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("bundle_links", Path(__file__).resolve().parents[1] / "scripts/check_apple_bundle_links.py")
links = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(links)


def output(image, loads=(), rpaths=(), kind="EXECUTE", identity=None, cpu="ARM64", subtype="ALL", architecture=None):
    commands = [("LC_LOAD_DYLIB", "name", x, 24) for x in loads]
    commands += [("LC_RPATH", "path", x, 12) for x in rpaths]
    if identity:
        commands.append(("LC_ID_DYLIB", "name", identity, 24))
    if not commands:
        commands = [("LC_UUID", None, None, None)]
    sizes = [24 if name is None else ((offset + len(name.encode()) + 4) // 4) * 4 for _, _, name, offset in commands]
    label = f"{image} (architecture {architecture})" if architecture else str(image)
    text = f"{label}:\nMach header\nmagic cputype cpusubtype caps filetype ncmds sizeofcmds flags\nMH_MAGIC_64 {cpu} {subtype} 0x00 {kind} {len(commands)} {sum(sizes)} PIE\n"
    for i, ((cmd, field, name, offset), size) in enumerate(zip(commands, sizes)):
        text += f"Load command {i}\n cmd {cmd}\n cmdsize {size}\n"
        if name is not None:
            text += f" {field} {name} (offset {offset})\n"
    return text


class BundleLinksTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "Candidate.app"
        self.root.mkdir()
        self.binary = self.make("Contents/MacOS/Helper")
        self.metadata = {self.binary: output(self.binary, ["/usr/lib/libSystem.B.dylib"])}
        self.called = []

    def make(self, relative):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(bytes.fromhex("cffaedfe") + bytes(64))
        return path

    def run_tool(self, path):
        self.called.append(path)
        return self.metadata[path]

    def check(self):
        return links.BundleChecker(self.root, self.run_tool).check()

    def refuse(self):
        with self.assertRaises(links.Refused):
            self.check()

    def test_system_cache_only_absolute_dependency_is_metadata_admitted(self):
        result = self.check()
        self.assertEqual(result["status"], "pass")
        self.assertEqual(result["dependencies"][0]["scope"], "apple-system")
        self.assertEqual(self.called, [self.binary])

    def test_declared_apple_swift_rpath_is_portable_system_leaf(self):
        self.metadata[self.binary] = output(self.binary, ["@rpath/libswiftCore.dylib"], ["/usr/lib/swift"])
        result = self.check()
        self.assertEqual(result["dependencies"][0]["resolved"], "/usr/lib/swift/libswiftCore.dylib")

    def test_incompatible_contained_architecture_refuses(self):
        library = self.make("Contents/Frameworks/Other.dylib")
        self.metadata[self.binary] = output(self.binary, ["@loader_path/../Frameworks/Other.dylib"])
        self.metadata[library] = output(library, ["/usr/lib/libSystem.B.dylib"], kind="DYLIB", cpu="X86_64")
        self.refuse()

    def test_thin_executable_accepts_matching_fat_slice_and_checks_all_slices(self):
        library = self.make("Contents/Frameworks/Fat.dylib")
        self.metadata[self.binary] = output(self.binary, ["@loader_path/../Frameworks/Fat.dylib"])
        arm = output(library, ["/usr/lib/libSystem.B.dylib"], kind="DYLIB", architecture="arm64")
        intel = output(library, ["/usr/lib/libSystem.B.dylib"], kind="DYLIB", cpu="X86_64", architecture="x86_64")
        self.metadata[library] = arm + intel
        result = self.check()
        slices = result["images"][str(library.relative_to(self.root))]["architectures"]
        self.assertEqual([x["architecture"] for x in slices], ["arm64", "x86_64"])
        self.metadata[library] = arm + output(library, ["/opt/homebrew/lib/bad.dylib"], kind="DYLIB", cpu="X86_64", architecture="x86_64")
        self.refuse()

    def test_universal_label_cannot_misrepresent_cpu_header(self):
        self.metadata[self.binary] = output(self.binary, ["/usr/lib/libSystem.B.dylib"], cpu="X86_64", architecture="arm64")
        self.refuse()

    def test_contained_framework_and_cycle_are_recursively_inspected(self):
        library = self.make("Contents/Frameworks/Good.dylib")
        self.metadata[self.binary] = output(self.binary, ["@rpath/Good.dylib"], ["@executable_path/../Frameworks"])
        self.metadata[library] = output(library, ["@loader_path/Good.dylib", "/usr/lib/libSystem.B.dylib"], kind="DYLIB", identity="@rpath/Good.dylib")
        result = self.check()
        self.assertEqual(set(self.called), {self.binary, library})
        self.assertEqual(len(result["images"]), 2)

    def test_unused_nix_or_build_rpaths_refuse(self):
        for path in ["/nix/store/abc/lib", "/private/nix/store/abc/lib", "/tmp/build/lib", "@loader_path/../target/lib", "/Applications/Xcode.app/Contents/Developer/usr/lib"]:
            with self.subTest(path=path):
                self.metadata[self.binary] = output(self.binary, ["/usr/lib/libSystem.B.dylib"], [path])
                self.refuse()

    def test_every_load_variant_and_identity_refuse_nix(self):
        for cmd in links.DYLIB | {"LC_ID_DYLIB"}:
            self.metadata[self.binary] = output(self.binary, ["/nix/store/abc/lib/libbad.dylib"]).replace("LC_LOAD_DYLIB", cmd)
            self.refuse()

    def test_unknown_absolute_relative_and_rpath_dependencies_refuse(self):
        for dependency in ["/opt/homebrew/lib/libbad.dylib", "libbad.dylib", "@rpath/libbad.dylib", "@rpath/../libbad.dylib", "/usr/lib/../../nix/store/bad"]:
            with self.subTest(dependency=dependency):
                self.metadata[self.binary] = output(self.binary, [dependency])
                self.refuse()

    def test_nested_bad_library_cannot_hide_behind_safe_root(self):
        library = self.make("Contents/Frameworks/Bad.dylib")
        self.metadata[self.binary] = output(self.binary, ["@loader_path/../Frameworks/Bad.dylib"])
        self.metadata[library] = output(library, ["/nix/store/abc/libbad.dylib"], kind="DYLIB")
        self.refuse()

    def test_symlink_escape_and_dangling_symlink_refuse_even_unused(self):
        for target in [str(Path(self.temp.name)), str(self.root / "missing")]:
            link = self.root / "unused"
            link.symlink_to(target)
            self.refuse()
            link.unlink()

    def test_internal_framework_symlink_is_allowed(self):
        target = self.root / "Contents/Resources/real"
        target.parent.mkdir(parents=True)
        target.write_text("resource")
        (target.parent / "alias").symlink_to("real")
        self.assertEqual(self.check()["status"], "pass")

    def test_non_macho_dependency_and_unresolved_loader_path_refuse(self):
        dependency = self.root / "Contents/Frameworks/Bad.dylib"
        dependency.parent.mkdir(parents=True)
        dependency.write_text("not Mach-O")
        self.metadata[self.binary] = output(self.binary, ["@loader_path/../Frameworks/Bad.dylib"])
        self.refuse()
        dependency.unlink()
        with self.assertRaises((links.Refused, OSError)):
            self.check()

    def test_magic_enumeration_finds_unexpected_extension(self):
        extra = self.make("Contents/Resources/not-a-library.txt")
        self.metadata[extra] = output(extra, ["/nix/store/abc/libbad.dylib"])
        self.refuse()
        self.assertIn(extra, self.called)

    def test_missing_unordered_duplicate_or_truncated_commands_refuse(self):
        good = self.metadata[self.binary]
        mutations = [good.replace("Load command 0", "Load command 1"), good.replace("ncmds sizeofcmds", "bad sizeofcmds"),
                     good.replace("EXECUTE 1", "EXECUTE 2"), good.replace(" cmdsize", " broken"),
                     good.replace(" cmd LC_LOAD_DYLIB", " cmd LC_LOAD_DYLIB\n cmd LC_RPATH"),
                     good.replace("LC_LOAD_DYLIB", "LC_DYLD_ENVIRONMENT"), good.replace(" (offset 24)", " (offset 12)"),
                     good.replace(" cmd LC_LOAD_DYLIB", " cmd bad extra\n cmd LC_LOAD_DYLIB"),
                     good.replace(" name /usr/lib", " name /nix/store/bad\n name /usr/lib")]
        for text in mutations:
            with self.subTest(text=text):
                self.metadata[self.binary] = text
                self.refuse()

    def test_changed_image_during_metadata_inspection_refuses(self):
        def changed(path):
            path.write_bytes(path.read_bytes() + b"changed")
            return self.metadata[path]
        with self.assertRaises(links.Refused):
            links.BundleChecker(self.root, changed).check()

    def test_symlink_changed_after_inventory_refuses(self):
        resource = self.root / "resource"
        resource.write_text("owned")
        link = self.root / "alias"
        link.symlink_to("resource")
        checker = links.BundleChecker(self.root, self.run_tool)
        original = checker.inventory
        def inventory_then_change():
            original()
            link.unlink()
            link.symlink_to(Path(self.temp.name))
        checker.inventory = inventory_then_change
        with self.assertRaises(links.Refused):
            checker.check()

    def test_malformed_absolute_or_token_components_refuse(self):
        for value in ["/usr/lib//libbad.dylib", "/usr/lib/libbad.dylib/", "@rpath//bad", "@loader_path/./bad"]:
            self.metadata[self.binary] = output(self.binary, [value])
            self.refuse()

    def test_uncontained_executable_token_and_unknown_rpath_refuse(self):
        for path in ["@executable_path/../../../outside", "relative/lib", "@rpath/other"]:
            self.metadata[self.binary] = output(self.binary, ["/usr/lib/libSystem.B.dylib"], [path])
            self.refuse()


if __name__ == "__main__":
    unittest.main()
