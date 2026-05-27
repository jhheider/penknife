import '../justfile'

[private]
a:
    @just -l

# install a Rust binary
[group('builds')]
install:
    @cargo install --locked --force --path crates/wm
