# no_std Rust example for QEMU RISC-V "virt" machine with two harts (RV64IMAC)

The Rust counterpart of [test-qemu-risc-v-virt-multicore](../test-qemu-risc-v-virt-multicore), and a two-hart version of [test-qemu-risc-v-virt-rs](../test-qemu-risc-v-virt-rs). Each hart runs its own image from its own private 2 MB region of RAM, and locks its own PMP (Physical Memory Protection, RISC-V's MPU) so it can only access its own region, a shared region and the devices it uses. If either hart touches the other hart's region, it gets an access fault, even in machine mode.

- Hart 0 owns the UART and the PLIC (through the `uart_16550` and `riscv-peripheral` crates, as in the single-hart version). It runs the same Hello/World loop and interrupt cascade, starts hart 1, and prints hart 1's messages.
- Hart 1 uses no peripherals except the CLINT timer. It posts a message to a mailbox in the shared region every 3 seconds, and hart 0 prints it after each loop iteration (every 2 seconds), so hart 1's messages can appear up to 2 seconds late.

Run `sudo apt install qemu-system-riscv gdb-multiarch`

Build and run (`.cargo/config.toml` runs QEMU through `qemu.sh`):

```
cargo qemu        # release build; in another shell: telnet localhost 5678
cargo qemu-debug  # debug build; QEMU halts and waits for GDB on localhost:1234
```

The debug build skips the interrupts and sleeps like the single-hart version. The VS Code "Rust: Remote Attach" configuration loads both images' symbols (`add-symbol-file` for `core1`), and each hart shows up as a thread. It also sets `scheduler-locking step`, so stepping one hart keeps the other halted. Otherwise GDB resumes both harts for every step, and a breakpoint on the other hart can cut the step short. Continue still resumes both. QEMU has no non-stop mode, so whenever one hart is stopped, the other is too. `.cargo/config.toml` has the equivalent GDB commands. No GCC toolchain is needed, as in the single-hart version.

If either hart touches the other hart's region, it reports the fault and halts, for example:

```
hart 1: store access fault at 0x80000000, mcause 0x7, PC 0x802006B8, halting
```

## Memory map and PMP entries

| Region | Address | Contents | PMP entry |
|---|---|---|---|
| Hart 0 | 0x80000000 | `core0` (QEMU starts every hart here), its stack | Hart 0: entry 0, RWX |
| Hart 1 | 0x80200000 | `core1`, its stack | Hart 1: entry 0, RWX |
| Shared | 0x80400000 | The mailbox | Both: entry 1, RW |
| CLINT | 0x02000000 | `mtime`, software interrupts | Both: entry 2, RW |
| PLIC | 0x0C000000 | Interrupt controller | Hart 0: entry 3, RW |
| UART | 0x10000000 | NS16550A | Hart 0: entry 4, RW |
| Everything else | | | Both: entry 7, no access |

The addresses are defined once, in `memory_map.rs`. Both images use it as a module, and `build.rs` includes it too, to pass each image's addresses to the linker (`--defsym`, with `cargo::rustc-link-arg-bin`). `linker.ld` asserts that each image, including its stack, fits in its region. Hart 0's `pmpcfg0` ends up as `0x9800009B9B9B9B9F`, where each byte is one entry's configuration: `0x9F` is locked + NAPOT + RWX, `0x9B` is locked + NAPOT + RW, and `0x98` (entry 7) is locked + NAPOT with no permissions.

## How it works

- **Two binaries.** `core0.rs` and `core1.rs` are separate crate roots (`[[bin]]` targets), and each includes `common.rs`, `memory_map.rs` and `pmp.rs` as modules with its own `CORE_ID` constant. Each image has its own copy of every static and its own stack, all inside its own region. `startup.S` is shared too: `global_asm!` substitutes `{core_id}` and the constants `park_hart1` needs.
- **Building and running both.** `cargo run` only builds the binary it runs (`core0`, the `default-run`), so the runner, `qemu.sh`, builds `core1` with the same profile before starting QEMU. QEMU loads `core0` with `-kernel` and `core1` with `-device loader,file=...`, which only copies it into memory.
- **Starting hart 1.** With `-bios none`, QEMU starts every hart at 0x80000000, which is `core0`. Hart 1 waits in `park_hart1` (in `startup.S`, assembled only into `core0`) until hart 0 sets hart 1's CLINT software interrupt (`start_hart1` in `core0.rs`, using the `msip1()` register that `clint_codegen!` generates from its `harts` list), then jumps to `core1` at 0x80200000. This plays the part of PSCI `CPU_ON` on Arm (with OpenSBI, it would be the SBI HSM `hart_start` call).
- **One PMP per hart.** Each hart has its own `pmpcfg`/`pmpaddr` CSRs, and each hart sets its own in `pmp_init` (`pmp.rs`, using the `riscv` crate's `pmpaddrN::write` and `pmpcfg0::set_pmp`). When several entries match an address, the lowest-numbered one wins, so entry 7, which covers the whole address space with no permissions, only applies to addresses no earlier entry allows. The protection is a whitelist: nothing allows the other hart's region. `set_pmp` turns each entry on as soon as it's set, so entry 7 goes last.
- **Locked entries.** Normally PMP only restricts supervisor and user mode, and machine mode can access anything. Every entry here is locked, which makes it apply to machine mode too, and makes it read-only until reset. So once a hart has locked its PMP, not even its own code can undo the protection.
- **The mailbox** (`Mailbox` in `common.rs`) sits at the start of the shared region. Its `full` flag is an `AtomicU32` with acquire/release ordering, so the text is always written before the flag is set, and read after it's seen. Messages are `&str`s; `FixedBuf` formats messages with `write!` without a heap.
- **Faults** go to `trap_entry` in `startup.S`, which passes `mcause`, `mtval` (the faulting address) and `mepc` to `handle_trap`. Hart 0 prints the fault itself; hart 1 posts it to the mailbox. Hart 1's panic handler also reports through the mailbox.

## Limitations

- **Locked entries can't be changed until reset.** That's what makes this enforceable in machine mode, but it also means the memory map is fixed once each hart starts. For protection that can change at run time, run the application code in supervisor or user mode and leave the entries unlocked, so a machine-mode monitor (like OpenSBI) can change them. In supervisor mode, each hart could also use its own Sv39 page tables (`satp`), like the Arm version's MMU.
- **Rust's guarantees stop at each image.** The compiler checks each binary on its own, so it can't see that both harts share the mailbox. That's why `Mailbox` uses atomics and `UnsafeCell`, with the cross-hart rules in its `SAFETY` comments.
- **PMP regions must be naturally aligned powers of two** (NAPOT), or be defined by a pair of entries with top-of-range (TOR) addressing. QEMU gives each hart 16 entries.
- **Hart 1 briefly runs in hart 0's region.** It waits in `park_hart1`, which is in `core0`, and it leaves before locking its own PMP (which would stop it running from there). It only uses CSRs and the CLINT while it waits.
- **The PMP only checks the harts' own accesses.** DMA from devices isn't checked (that needs an IOPMP or IOMMU).
- **QEMU doesn't model caches**, so it can't catch a cache-related bug that would show up on real hardware.
- **Debug builds spin.** Hart 1 waits in `Mailbox::post` for hart 0 to empty the mailbox, and `sleep_ms` busy-waits, so each QEMU vCPU uses a full host core.

## Notes

- See [test-qemu-risc-v-virt-rs](../test-qemu-risc-v-virt-rs) for how the crates map to the Arm version
- Serial output is lost until a telnet client connects; change `wait=off` to `wait=on` in `qemu.sh` to make QEMU wait for the connection before starting
- `-bios none` skips OpenSBI, so the program runs in machine mode starting at the RAM base 0x80000000. `core0` is passed straight to `-kernel` as an ELF, so QEMU loads it at its linked address and jumps to `_start`
