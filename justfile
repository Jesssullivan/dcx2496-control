set dotenv-load := false
set shell := ["bash", "-euo", "pipefail", "-c"]

mod repo 'just/repo.just'

default: check

check: repo::check

bazel-check: repo::bazel-check

nix-check: repo::nix-check

security: repo::security

transport-check: repo::transport-check

darwin-carrier-check: repo::darwin-carrier-check

live-package: repo::live-package

apple-package-check: repo::apple-package-check

schema-check: repo::schema-check

apple-project-generate: repo::apple-project-generate

apple-bundle-check: repo::apple-bundle-check

product-check: repo::product-check

dcxctl *args:
    cargo run --quiet --package dcxctl -- {{args}}
