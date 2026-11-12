//! PMP (Physical Memory Protection) is RISC-V's MPU: each hart has its own set of entries (16 on
//! QEMU's default CPU), each with an address CSR (pmpaddrN) and a config byte (byte N of pmpcfg0 on
//! RV64 for entries 0-7). When several entries match an address, the lowest-numbered one wins.
//!
//! Normally PMP only restricts S-mode and U-mode, and machine mode can access anything. Setting an
//! entry's L (lock) bit makes it apply to machine mode too, and the entry can't be changed again
//! until reset. This program runs in machine mode, so every entry here is locked: once set, not
//! even this hart's own code can undo them.
//!
//! Unmatched machine-mode accesses are allowed, so the last entry covers the whole address space
//! with no permissions. The result is a whitelist: anything not allowed by an earlier entry faults

use crate::CORE_ID;
use crate::common::{CLINT_BASE, CLINT_SIZE, OWN_BASE};
use crate::memory_map::*;
use riscv::register::{
    Permission, Range, pmpaddr0, pmpaddr1, pmpaddr2, pmpaddr3, pmpaddr4, pmpaddr7, pmpcfg0,
};

// Devices only hart 0 uses
const PLIC_BASE: usize = 0x0C00_0000;
const PLIC_SIZE: usize = 0x40_0000; // Covers priorities, enables and context 0's threshold/claim
const UART0_BASE: usize = 0x1000_0000;
const UART0_SIZE: usize = 0x100;

const _: () = assert!(REGION_SIZE.is_power_of_two(), "NAPOT regions must be a power of two");

/// pmpaddr for a NAPOT region: the address bits above the size, then a 0, then all ones below it
/// (size must be a power of two, at least 8 bytes, and base must be aligned to it)
const fn napot(base: usize, size: usize) -> usize {
    (base >> 2) | ((size >> 3) - 1)
}

/// NAPOT with every bit set covers the whole address space
const ALL_ADDRESSES: usize = usize::MAX;

/// Sets and locks this hart's PMP entries. Call once, first thing in main_app
pub fn pmp_init() {
    // SAFETY: Every entry allows this hart's own region (code, data, stack) before entry 7 shuts
    // off everything else, so this hart keeps running. Each set_pmp turns its entry on right away,
    // which is why entry 7 goes last
    unsafe {
        // Entry 0: this hart's own region (code, data, stack)
        pmpaddr0::write(napot(OWN_BASE, REGION_SIZE));
        pmpcfg0::set_pmp(0, Range::NAPOT, Permission::RWX, true);

        // Entry 1: the shared region (mailbox), not executable
        pmpaddr1::write(napot(SHARED_BASE, REGION_SIZE));
        pmpcfg0::set_pmp(1, Range::NAPOT, Permission::RW, true);

        // Entry 2: the CLINT (mtime for sleep_ms, and the software interrupt registers)
        pmpaddr2::write(napot(CLINT_BASE, CLINT_SIZE));
        pmpcfg0::set_pmp(2, Range::NAPOT, Permission::RW, true);

        if CORE_ID == 0 {
            // Entries 3 and 4: the PLIC and the UART, which only hart 0 uses
            pmpaddr3::write(napot(PLIC_BASE, PLIC_SIZE));
            pmpcfg0::set_pmp(3, Range::NAPOT, Permission::RW, true);
            pmpaddr4::write(napot(UART0_BASE, UART0_SIZE));
            pmpcfg0::set_pmp(4, Range::NAPOT, Permission::RW, true);
        }

        // Entry 7: everything else, including the other hart's region, with no permissions
        // (entries 5 and 6, and 3 and 4 on hart 1, stay off)
        pmpaddr7::write(ALL_ADDRESSES);
        pmpcfg0::set_pmp(7, Range::NAPOT, Permission::NONE, true);
    }
}
