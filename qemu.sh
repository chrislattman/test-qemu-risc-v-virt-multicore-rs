#!/bin/sh
# Cargo runner (see .cargo/config.toml). `cargo run` only builds core0 and passes its ELF as the
# last argument, so this also builds core1 with the same profile, then starts QEMU with both.
# --gdb (first argument) halts QEMU at startup and waits for GDB on localhost:1234
set -e

gdb=
if [ "$1" = "--gdb" ]; then
    gdb="-S -gdb tcp:localhost:1234"
    shift
fi

core0=$1
dir=$(dirname "$core0")
if [ "$(basename "$dir")" = release ]; then
    "${CARGO:-cargo}" build --quiet --release --bin core1
else
    "${CARGO:-cargo}" build --quiet --bin core1
fi

# -smp 2: two harts. -bios none: no OpenSBI firmware, so every hart starts in machine mode at
# 0x80000000, where -kernel loads core0. The generic loader device copies core1 into hart 1's
# region without starting anything there (hart 0 releases hart 1 into it).
# In another shell: telnet localhost 5678
# shellcheck disable=SC2086
exec qemu-system-riscv64 \
    -M virt \
    -smp 2 \
    -bios none \
    -nographic \
    -serial telnet:localhost:5678,server=on,wait=off \
    -kernel "$core0" \
    -device loader,file="$dir/core1" \
    $gdb
