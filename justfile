set dotenv-load := false
set shell := ["bash", "-euo", "pipefail", "-c"]

mod repo 'just/repo.just'

# The GloriousFlywheel consumer front door, vendored from GF `kit/frontdoor`
# rather than re-implemented. It owns no global setting, endpoint, token, or
# runner enrollment; `gloriousflywheel-bazel` is an operator tool, so the
# import is optional and the recipes it defines are absent where it is.
import? 'justfile.flywheel'

default: check

check: repo::check

test-unit: repo::test-unit

bazel-check: repo::bazel-check

hosted-advisory: repo::hosted-advisory

nix-check: repo::nix-check

security: repo::security

transport-check: repo::transport-check

darwin-carrier-check: repo::darwin-carrier-check

live-package: repo::live-package

apple-package-check: repo::apple-package-check

schema-check: repo::schema-check

apple-project-generate: repo::apple-project-generate

apple-bundle-check: repo::apple-bundle-check

apple-adhoc-bundle: repo::apple-adhoc-bundle

apple-team-signed-bundle: repo::apple-team-signed-bundle

product-check: repo::product-check

dcxctl *args:
    cargo run --quiet --package dcxctl -- {{args}}
