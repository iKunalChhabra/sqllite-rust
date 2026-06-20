# Makefile for sqllite-rust

.PHONY: all build test test-sqlite clean

all: build

build:
	cargo build --release

test:
	cargo test

test-sqlite:
	cargo run -p sqllite-tests -- tests -v

clean:
	cargo clean
