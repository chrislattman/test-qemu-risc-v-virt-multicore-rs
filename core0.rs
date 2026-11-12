//! Hart 0: owns the UART and the PLIC. It starts hart 1 and prints hart 1's messages from the
//! mailbox
#![no_std]
#![no_main]
// Debug builds compile out the sleeps and interrupts in main_app, leaving their helpers unused
#![cfg_attr(debug_assertions, allow(dead_code))]

mod common;
mod memory_map;
mod pmp;

use common::{CLINT_BASE, Clint, Mailbox, describe_fault};
use core::arch::asm;
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::num::NonZeroUsize;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};
use uart_16550::Uart16550;
use uart_16550::backend::MmioBackend;
use memory_map::CORE1_BASE;
use uart_16550::spec::registers::IER;

const CORE_ID: usize = 0;

const MIP_MSIP: usize = 1 << 3; // Machine software interrupt pending (mip/mie bit)

// Core Entry Point Setup
core::arch::global_asm!(
    include_str!("startup.S"),
    core_id = const CORE_ID,
    core1_base = const CORE1_BASE,
    msip1 = const CLINT_BASE + 4, // Hart 1's MSIP register
    mip_msip = const MIP_MSIP,
);

// --- QEMU virt Memory Map ---
const UART0_BASE: usize = 0x1000_0000; // NS16550A UART

// PLIC (Platform-Level Interrupt Controller): routes device ("external") interrupts to harts, like
// the GIC distributor + CPU interface. Context 0 is hart 0 in machine mode (context 1 is its S-mode)
riscv_peripheral::plic_codegen!(Plic, base 0x0C00_0000);

/// PLIC interrupt sources used here (QEMU virt numbering)
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[riscv::pac_enum(unsafe ExternalInterruptNumber)]
enum Interrupt {
    Uart0 = 10,
}

/// PLIC priority levels used here (0 means "never interrupt")
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[riscv::pac_enum(unsafe PriorityNumber)]
enum Priority {
    P0 = 0,
    P1 = 1,
}

// mcause values for interrupts (bit 63 set) on RV64
const MCAUSE_INTERRUPT: usize = 1 << 63;
const IRQ_M_SOFT: usize = 3; // Machine software interrupt (CLINT MSIP)
const IRQ_M_EXT: usize = 11; // Machine external interrupt (PLIC)

const MSTATUS_MIE: usize = 1 << 3;

// Shared variable across foreground code and async ISR layers
static SHARED_DATA: AtomicU32 = AtomicU32::new(0);

/// A driver used by both main_app and handle_trap. The drivers need `&mut self`, so access is
/// only handed out with interrupts masked, which on this hart makes it exclusive.
struct IrqShared<T>(UnsafeCell<T>);

// SAFETY: Only hart 0 uses these (hart 1's PMP doesn't even allow the UART), and `with` masks the
// only other context (interrupts)
unsafe impl<T> Sync for IrqShared<T> {}

impl<T> IrqShared<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// Runs `f` with interrupts masked, then restores the previous mask (so it's safe inside
    /// handle_trap). `f` must not call `with` on the same value again.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let prev = irq_save();
        let result = f(unsafe { &mut *self.0.get() });
        irq_restore(prev);
        result
    }
}

/// The 16550 driver, plus `fmt::Write` (Uart16550 only has byte-level send functions)
struct Uart(Uart16550<MmioBackend>);

