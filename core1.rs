//! Hart 1: uses no peripherals except the CLINT timer. It posts its messages to the mailbox in the
//! shared region, and hart 0 prints them
#![no_std]
#![no_main]

mod common;
mod memory_map;
mod pmp;

use common::{FixedBuf, Mailbox, describe_fault};
use core::fmt::Write;
use core::panic::PanicInfo;

const CORE_ID: usize = 1;

// Core Entry Point Setup (park_hart1 is only in core0, so these last three go unused here)
core::arch::global_asm!(
    include_str!("startup.S"),
    core_id = const CORE_ID,
    core1_base = const 0,
    msip1 = const 0,
    mip_msip = const 0,
);

// Called from trap_entry in startup.S. Hart 1 never enables interrupts, so only synchronous
// exceptions (e.g. an access fault from the PMP) get here: report it and halt
#[unsafe(no_mangle)]
pub extern "C" fn handle_trap(mcause: usize, mtval: usize, mepc: usize) {
    let mut buf = FixedBuf::<120>::new();
    let _ = describe_fault(&mut buf, mcause, mtval, mepc);
    Mailbox::get().post(buf.as_str());
    loop {
        riscv::asm::wfi();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main_app() -> ! {
    pmp::pmp_init();
    let mailbox = Mailbox::get();
    mailbox.post("hart 1: PMP locked\r\n");

    loop {
        mailbox.post("Hello from hart 1\r\n");
        #[cfg(not(debug_assertions))]
        common::sleep_ms(3000);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Hart 1 has no UART, so the panic message goes through the mailbox (truncated to fit)
    let mut buf = FixedBuf::<120>::new();
    let _ = write!(buf, "hart 1: PANIC: {}\r\n", info.message());
    Mailbox::get().post(buf.as_str());
    loop {}
}
