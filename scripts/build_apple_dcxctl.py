#!/usr/bin/env python3
"""TIN-6964: embed dcxctl using explicit Apple tools and a closed environment."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def build_environment(*, home, cargo_home, temporary, target, rustc, clang, sdk, developer, jobs):
    """Ambient Rust, Nix, compiler and dynamic-loader flags never enter Cargo."""
    separator = chr(31)
    flags = separator.join(['-C', 'linker=' + str(clang), '-C', 'link-arg=-isysroot', '-C', 'link-arg=' + str(sdk)])
    return {
        'HOME': str(home), 'CARGO_HOME': str(cargo_home), 'TMPDIR': str(temporary),
        'PATH': str(clang.parent) + ':/usr/bin:/bin:/usr/sbin:/sbin',
        'LANG': 'en_US.UTF-8', 'LC_ALL': 'en_US.UTF-8',
        'DEVELOPER_DIR': str(developer), 'SDKROOT': str(sdk),
        'MACOSX_DEPLOYMENT_TARGET': '14.0', 'CARGO_BUILD_JOBS': str(jobs),
        'CARGO_TARGET_DIR': str(target), 'RUSTC': str(rustc),
        'CC': str(clang), 'CXX': str(clang.parent / 'clang++'),
        'CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER': str(clang),
        'CARGO_ENCODED_RUSTFLAGS': flags,
    }


def reject_cargo_configuration(source, cargo_home):
    # Hidden Cargo configuration could override the reviewed linker contract.
    for directory in [cargo_home] + [p / '.cargo' for p in [source, *source.parents]]:
        if any((directory / name).exists() for name in ['config', 'config.toml']):
            raise ValueError('ambient Cargo configuration refused: ' + str(directory))


def tool_digest(path):
    if path.stat().st_size > 512 * 1024 * 1024:
        raise ValueError('compiler tool exceeds bounded metadata limit')
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'target', 'cargo', 'rustc', 'developer']:
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--jobs', type=int, choices=[1, 2], default=1)
    args = parser.parse_args()
    if sys.platform != 'darwin' or os.uname().machine != 'arm64':
        parser.error('native arm64 Darwin build required')
    source, cargo, rustc, developer = [p.resolve(strict=True) for p in [args.source, args.cargo, args.rustc, args.developer]]
    if not str(developer).startswith('/Applications/') or not str(developer).endswith('.app/Contents/Developer'):
        parser.error('selected Xcode developer directory required')
    clang = developer / 'Toolchains/XcodeDefault.xctoolchain/usr/bin/clang'
    if not all(p.is_file() and os.access(p, os.X_OK) for p in [cargo, rustc, clang, clang.parent / 'clang++']):
        parser.error('exact compiler tool missing')
    home = Path(os.environ['HOME']).resolve(strict=True)
    cargo_home = Path(os.environ.get('CARGO_HOME', str(home / '.cargo'))).resolve(strict=True)
    temporary = Path(os.environ['TMPDIR']).resolve(strict=True)
    reject_cargo_configuration(source, cargo_home)
    sdk = Path(subprocess.check_output(['/usr/bin/xcrun', '--sdk', 'macosx', '--show-sdk-path'],
        env={'PATH': '/usr/bin:/bin', 'DEVELOPER_DIR': str(developer)}, text=True, timeout=15).strip()).resolve(strict=True)
    if developer not in sdk.parents:
        parser.error('SDK must belong to selected Xcode')
    target = args.target.resolve()
    target.mkdir(parents=True, exist_ok=True)
    environment = build_environment(home=home, cargo_home=cargo_home, temporary=temporary,
        target=target, rustc=rustc, clang=clang, sdk=sdk, developer=developer, jobs=args.jobs)
    metadata = {'schemaVersion': 'dcx.apple-cli-link-inputs/v1',
        'tools': {str(p): tool_digest(p) for p in [cargo, rustc, clang]},
        'sdk': str(sdk), 'environment': environment}
    (target / 'apple-cli-link-inputs.json').write_text(json.dumps(metadata, sort_keys=True, indent=2) + '\n')
    subprocess.run([str(cargo), 'build', '--manifest-path', str(source / 'Cargo.toml'),
        '--frozen', '--release', '--package', 'dcxctl', '--features', 'live-control'],
        cwd=source, env=environment, check=True)


if __name__ == '__main__':
    main()
