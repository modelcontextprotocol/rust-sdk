fmt: 
    cargo +nightly fmt --all

check:
    cargo clippy --all-targets --all-features -- -D warnings

fix:
    cargo clippy --fix --all-targets --all-features --allow-staged