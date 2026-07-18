set fallback

import '../justfile'

[private]
a:
    @just -l

# install a Rust binary
[group('builds')]
install:
    @cargo install --locked --force --path crates/penknife

# Run the full workspace test suite.
[group('checks')]
test:
    cargo test --workspace --all-features

# fmt + clippy the way CI does (warnings are errors).
[group('checks')]
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Generate coverage/lcov.info (matches the CI Coverage job; needs cargo-llvm-cov,
# install with `cargo install cargo-llvm-cov` or `taiki-e/install-action`).
[group('coverage')]
coverage:
    mkdir -p coverage
    cargo llvm-cov --workspace --all-features --lcov --output-path coverage/lcov.info

# A human-readable coverage summary (per file), no file written.
[group('coverage')]
coverage-summary:
    cargo llvm-cov --workspace --all-features --summary-only

# The uncovered lines per file, for finding gaps to test.
[group('coverage')]
coverage-missing:
    cargo llvm-cov report --show-missing-lines
