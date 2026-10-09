set working-directory := ".."

# Modules keep their own environment; the same bound reaches nested Cargo builds.
# Ruling: dec-local-first-reapi-20261004. CLI flags also bind compiling recipes.
export CARGO_BUILD_JOBS := "2"

fmt:
    cargo fmt --all
    rustfmt --edition 2024 tests/toolchain_pin_parity.rs
    alejandra flake.nix
    rg --files -0 -g 'BUILD.bazel' -g '*.bzl' -g 'MODULE.bazel' | xargs -0 buildifier -mode=fix

fmt-check:
    cargo fmt --all --check
    rustfmt --edition 2024 --check tests/toolchain_pin_parity.rs
    alejandra --check flake.nix
    rg --files -0 -g 'BUILD.bazel' -g '*.bzl' -g 'MODULE.bazel' | xargs -0 buildifier -mode=check

lint-static:
    #!/usr/bin/env bash
    set -euo pipefail
    shellcheck scripts/*.sh tests/*.sh
    workflow_files=()
    while IFS= read -r workflow_file; do
        workflow_files+=("$workflow_file")
    done < <(rg --files .github/workflows -g '*.yml' -g '*.yaml' 2>/dev/null || true)
    if ((${#workflow_files[@]})); then
        actionlint "${workflow_files[@]}"
    fi

lint: lint-static
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --locked --jobs 2 --workspace --all-targets --all-features -- -D warnings
    # tests/toolchain_pin_parity.rs belongs to no Cargo package -- the root
    # manifest is a virtual workspace -- so `cargo fmt --all` and
    # `cargo clippy --workspace` never reach it and only Bazel compiles it.
    # Lint it directly, under the levels [workspace.lints] sets, so the file
    # asserting the toolchain pins is not itself the one source nothing checks.
    parity_lint_out="$(mktemp -d)"
    trap 'rm -rf "$parity_lint_out"' EXIT
    clippy-driver --edition 2024 --test \
        --crate-name toolchain_pin_parity \
        --emit=metadata --out-dir "$parity_lint_out" \
        tests/toolchain_pin_parity.rs \
        -F unsafe_code -D warnings \
        -D clippy::all -D clippy::pedantic \
        -A clippy::module_name_repetitions -A clippy::must_use_candidate

test:
    cargo test --locked --jobs 2 --workspace --all-targets --all-features -- --test-threads=2

# dec-local-first-reapi-20261004: pinned Bazelisk, local unless explicitly REAPI.
test-unit:
    bash scripts/run_bazelisk.sh test //:remote_eligible

build:
    bash scripts/run_bazelisk.sh build //:dcxctl //crates/dcx-darwin-tty:dcx_darwin_tty

[script]
[positional-arguments]
bazel command *args:
    exec bash scripts/run_bazelisk.sh "$@"

[script]
[positional-arguments]
remote-build *targets="//:dcxctl":
    exec bash scripts/run_bazelisk.sh --reapi build "$@"

[script]
[positional-arguments]
remote-test *targets="//:remote_eligible":
    exec bash scripts/run_bazelisk.sh --reapi test "$@"

# Precondition refusals never launch Bazel. The test uses the host shell/tools.
frontdoor-check:
    bash tests/bazel_frontdoor_rejections.sh scripts/run_bazelisk.sh

# Exercise the injected, no-device discovery/snapshot/apply transport contract.
transport-check:
    cargo test --locked --jobs 2 --package dcx-transport --all-targets --all-features -- --test-threads=2

# Exercise the Darwin carrier only through its injected fake syscall boundary.
darwin-carrier-check:
    cargo test --locked --jobs 2 --package dcx-darwin-tty --all-targets --all-features -- --test-threads=2

fixtures:
    cargo run --locked --jobs 2 --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-direct-parameter.hex >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-o4-mute-on.hex >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-o4-mute-off.hex >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- decode --file fixtures/protocol/SYNTHETIC-search-response-26.hex >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- discovery plan --expected-device 0 >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- discovery validate-response fixtures/protocol/SYNTHETIC-search-response-26.hex --expected-device 0 >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- profile validate fixtures/profiles/safe-muted-v1.json >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- profile diff fixtures/profiles/safe-muted-v1.json fixtures/profiles/SYNTHETIC-control-room-v1.json >/dev/null
    cargo run --locked --jobs 2 --quiet --package dcxctl -- rew import fixtures/rew/SYNTHETIC-cut-only.txt --target-output 3 >/dev/null
    ! cargo run --locked --jobs 2 --quiet --package dcxctl -- profile apply-ready fixtures/profiles/safe-muted-v1.json --profile-digest aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa >/dev/null 2>&1
    ! cargo run --locked --jobs 2 --quiet --package dcxctl -- rew import fixtures/rew/SYNTHETIC-cut-only.txt --target-output 3 --allow-boost >/dev/null 2>&1
    cargo build --locked --jobs 2 --quiet --package dcxctl
    bash tests/feedback_pipeline.sh "${CARGO_TARGET_DIR:-target}/debug/dcxctl" fixtures/feedback/SYNTHETIC-ring-out.txt fixtures/rew/SYNTHETIC-feedback-notches.txt fixtures/snapshots/SYNTHETIC-blank-snapshot.json
    bash tests/routing_pipeline.sh "${CARGO_TARGET_DIR:-target}/debug/dcxctl" fixtures/snapshots/SYNTHETIC-blank-snapshot.json
    bash tests/apple_feedback_fixture_parity.sh "${CARGO_TARGET_DIR:-target}/debug/dcxctl" fixtures/feedback/SYNTHETIC-ring-out.txt fixtures/snapshots/SYNTHETIC-blank-snapshot.json apple/Tests/DCXLogicHelperCoreTests/Fixtures/feedback

lockfiles-static:
    test -s Cargo.lock
    test -s MODULE.bazel.lock
    test -s flake.lock
    git ls-files --error-unmatch Cargo.lock MODULE.bazel.lock flake.lock >/dev/null

lockfiles: lockfiles-static
    cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null

check: fmt-check lint test fixtures lockfiles frontdoor-check

bazel-check:
    #!/usr/bin/env bash
    set -euo pipefail
    build_targets=(//:dcxctl //crates/dcx-darwin-tty:dcx_darwin_tty)
    if [[ $(uname -s) == Darwin ]]; then
        build_targets+=(//:dcxctl_live_control)
    fi
    bash scripts/run_bazelisk.sh test --config=host-gates //:check
    bash scripts/run_bazelisk.sh build "${build_targets[@]}"

# Hosted static feedback performs no Rust compile/test or Bazel execution.
hosted-advisory: fmt-check lint-static schema-check lockfiles-static

nix-check:
    nix flake check

# Materialize the aarch64-darwin live control binary without profile activation.
live-package:
    nix build .#dcxctl-live --no-link

# Compile and test the shared Swift bridge/helper/AU libraries without launching a GUI.
apple-package-check:
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"; \
      /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
      "$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" \
      test --package-path apple --jobs 1

# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13:
# Named Darwin-only recovery suites, one Swift job, owned fixture lease stores.
native-mutation-recovery:
    bash scripts/run_bazelisk.sh test //apple:mutation_recovery

# dec-native-studio-20261004 / R-N13: real owned child and recovery fixtures.
native-process-runner:
    bash scripts/run_bazelisk.sh test //apple:process_runner

# Explicit first-Apply admission and zero-contact document recall fixtures.
native-logic-recall:
    bash scripts/run_bazelisk.sh test //apple:logic_recall

# Pure correlation of replies with the exact request and readback digest.
native-response-binding:
    bash scripts/run_bazelisk.sh test //apple:response_binding

# Bridge v2 notch surface: v2 bindings, helper planning, AU staging, real dcxctl.
native-feedback-notch:
    bash scripts/run_bazelisk.sh test //apple:feedback_notch

# Parse the checked-in bridge schemas without introducing another validator stack.
# v1 is the frozen prior wire; v2 is current.
schema-check:
    jq -e . schemas/v1/dcx_logic_bridge.schema.json >/dev/null
    jq -e . schemas/v2/dcx_logic_bridge.schema.json >/dev/null

# Regenerate the ignored Xcode project from the checked-in XcodeGen source.
apple-project-generate:
    xcodegen generate --spec apple/project.yml

# Build the helper, embedded AUv3, and embedded dcxctl without signing or install.
apple-bundle-check: apple-project-generate
    # Resolve the compiler from Xcode itself: inside a Nix shell, xcrun can return a wrapped Nix clang.
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"; \
      xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"; \
      dcx_cargo_bin="$(command -v cargo)"; \
      dcx_rustc_bin="$(command -v rustc)"; \
      DEVELOPER_DIR="$xcode_developer_dir" "$xcode_developer_dir/usr/bin/xcodebuild" -project apple/DCXLogic.xcodeproj -scheme DCXLogicHelper -configuration Debug -derivedDataPath apple/.derived -jobs 1 ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO CC="$xcode_toolchain/clang" CPLUSPLUS="$xcode_toolchain/clang++" LD="$xcode_toolchain/clang" LDPLUSPLUS="$xcode_toolchain/clang++" DCX_CARGO_BIN="$dcx_cargo_bin" DCX_RUSTC_BIN="$dcx_rustc_bin" build

# Build and verify one arm64 ad-hoc test carrier for helper-absent AU discovery/auval.
# Its qualification-only entitlements deliberately omit the team-bound App Group
# and serial access; this does not prove helper IPC, install/register, or launch a GUI.
apple-adhoc-bundle: apple-project-generate
    #!/usr/bin/env bash
    set -euo pipefail
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
    xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"
    dcx_cargo_bin="$(command -v cargo)"
    dcx_rustc_bin="$(command -v rustc)"
    derived_data="apple/.derived-adhoc"
    archive_path="$derived_data/DCXLogicHelper.xcarchive"
    DEVELOPER_DIR="$xcode_developer_dir" "$xcode_developer_dir/usr/bin/xcodebuild" \
      -project apple/DCXLogic.xcodeproj \
      -scheme DCXLogicHelper \
      -configuration Release \
      -destination 'generic/platform=macOS' \
      -derivedDataPath "$derived_data" \
      -archivePath "$archive_path" \
      -jobs 1 \
      ARCHS=arm64 \
      ONLY_ACTIVE_ARCH=YES \
      CODE_SIGNING_ALLOWED=NO \
      CC="$xcode_toolchain/clang" \
      CPLUSPLUS="$xcode_toolchain/clang++" \
      LD="$xcode_toolchain/clang" \
      LDPLUSPLUS="$xcode_toolchain/clang++" \
      DCX_CARGO_BIN="$dcx_cargo_bin" \
      DCX_RUSTC_BIN="$dcx_rustc_bin" \
      archive
    app="$archive_path/Products/Applications/DCXLogicHelper.app"
    au="$app/Contents/PlugIns/DCXControlAU.appex"
    child="$app/Contents/Helpers/dcxctl"
    app_binary="$app/Contents/MacOS/DCXLogicHelper"
    au_binary="$au/Contents/MacOS/DCXControlAU"
    test -d "$app"
    test -d "$au"
    /usr/bin/codesign \
      --force --sign - --options runtime --timestamp=none \
      --entitlements apple/DCXLogicHelper/dcxctl.entitlements \
      "$child"
    /usr/bin/codesign \
      --force --sign - --options runtime --timestamp=none \
      --entitlements apple/DCXControlAU/DCXControlAU.adhoc.entitlements \
      "$au"
    /usr/bin/codesign \
      --force --sign - --options runtime --timestamp=none \
      --entitlements apple/DCXLogicHelper/DCXLogicHelper.adhoc.entitlements \
      "$app"
    for binary in "$app_binary" "$au_binary" "$child"; do
      test -x "$binary"
      test "$(/usr/bin/lipo -archs "$binary")" = arm64
    done
    verify_adhoc() {
      local artifact="$1" signature
      /usr/bin/codesign --verify --strict "$artifact"
      signature="$(/usr/bin/codesign -dvvv "$artifact" 2>&1)"
      printf '%s\n' "$signature" | /usr/bin/grep -qx 'Signature=adhoc'
      printf '%s\n' "$signature" | /usr/bin/grep -qx 'TeamIdentifier=not set'
      ! printf '%s\n' "$signature" | /usr/bin/grep -q '^Authority='
    }
    verify_adhoc "$child"
    verify_adhoc "$au"
    verify_adhoc "$app"
    /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"
    test ! -e "$app/Contents/embedded.provisionprofile"
    test ! -e "$au/Contents/embedded.provisionprofile"
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = io.tinyland.dcx2496.logic
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$au/Contents/Info.plist")" = io.tinyland.dcx2496.logic.midi
    extract_entitlements_json() {
      /usr/bin/codesign --display --entitlements - --xml "$1" 2>/dev/null \
        | /usr/bin/plutil -convert json -o - -
    }
    app_entitlements="$(extract_entitlements_json "$app")"
    au_entitlements="$(extract_entitlements_json "$au")"
    child_entitlements="$(extract_entitlements_json "$child")"
    printf '%s' "$app_entitlements" | /usr/bin/jq -e '
      ."com.apple.security.app-sandbox" == true and
      (has("com.apple.security.device.serial") | not) and
      (has("com.apple.security.application-groups") | not)
    ' >/dev/null
    printf '%s' "$au_entitlements" | /usr/bin/jq -e '
      ."com.apple.security.app-sandbox" == true and
      ."com.apple.security.files.user-selected.read-only" == true and
      (has("com.apple.security.application-groups") | not) and
      (has("com.apple.security.device.serial") | not)
    ' >/dev/null
    printf '%s' "$child_entitlements" | /usr/bin/jq -e '
      ."com.apple.security.app-sandbox" == true and
      ."com.apple.security.inherit" == true
    ' >/dev/null
    info="$au/Contents/Info.plist"
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:type' "$info")" = aumi
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:subtype' "$info")" = DcxC
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:manufacturer' "$info")" = TnLd
    printf 'adhoc_bundle=ready\n'

# Build the containing app unsigned, then sign the app, AUv3, and dcxctl with
# one exact Apple Development or Developer ID Application identity from the
# fixed team. macOS team-prefixed App Groups do not require embedded
# provisioning profiles.
#
# Set DCX_CODESIGN_IDENTITY to one exact certificate SHA-1 or full identity
# label when more than one valid team identity is visible. Apple Development
# labels may carry a member identifier rather than the signing TeamIdentifier;
# an explicit match is therefore accepted only subject to the post-sign team
# verification below. Set
# DCX_CODESIGN_KEYCHAIN to restrict discovery and signing to one temporary
# host-owned keychain. For a noninteractive file keychain, set
# DCX_CODESIGN_KEYCHAIN_PASSWORD_FILE to a caller-owned password file so the
# recipe can restore the keychain's unlocked state after the unsigned archive.
# This recipe never imports a signing credential, installs/registers, or
# launches a GUI.
apple-team-signed-bundle: apple-project-generate
    #!/usr/bin/env bash
    set -euo pipefail
    team_id="QP994XQKNH"
    app_group="QP994XQKNH.io.tinyland.dcx2496"
    requested_signer="${DCX_CODESIGN_IDENTITY:-}"
    keychain="${DCX_CODESIGN_KEYCHAIN:-}"
    keychain_password_file="${DCX_CODESIGN_KEYCHAIN_PASSWORD_FILE:-}"
    unset DCX_CODESIGN_KEYCHAIN_PASSWORD_FILE
    security_args=(find-identity -v -p codesigning)
    codesign_keychain_args=()
    if [[ -n "$keychain" ]]; then
      security_args+=("$keychain")
      codesign_keychain_args=(--keychain "$keychain")
    fi
    if [[ -n "$keychain_password_file" ]]; then
      test -n "$keychain"
      test -r "$keychain_password_file"
    fi
    unlock_selected_keychain() {
      if [[ -n "$keychain_password_file" ]]; then
        local unlock_token
        unlock_token="$(<"$keychain_password_file")"
        test -n "$unlock_token"
        /usr/bin/security unlock-keychain -p "$unlock_token" "$keychain"
        unset unlock_token
      fi
    }
    unlock_selected_keychain
    identity_lines="$(/usr/bin/security "${security_args[@]}")"
    eligible_signers="$(
      printf '%s\n' "$identity_lines" \
        | /usr/bin/awk '
            $0 ~ /"(Apple Development|Developer ID Application):/ {
              hash = toupper($2)
              label = $0
              sub(/^[^"]*"/, "", label)
              sub(/"[^"]*$/, "", label)
              print hash "\t" label
            }
          '
    )"
    team_signers="$(
      printf '%s\n' "$eligible_signers" \
        | /usr/bin/awk -F '\t' -v team="$team_id" 'index($2, "(" team ")") { print $0 }'
    )"
    team_signer_count="$(
      printf '%s\n' "$team_signers" \
        | /usr/bin/sed '/^$/d' \
        | /usr/bin/wc -l \
        | /usr/bin/tr -d ' '
    )"
    if [[ -z "$requested_signer" ]]; then
      test "$team_signer_count" = 1
      signer_record="$team_signers"
    else
      signer_record="$(
        printf '%s\n' "$eligible_signers" \
          | /usr/bin/awk -F '\t' -v requested="$requested_signer" '
              toupper(requested) == $1 || requested == $2 {
                print $0
              }
            '
      )"
      signer_match_count="$(
        printf '%s\n' "$signer_record" \
          | /usr/bin/sed '/^$/d' \
          | /usr/bin/wc -l \
          | /usr/bin/tr -d ' '
      )"
      test "$signer_match_count" = 1
    fi
    signer="${signer_record%%$'\t'*}"
    signer_label="${signer_record#*$'\t'}"
    [[ "$signer" =~ ^[0-9A-F]{40}$ ]]
    case "$signer_label" in
      "Apple Development:"*|"Developer ID Application:"*) ;;
      *) exit 1 ;;
    esac
    if [[ -z "$requested_signer" ]]; then
      [[ "$signer_label" == *"($team_id)" ]]
    fi
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
    xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"
    dcx_cargo_bin="$(command -v cargo)"
    dcx_rustc_bin="$(command -v rustc)"
    derived_data="apple/.derived-team-signed"
    archive_path="$derived_data/DCXLogicHelper.xcarchive"
    DEVELOPER_DIR="$xcode_developer_dir" "$xcode_developer_dir/usr/bin/xcodebuild" \
      -project apple/DCXLogic.xcodeproj \
      -scheme DCXLogicHelper \
      -configuration Release \
      -destination 'generic/platform=macOS' \
      -derivedDataPath "$derived_data" \
      -archivePath "$archive_path" \
      -jobs 1 \
      ARCHS=arm64 \
      ONLY_ACTIVE_ARCH=YES \
      CODE_SIGNING_ALLOWED=NO \
      CODE_SIGN_STYLE=Manual \
      DEVELOPMENT_TEAM="$team_id" \
      CC="$xcode_toolchain/clang" \
      CPLUSPLUS="$xcode_toolchain/clang++" \
      LD="$xcode_toolchain/clang" \
      LDPLUSPLUS="$xcode_toolchain/clang++" \
      DCX_CARGO_BIN="$dcx_cargo_bin" \
      DCX_RUSTC_BIN="$dcx_rustc_bin" \
      archive
    unlock_selected_keychain
    app="$archive_path/Products/Applications/DCXLogicHelper.app"
    au="$app/Contents/PlugIns/DCXControlAU.appex"
    child="$app/Contents/Helpers/dcxctl"
    app_binary="$app/Contents/MacOS/DCXLogicHelper"
    au_binary="$au/Contents/MacOS/DCXControlAU"
    test -d "$app"
    test -d "$au"
    test ! -e "$app/Contents/embedded.provisionprofile"
    test ! -e "$au/Contents/embedded.provisionprofile"
    /usr/bin/codesign \
      --force --sign "$signer" "${codesign_keychain_args[@]}" \
      --options runtime --timestamp=none \
      --entitlements apple/DCXLogicHelper/dcxctl.entitlements \
      "$child"
    /usr/bin/codesign \
      --force --sign "$signer" "${codesign_keychain_args[@]}" \
      --options runtime --timestamp=none \
      --entitlements apple/DCXControlAU/DCXControlAU.entitlements \
      "$au"
    /usr/bin/codesign \
      --force --sign "$signer" "${codesign_keychain_args[@]}" \
      --options runtime --timestamp=none \
      --entitlements apple/DCXLogicHelper/DCXLogicHelper.entitlements \
      "$app"
    for binary in "$app_binary" "$au_binary" "$child"; do
      test -x "$binary"
      test "$(/usr/bin/lipo -archs "$binary")" = arm64
    done
    for artifact in "$app" "$au" "$child"; do
      /usr/bin/codesign --verify --strict "$artifact"
      signature="$(/usr/bin/codesign -dvvv "$artifact" 2>&1)"
      observed_team="$(printf '%s\n' "$signature" | /usr/bin/awk -F= '$1 == "TeamIdentifier" {print $2}')"
      test "$observed_team" = "$team_id"
      leaf_authority="$(printf '%s\n' "$signature" | /usr/bin/awk -F= '$1 == "Authority" {print $2; exit}')"
      case "$leaf_authority" in
        "Apple Development:"*|"Developer ID Application:"*) ;;
        *) exit 1 ;;
      esac
      test "$leaf_authority" = "$signer_label"
    done
    /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = io.tinyland.dcx2496.logic
    test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$au/Contents/Info.plist")" = io.tinyland.dcx2496.logic.midi
    extract_entitlements_json() {
      /usr/bin/codesign --display --entitlements - --xml "$1" 2>/dev/null \
        | /usr/bin/plutil -convert json -o - -
    }
    app_entitlements="$(extract_entitlements_json "$app")"
    au_entitlements="$(extract_entitlements_json "$au")"
    child_entitlements="$(extract_entitlements_json "$child")"
    printf '%s' "$app_entitlements" | /usr/bin/jq -e --arg group "$app_group" '
      (keys | sort) == ([
        "com.apple.security.app-sandbox",
        "com.apple.security.application-groups",
        "com.apple.security.device.serial"
      ] | sort) and
      ."com.apple.security.app-sandbox" == true and
      ."com.apple.security.device.serial" == true and
      ."com.apple.security.application-groups" == [$group]
    ' >/dev/null
    printf '%s' "$au_entitlements" | /usr/bin/jq -e --arg group "$app_group" '
      (keys | sort) == ([
        "com.apple.security.app-sandbox",
        "com.apple.security.application-groups",
        "com.apple.security.files.user-selected.read-only"
      ] | sort) and
      ."com.apple.security.app-sandbox" == true and
      ."com.apple.security.application-groups" == [$group] and
      ."com.apple.security.files.user-selected.read-only" == true
    ' >/dev/null
    printf '%s' "$child_entitlements" | /usr/bin/jq -e '
      (keys | sort) == ([
        "com.apple.security.app-sandbox",
        "com.apple.security.inherit"
      ] | sort) and
      ."com.apple.security.app-sandbox" == true and
      ."com.apple.security.inherit" == true
    ' >/dev/null
    info="$au/Contents/Info.plist"
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:type' "$info")" = aumi
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:subtype' "$info")" = DcxC
    test "$(/usr/libexec/PlistBuddy -c 'Print :NSExtension:NSExtensionAttributes:AudioComponents:0:manufacturer' "$info")" = TnLd
    printf 'team_signed_bundle=ready\n'

# Full product-source compile without signing, install, GUI launch, or hardware I/O.
product-check: schema-check apple-bundle-check

security:
    cargo deny check
    gitleaks dir . --no-banner
    gitleaks git . --no-banner
