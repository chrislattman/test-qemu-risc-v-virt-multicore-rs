// Startup code for both harts' images, included by core0.rs and core1.rs with global_asm!, which
// substitutes {core_id}, {core1_base}, {msip1} and {mip_msip}

.section ".text.boot", "ax"
.global _start

// QEMU starts every hart at 0x80000000 (core0) in machine mode (M-mode), with a0 = hart ID and
// a1 = device tree. Hart 1 waits in park_hart1 until hart 0 starts it, then starts here in core1
_start:
    .cfi_startproc
    .cfi_undefined ra           // Outermost frame for debuggers
    csrr    t0, mhartid
.if {core_id} == 0
    li      t1, 1
    beq     t0, t1, park_hart1
.endif
    // Each image only runs on its own hart: put any other hart into an infinite sleep loop
    li      t1, {core_id}
    beq     t0, t1, this_hart
hang:
    wfi
    j       hang

this_hart:
    // Set up the global pointer, which the linker uses to shorten accesses to small data
    // (relaxation must be off here, or `la gp` would be relaxed into a gp-relative load of itself)
.option push
.option norelax
    la      gp, __global_pointer$
.option pop

    // Set the trap vector to trap_entry (direct mode: every trap goes to the same address)
    la      t0, trap_entry
    csrw    mtvec, t0

    // Set up this hart's stack, inside its own private region
    la      sp, _stack_top

    // Zero .bss, which Rust requires for zero-initialized statics
    la      t0, __bss_start
    la      t1, __bss_end
zero_bss:
    bgeu    t0, t1, zero_bss_done
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       zero_bss
zero_bss_done:

    // Jump straight to main architecture loop (interrupts stay off until main_app enables them)
    call    main_app

    // Defensive fallback hang loop
    j       hang
    .cfi_endproc

.if {core_id} == 0
// Hart 1 waits here, in core0, until hart 0 sets its software interrupt (start_hart1 in core0.rs),
// then jumps to core1 at CORE1_BASE. Only the MSIP bit is set in mie and mstatus.MIE stays clear,
// so the interrupt wakes wfi without trapping. This only uses CSRs and the CLINT (no stack or
// memory), and hart 1 leaves before its own PMP locks it out of this region
park_hart1:
    .cfi_startproc
    .cfi_undefined ra
    li      t0, {mip_msip}
    csrw    mie, t0
1:  wfi                         // wfi may also return without an interrupt, so check mip
    csrr    t0, mip
    andi    t0, t0, {mip_msip}
    beqz    t0, 1b
    li      t0, {msip1}
    sw      zero, 0(t0)         // Acknowledge the software interrupt
    csrw    mie, zero
    li      t0, {core1_base}
    jr      t0
    .cfi_endproc
.endif

// --- Trap entry ---
// mtvec requires 4-byte alignment in direct mode. On a trap, the hart saves the PC in mepc and the
// cause in mcause, clears mstatus.MIE (masking interrupts), and jumps here
.align 2
trap_entry:
    .cfi_startproc
    .cfi_undefined ra           // Outermost frame for debuggers: return address is in mepc, not ra
    // Context Switch Save Pass: Push caller-saved registers onto the stack (callee-saved registers
    // are preserved by handle_trap itself). RV64IMAC has no FP registers to save
    addi    sp, sp, -128
    sd      ra,   0(sp)
    sd      t0,   8(sp)
    sd      t1,  16(sp)
    sd      t2,  24(sp)
    sd      a0,  32(sp)
    sd      a1,  40(sp)
    sd      a2,  48(sp)
    sd      a3,  56(sp)
    sd      a4,  64(sp)
    sd      a5,  72(sp)
    sd      a6,  80(sp)
    sd      a7,  88(sp)
    sd      t3,  96(sp)
    sd      t4, 104(sp)
    sd      t5, 112(sp)
    sd      t6, 120(sp)

    csrr    a0, mcause          // Pass the trap cause,
    csrr    a1, mtval           // the faulting address (for access faults),
    csrr    a2, mepc            // and the trapping instruction as handle_trap's arguments
    call    handle_trap         // Run our Rust function

    // Context Switch Restore Pass: Pop registers cleanly off stack memory
    ld      ra,   0(sp)
    ld      t0,   8(sp)
    ld      t1,  16(sp)
    ld      t2,  24(sp)
    ld      a0,  32(sp)
    ld      a1,  40(sp)
    ld      a2,  48(sp)
    ld      a3,  56(sp)
    ld      a4,  64(sp)
    ld      a5,  72(sp)
    ld      a6,  80(sp)
    ld      a7,  88(sp)
    ld      t3,  96(sp)
    ld      t4, 104(sp)
    ld      t5, 112(sp)
    ld      t6, 120(sp)
    addi    sp, sp, 128
    mret                        // Machine-mode trap return back to your super loop (restores MIE)
    .cfi_endproc
