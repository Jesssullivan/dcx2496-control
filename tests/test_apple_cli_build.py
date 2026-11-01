"""TIN-6964: source-only tests. No compiler, Apple tool or device is invoked."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('apple_cli_build', Path(__file__).parents[1] / 'scripts/build_apple_dcxctl.py')
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class AppleCLIEnvironmentTests(unittest.TestCase):
    def environment(self, **overrides):
        values = dict(home=Path('/owned/home'), cargo_home=Path('/owned/cargo'),
            temporary=Path('/owned/tmp'), target=Path('/owned/target'), rustc=Path('/pinned/rustc'),
            clang=Path('/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang'),
            sdk=Path('/Applications/Xcode.app/SDKs/MacOSX.sdk'), developer=Path('/Applications/Xcode.app/Contents/Developer'), jobs=2)
        values.update(overrides)
        return build.build_environment(**values)

    def testPoisonedAmbientLinkerAndLoaderFlagsNeverReachCargo(self):
        poison = {key: '/nix/store/SYNTHETIC-poison' for key in [
            'NIX_LDFLAGS', 'NIX_CFLAGS_COMPILE', 'RUSTFLAGS', 'RUSTC_WRAPPER',
            'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTFLAGS', 'CARGO_BUILD_TARGET',
            'LIBRARY_PATH', 'CPATH', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS',
            'DYLD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES', 'BASH_ENV']}
        poison['CARGO_ENCODED_RUSTFLAGS'] = 'poison'
        with patch.dict(os.environ, poison):
            result = self.environment()
        for key in poison:
            if key == 'CARGO_ENCODED_RUSTFLAGS':
                self.assertNotIn('poison', result[key])
            else:
                self.assertNotIn(key, result)
        self.assertNotIn('/nix/store', result['PATH'])

    def testEncodedLinkerAndSDKArgumentsKeepSpacesAsSingleArguments(self):
        sdk = Path('/Applications/Xcode Test.app/SDKs/MacOSX.sdk')
        result = self.environment(sdk=sdk)
        self.assertEqual(result['CARGO_ENCODED_RUSTFLAGS'].split(chr(31)), [
            '-C', 'linker=' + result['CC'], '-C', 'link-arg=-isysroot', '-C', 'link-arg=' + str(sdk)])
        self.assertEqual(result['CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER'], result['CC'])
        self.assertEqual(result['SDKROOT'], str(sdk))

    def testTargetRustcAndDeploymentAreExplicit(self):
        result = self.environment()
        self.assertEqual(result['RUSTC'], '/pinned/rustc')
        self.assertEqual(result['MACOSX_DEPLOYMENT_TARGET'], '14.0')
        self.assertEqual(result['CARGO_TARGET_DIR'], '/owned/target')
        self.assertEqual(result['CARGO_BUILD_JOBS'], '2')

    def testCargoHomeConfigurationCannotOverrideLinker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            cargo = root / 'cargo'
            source.mkdir(); cargo.mkdir()
            build.reject_cargo_configuration(source, cargo)
            for name in ['config', 'config.toml']:
                config = cargo / name
                config.write_text('[build]\nrustflags=["-L/nix/store/poison"]\n')
                with self.assertRaises(ValueError):
                    build.reject_cargo_configuration(source, cargo)
                config.unlink()

    def testSourceAncestorConfigurationCannotOverrideLinker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            cargo = root / 'cargo'
            source.mkdir(); cargo.mkdir()
            config = root / '.cargo'
            config.mkdir()
            (config / 'config.toml').write_text('[target.aarch64-apple-darwin]\nlinker="poison"\n')
            with self.assertRaises(ValueError):
                build.reject_cargo_configuration(source, cargo)


if __name__ == '__main__':
    unittest.main()
