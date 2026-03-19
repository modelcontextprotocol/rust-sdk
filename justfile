fmt: 
    cargo +nightly fmt --all

check:
    cargo clippy --all-targets --all-features -- -D warnings

fix: fmt
    git add ./
    cargo clippy --fix --all-targets --all-features --allow-staged
    
test:
    cargo test --all-features
    if command -v jq > /dev/null 2>&1; then \
      FEATURES=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '[.packages[] | select(.name == "rmcp") | .features | keys[] \
                  | select(startswith("__") | not) \
                  | select(. != "local")] | join(",")') && \
      cargo test -p rmcp --features "$FEATURES"; \
    else \
      echo "warning: jq not found, skipping non-local feature tests"; \
    fi

cov:
    cargo llvm-cov --lcov --output-path {{justfile_directory()}}/target/llvm-cov-target/coverage.lcov