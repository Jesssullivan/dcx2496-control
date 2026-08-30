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
    # Resolve the compiler from Xcode itself: inside a Nix shell, xcrun can return a wrapped Nix clang.
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"; \
      xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"; \
      DEVELOPER_DIR="$xcode_developer_dir" "$xcode_developer_dir/usr/bin/xcodebuild" -project apple/DCXLogic.xcodeproj -scheme DCXLogicHelper -configuration Debug -derivedDataPath apple/.derived -jobs 1 ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO CC="$xcode_toolchain/clang" CPLUSPLUS="$xcode_toolchain/clang++" LD="$xcode_toolchain/clang" LDPLUSPLUS="$xcode_toolchain/clang++" build

# Build and verify the provisioned containing app, embedded AUv3, and dcxctl.
# This does not authorize provisioning updates, install/register, or launch a GUI.
apple-provisioned-bundle: apple-project-generate
    #!/usr/bin/env bash
    set -euo pipefail
    team_id="QP994XQKNH"
    app_group="QP994XQKNH.io.tinyland.dcx2496"
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
    xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"
    derived_data="apple/.derived-provisioned"
    DEVELOPER_DIR="$xcode_developer_dir" "$xcode_developer_dir/usr/bin/xcodebuild" \
      -project apple/DCXLogic.xcodeproj \
      -scheme DCXLogicHelper \
      -configuration Release \
      -destination 'generic/platform=macOS' \
      -derivedDataPath "$derived_data" \
      -jobs 1 \
      ARCHS=arm64 \
      ONLY_ACTIVE_ARCH=YES \
      CODE_SIGNING_ALLOWED=YES \
      CODE_SIGNING_REQUIRED=YES \
      CODE_SIGN_STYLE=Automatic \
      CODE_SIGN_IDENTITY="Apple Development" \
      DEVELOPMENT_TEAM="$team_id" \
      CC="$xcode_toolchain/clang" \
      CPLUSPLUS="$xcode_toolchain/clang++" \
      LD="$xcode_toolchain/clang" \
      LDPLUSPLUS="$xcode_toolchain/clang++" \
      build
    app="$derived_data/Build/Products/Release/DCXLogicHelper.app"
    au="$app/Contents/PlugIns/DCXControlAU.appex"
    child="$app/Contents/Helpers/dcxctl"
    app_binary="$app/Contents/MacOS/DCXLogicHelper"
    au_binary="$au/Contents/MacOS/DCXControlAU"
    test -d "$app"
    test -d "$au"
    for binary in "$app_binary" "$au_binary" "$child"; do
      test -x "$binary"
      test "$(/usr/bin/lipo -archs "$binary")" = arm64
    done
    for artifact in "$app" "$au" "$child"; do
      /usr/bin/codesign --verify --strict "$artifact"
      signature="$(/usr/bin/codesign -dvvv "$artifact" 2>&1)"
      observed_team="$(printf '%s\n' "$signature" | /usr/bin/awk -F= '$1 == "TeamIdentifier" {print $2}')"
      test "$observed_team" = "$team_id"
      printf '%s\n' "$signature" | /usr/bin/grep -q '^Authority=Apple Development:'
    done
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = io.tinyland.dcx2496.logic
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$au/Contents/Info.plist")" = io.tinyland.dcx2496.logic.midi
    app_entitlements="$(/usr/bin/codesign -d --entitlements :- "$app" 2>/dev/null)"
    au_entitlements="$(/usr/bin/codesign -d --entitlements :- "$au" 2>/dev/null)"
    child_entitlements="$(/usr/bin/codesign -d --entitlements :- "$child" 2>/dev/null)"
    test "$(printf '%s' "$app_entitlements" | /usr/bin/plutil -extract com.apple.security.app-sandbox raw -o - -)" = true
    test "$(printf '%s' "$app_entitlements" | /usr/bin/plutil -extract com.apple.security.device.serial raw -o - -)" = true
    printf '%s' "$app_entitlements" | /usr/bin/plutil -extract com.apple.security.application-groups json -o - - | /usr/bin/jq -e --arg group "$app_group" 'length == 1 and .[0] == $group' >/dev/null
    test "$(printf '%s' "$au_entitlements" | /usr/bin/plutil -extract com.apple.security.app-sandbox raw -o - -)" = true
    printf '%s' "$au_entitlements" | /usr/bin/plutil -extract com.apple.security.application-groups json -o - - | /usr/bin/jq -e --arg group "$app_group" 'length == 1 and .[0] == $group' >/dev/null
    ! printf '%s' "$au_entitlements" | /usr/bin/plutil -extract com.apple.security.device.serial raw -o - - >/dev/null 2>&1
    test "$(printf '%s' "$child_entitlements" | /usr/bin/plutil -extract com.apple.security.inherit raw -o - -)" = true
    verify_profile() {
      local profile="$1" expected_identifier="$2" decoded groups
      test -f "$profile"
      decoded="$(/usr/bin/security cms -D -i "$profile")"
      test "$(printf '%s' "$decoded" | /usr/bin/plutil -extract Entitlements.application-identifier raw -o - -)" = "$team_id.$expected_identifier"
      test "$(printf '%s' "$decoded" | /usr/bin/plutil -extract Entitlements.com.apple.developer.team-identifier raw -o - -)" = "$team_id"
      groups="$(printf '%s' "$decoded" | /usr/bin/plutil -extract Entitlements.com.apple.security.application-groups json -o - -)"
      printf '%s' "$groups" | /usr/bin/jq -e --arg group "$app_group" 'length == 1 and .[0] == $group' >/dev/null
    }
    verify_profile "$app/Contents/embedded.provisionprofile" io.tinyland.dcx2496.logic
    verify_profile "$au/Contents/embedded.provisionprofile" io.tinyland.dcx2496.logic.midi
    info="$au/Contents/Info.plist"
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:type' "$info")" = aumi
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:subtype' "$info")" = DcxC
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:manufacturer' "$info")" = TnLd
    printf 'provisioned_bundle=ready\n'

# Full product-source compile without signing, install, GUI launch, or hardware I/O.
product-check: schema-check apple-bundle-check

security:
    cargo deny check
    gitleaks dir . --no-banner
    gitleaks git . --no-banner