impl Uart {
    /// Creates the driver without touching the hardware (uart_init configures it)
    const fn new() -> Self {
        // SAFETY: UART0_BASE is the NS16550A on QEMU virt, with registers 1 byte apart. There's no
        // MMU in machine mode, pmp_init allows it, and nothing else accesses it
        match unsafe {
            Uart16550::new_mmio_from_address(NonZeroUsize::new(UART0_BASE).unwrap(), 1)
        } {
            Ok(uart) => Self(uart),
            Err(_) => panic!("invalid UART address"),
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.send_bytes_exact(s.as_bytes());
        Ok(())
    }
}

static UART: IrqShared<Uart> = IrqShared::new(Uart::new());

fn uart_puts(s: &str) {
    UART.with(|uart| uart.write_str(s).unwrap());
}

// No `nomem` option, so these also act as compiler memory barriers
fn enable_interrupts() {
    unsafe {
        asm!("csrs mstatus, {}", in(reg) MSTATUS_MIE, options(nostack));
    }
}

fn disable_interrupts() {
    unsafe {
        asm!("csrc mstatus, {}", in(reg) MSTATUS_MIE, options(nostack));
    }
}

/// Masks interrupts and returns the previous mstatus.MIE, so it can be restored with irq_restore.
/// Inside a trap handler interrupts are already masked, and unconditionally re-enabling them would
/// allow a nested trap that overwrites mepc/mcause before the handler returns.
fn irq_save() -> usize {
    let prev: usize;
    unsafe {
        asm!("csrrc {}, mstatus, {}", out(reg) prev, in(reg) MSTATUS_MIE, options(nostack));
    }
    prev & MSTATUS_MIE
}

fn irq_restore(prev: usize) {
    unsafe {
        asm!("csrs mstatus, {}", in(reg) prev, options(nostack));
    }
}

/// "Hardware" interrupt: a real device interrupt through the PLIC. Unlike the GIC, the PLIC has no
/// way for software to raise an interrupt, so this enables the UART's "transmit holding register
/// empty" interrupt. The THR is empty whenever the UART is idle, so the UART raises it right away
fn trigger_hw_irq() {
    UART.with(|uart| uart.0.enable_interrupts(IER::THR_EMPTY));
}

/// "Software" interrupt: writing 1 to this hart's MSIP register raises a machine software
/// interrupt. The PLIC only handles device interrupts, so this one comes from the CLINT
fn trigger_soft_irq() {
    Clint::new().msip0().pend();
}

// The Unified Trap Handler, called from trap_entry in startup.S
#[unsafe(no_mangle)]
pub extern "C" fn handle_trap(mcause: usize, mtval: usize, mepc: usize) {
    if mcause & MCAUSE_INTERRUPT == 0 {
        // Synchronous exception (e.g. an access fault from the PMP), so report it and halt. Take
        // over the UART, as in the panic handler, since the fault may have happened inside UART.with
        let _ = describe_fault(&mut Uart::new(), mcause, mtval, mepc);
        loop {
            riscv::asm::wfi();
        }
    }

    let interrupt_id = mcause & !MCAUSE_INTERRUPT;

    if interrupt_id == IRQ_M_EXT {
        let claim = Plic::new().ctx0().claim();

        // Read the claim register to get the pending source (like reading GICC_IAR)
        let Some(source) = claim.claim::<Interrupt>() else {
            return; // Nothing to claim (another context got it first), nothing to complete
        };

        match source {
            Interrupt::Uart0 => {
                // Turn the THR empty interrupt back off, otherwise the UART keeps raising it
                UART.with(|uart| uart.0.disable_interrupts(IER::THR_EMPTY));

                uart_puts("[ISR] 'Hardware' Interrupt tripped! Writing data to shared space...\r\n");
                SHARED_DATA.store(0xAABB_CCDD, Ordering::Relaxed); // Write to shared variable

                // Instantly trigger our secondary "Software" interrupt. mstatus.MIE is cleared
                // while in the trap handler, so it's taken right after mret returns
                trigger_soft_irq();
            }
        }

        // Signal completion by writing back the claimed ID (like writing GICC_EOIR)
        claim.complete(source);
    } else if interrupt_id == IRQ_M_SOFT {
        // Acknowledge: the software interrupt stays pending until MSIP is cleared
        Clint::new().msip0().unpend();

        uart_puts(
            "[ISR] 'Software' Interrupt tripped! Processing data inside a Critical Section...\r\n",
        );

        // --- CRITICAL SECTION BEGIN ---
        let prev = irq_save(); // Lock out incoming requests safely
        let local_copy = SHARED_DATA.load(Ordering::Relaxed);

        // Print proof of successful read
        UART.with(|uart| {
            write!(
                uart,
                "      Critical Read complete! Value is: 0x{local_copy:08X}\r\n"
            )
            .unwrap()
        });

        irq_restore(prev);
        // --- CRITICAL SECTION END ---
    }
}

fn plic_init() {
    let plic = Plic::new();
    let ctx = plic.ctx0();

    // SAFETY (all of these): interrupts are still masked (mstatus.MIE is 0), and handle_trap
    // handles every interrupt enabled here
    unsafe {
        // Give UART0 a non-zero priority, and enable it for hart 0's machine-mode context
        plic.priorities().set_priority(Interrupt::Uart0, Priority::P1);
        ctx.enables().enable(Interrupt::Uart0);

        // Set the threshold to 0 to let every priority above 0 through (like GICC_PMR = 0xFF)
        ctx.threshold().set_threshold(Priority::P0);

        // Enable machine external (PLIC) and machine software (CLINT) interrupts at the hart
        // (sets mie.MEIE and mie.MSIE)
        Clint::new().msip0().unpend(); // No pending software interrupt
        plic.enable();
        Clint::new().mswi().enable();
    }
}

/// Releases hart 1 from park_hart1 in startup.S, which then jumps to core1 at CORE1_BASE. This
/// plays the part of PSCI CPU_ON on Arm (with OpenSBI, it would be the SBI HSM hart_start call)
fn start_hart1() {
    Clint::new().msip1().pend();
}

#[unsafe(no_mangle)]
pub extern "C" fn main_app() -> ! {
    UART.with(|uart| uart.0.init(uart_16550::Config::default()).unwrap());
    pmp::pmp_init();
    uart_puts("hart 0: PMP locked\r\n");

    // Empty the mailbox before hart 1 can use it
    let mailbox = Mailbox::get();
    mailbox.clear();

    start_hart1();
    uart_puts("hart 0: started hart 1\r\n");

    #[cfg(not(debug_assertions))]
    {
        plic_init();
        enable_interrupts(); // Set mstatus.MIE to take machine-mode interrupts
    }
    loop {
        uart_puts("Hello\r\n");
        #[cfg(not(debug_assertions))]
        common::sleep_ms(1000);
        uart_puts("World!\r\n");
        #[cfg(not(debug_assertions))]
        {
            trigger_hw_irq(); // Kick off the hardware -> software cascade
            common::sleep_ms(1000);
        }

        // Print hart 1's message, if it has posted one
        mailbox.take(uart_puts);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    disable_interrupts();
    // Take over the UART rather than going through UART.with, since the panic may have
    // happened inside it. This never returns, so the interrupted access is never resumed.
    let mut uart = Uart::new();
    let _ = write!(
        uart,
        "\r\n!!! PANIC TRIGGERED IN RUST ENGINE (hart 0) !!!\r\n{info}\r\n"
    );
    loop {}
}
