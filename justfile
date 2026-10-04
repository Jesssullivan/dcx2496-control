set dotenv-load := false
set shell := ["bash", "-euo", "pipefail", "-c"]

# dec-local-first-reapi-20261004: bound Cargo inherited by frontend children.
export CARGO_BUILD_JOBS := "2"

mod repo 'just/repo.just'

default: check

check: repo::check

test-unit: repo::test-unit

build: repo::build

[script]
[positional-arguments]
bazel command *args:
    exec just -- repo::bazel "$@"

[script]
[positional-arguments]
remote-build *targets:
    exec just -- repo::remote-build "$@"

[script]
[positional-arguments]
remote-test *targets:
    exec just -- repo::remote-test "$@"

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

[script]
[positional-arguments]
dcxctl *args:
    exec cargo run --locked --jobs 2 --quiet --package dcxctl -- "$@"
