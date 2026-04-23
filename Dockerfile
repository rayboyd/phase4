FROM rust:1.87

RUN apt-get update -qq && \
    apt-get install -y -qq libasound2-dev pkg-config && \
    rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt
RUN cargo install cargo-audit cargo-deny git-cliff

WORKDIR /workspace
