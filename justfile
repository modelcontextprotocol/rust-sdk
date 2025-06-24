fmt: 
    cargo +nightly fmt --all

check:
    cargo clippy --all-targets --all-features -- -D warnings

fix: fmt
    git add ./
    cargo clippy --fix --all-targets --all-features --allow-staged
    
test:
    cargo test --all-features

cov:
    cargo llvm-cov --lcov --output-path {{justfile_directory()}}/target/llvm-cov-target/coverage.lcov