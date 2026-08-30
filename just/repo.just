set working-directory := ".."

fmt:
    cargo fmt --all
    alejandra flake.nix
    rg --files -0 -g 'BUILD.bazel' -g '*.bzl' -g 'MODULE.bazel' | xargs -0 buildifier -mode=fix

fmt-check:
    cargo fmt --all --check
    alejandra --check flake.nix
    rg --files -0 -g 'BUILD.bazel' -g '*.bzl' -g 'MODULE.bazel' | xargs -0 buildifier -mode=check

lint:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    shellcheck tests/*.sh
    workflow_files=()
    while IFS= read -r workflow_file; do
        workflow_files+=("$workflow_file")
    done < <(rg --files .github/workflows -g '*.yml' -g '*.yaml' 2>/dev/null || true)
    if ((${#workflow_files[@]})); then
        actionlint "${workflow_files[@]}"
    fi

test:
    cargo test --workspace --all-targets --all-features

# Exercise the injected, no-device discovery/snapshot/apply transport contract.
transport-check:
    cargo test --locked --package dcx-transport --all-targets --all-features

# Exercise the Darwin carrier only through its injected fake syscall boundary.
darwin-carrier-check:
    cargo test --locked --package dcx-darwin-tty --all-targets --all-features

fixtures:
    cargo run --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-direct-parameter.hex >/dev/null
    cargo run --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-search-response-26.hex >/dev/null
    cargo run --quiet --package dcxctl -- discovery plan --expected-device 0 >/dev/null
    cargo run --quiet --package dcxctl -- discovery validate-response fixtures/protocol/SYNTHETIC-search-response-26.hex --expected-device 0 >/dev/null
    cargo run --quiet --package dcxctl -- profile validate fixtures/profiles/safe-muted-v1.json >/dev/null
    cargo run --quiet --package dcxctl -- profile diff fixtures/profiles/safe-muted-v1.json fixtures/profiles/SYNTHETIC-control-room-v1.json >/dev/null
    cargo run --quiet --package dcxctl -- rew import fixtures/rew/SYNTHETIC-cut-only.txt --target-output 3 >/dev/null
    ! cargo run --quiet --package dcxctl -- profile apply-ready fixtures/profiles/safe-muted-v1.json --profile-digest aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa >/dev/null 2>&1
    ! cargo run --quiet --package dcxctl -- rew import fixtures/rew/SYNTHETIC-cut-only.txt --target-output 3 --allow-boost >/dev/null 2>&1

lockfiles:
    test -s Cargo.lock
    test -s MODULE.bazel.lock
    test -s flake.lock
    git ls-files --error-unmatch Cargo.lock MODULE.bazel.lock flake.lock >/dev/null
    cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null

check: fmt-check lint test fixtures lockfiles

bazel-check:
    #!/usr/bin/env bash
    set -euo pipefail
    repo_env=()
    build_targets=(//:dcxctl //crates/dcx-darwin-tty:dcx_darwin_tty)
    if [[ $(uname -s) == Darwin ]]; then
        repo_env=(--repo_env=CC=/usr/bin/clang --repo_env=CXX=/usr/bin/clang++)
        build_targets+=(//:dcxctl_live_control)
    fi
    bazelisk test "${repo_env[@]}" //:check
    bazelisk build "${repo_env[@]}" "${build_targets[@]}"

nix-check:
    nix flake check

# Materialize the aarch64-darwin live control binary without profile activation.
live-package:
    nix build .#dcxctl-live --no-link

# Compile the shared Swift bridge/helper/AU libraries without launching a GUI.
apple-package-check:
    swift build --package-path apple --jobs 1

# Parse the checked-in bridge schema without introducing another validator stack.
schema-check:
    jq -e . schemas/v1/dcx_logic_bridge.schema.json >/dev/null

# Regenerate the ignored Xcode project from the checked-in XcodeGen source.
apple-project-generate:
    xcodegen generate --spec apple/project.yml

# Build the helper, embedded AUv3, and embedded dcxctl without signing or install.
apple-bundle-check: apple-project-generate
    xcodebuild -project apple/DCXLogic.xcodeproj -scheme DCXLogicHelper -configuration Debug -derivedDataPath apple/.derived -jobs 1 ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO CC="$(/usr/bin/xcrun --find clang)" CPLUSPLUS="$(/usr/bin/xcrun --find clang++)" LD="$(/usr/bin/xcrun --find clang)" LDPLUSPLUS="$(/usr/bin/xcrun --find clang++)" build

# Full product-source compile without signing, install, GUI launch, or hardware I/O.
product-check: schema-check apple-bundle-check

security:
    cargo deny check
    gitleaks dir . --no-banner
    gitleaks git . --no-banner
