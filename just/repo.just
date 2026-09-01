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

# Compile and test the shared Swift bridge/helper/AU libraries without launching a GUI.
apple-package-check:
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"; \
      /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
      "$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" \
      test --package-path apple --jobs 1

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
# one exact Apple Development identity from the fixed team. macOS team-prefixed
# App Groups do not require embedded provisioning profiles.
#
# Set DCX_CODESIGN_IDENTITY to one exact certificate SHA-1 or full identity
# label when more than one valid team identity is visible. Set
# DCX_CODESIGN_KEYCHAIN to restrict discovery and signing to one temporary
# host-owned keychain. This recipe never imports credentials, installs/registers,
# or launches a GUI.
apple-team-development-bundle: apple-project-generate
    #!/usr/bin/env bash
    set -euo pipefail
    team_id="QP994XQKNH"
    app_group="QP994XQKNH.io.tinyland.dcx2496"
    requested_signer="${DCX_CODESIGN_IDENTITY:-}"
    keychain="${DCX_CODESIGN_KEYCHAIN:-}"
    security_args=(find-identity -v -p codesigning)
    codesign_keychain_args=()
    if [[ -n "$keychain" ]]; then
      security_args+=("$keychain")
      codesign_keychain_args=(--keychain "$keychain")
    fi
    identity_lines="$(/usr/bin/security "${security_args[@]}")"
    team_signers="$(
      printf '%s\n' "$identity_lines" \
        | /usr/bin/awk -v team="$team_id" '
            $0 ~ /"Apple Development:/ && index($0, "(" team ")") {
              print $2
            }
          '
    )"
    team_signer_count="$(
      printf '%s\n' "$team_signers" \
        | /usr/bin/sed '/^$/d' \
        | /usr/bin/wc -l \
        | /usr/bin/tr -d ' '
    )"
    if [[ -z "$requested_signer" ]]; then
      test "$team_signer_count" = 1
      signer="$team_signers"
    else
      signer="$(
        printf '%s\n' "$identity_lines" \
          | /usr/bin/awk -v requested="$requested_signer" -v team="$team_id" '
              $0 ~ /"Apple Development:/ && index($0, "(" team ")") {
                hash = toupper($2)
                label = $0
                sub(/^[^"]*"/, "", label)
                sub(/"[^"]*$/, "", label)
                if (toupper(requested) == hash || requested == label) {
                  print hash
                }
              }
            '
      )"
    fi
    [[ "$signer" =~ ^[0-9A-F]{40}$ ]]
    signer_matches="$(
      printf '%s\n' "$team_signers" \
        | /usr/bin/awk -v signer="$signer" '$0 == signer { count += 1 } END { print count + 0 }'
    )"
    test "$signer_matches" = 1
    xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
    xcode_toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"
    dcx_cargo_bin="$(command -v cargo)"
    dcx_rustc_bin="$(command -v rustc)"
    derived_data="apple/.derived-team-development"
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
      printf '%s\n' "$signature" | /usr/bin/grep -q '^Authority=Apple Development:'
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
    printf 'team_development_bundle=ready\n'

# Full product-source compile without signing, install, GUI launch, or hardware I/O.
product-check: schema-check apple-bundle-check

security:
    cargo deny check
    gitleaks dir . --no-banner
    gitleaks git . --no-banner
