//! Code shared by both harts' images. Each image compiles its own copy, with its own CORE_ID
#![allow(dead_code)] // Each image uses only some of this

use crate::CORE_ID;
use crate::memory_map::*;
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering};

pub const OWN_BASE: usize = if CORE_ID == 0 { CORE0_BASE } else { CORE1_BASE };

/// Hart IDs on QEMU virt with -smp 2
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[riscv::pac_enum(unsafe HartIdNumber)]
pub enum HartId {
    H0 = 0,
    H1 = 1,
}

// CLINT (Core-Local Interruptor): each hart's timer (mtime runs at 10 MHz on QEMU virt, the device
// tree's timebase-frequency) and machine software interrupt (msip0() and msip1())
riscv_peripheral::clint_codegen!(
    pub Clint,
    base 0x0200_0000,
    mtime_freq 10_000_000,
    harts [HartId::H0 => 0, HartId::H1 => 1]
);
pub const CLINT_BASE: usize = 0x0200_0000;
pub const CLINT_SIZE: usize = 0x1_0000;

pub fn sleep_ms(ms: u32) {
    let mtime = Clint::new().mtimer().mtime();
    let ticks_needed = (Clint::new().mtimer().mtime_freq() as u64 / 1000) * ms as u64;
    let start = mtime.read();
    while mtime.read() - start < ticks_needed {}
}

// --- Mailbox ---

const MAILBOX_TEXT_SIZE: usize = 120;

/// Only hart 0 touches the UART, so hart 1 posts its messages here and hart 0 prints them.
/// It holds one message at a time: hart 1 waits until hart 0 has printed the previous one
#[repr(C)]
pub struct Mailbox {
    full: AtomicU32, // 1 while text holds a message hart 0 hasn't printed yet
    len: UnsafeCell<u32>,
    text: UnsafeCell<[u8; MAILBOX_TEXT_SIZE]>,
}

// SAFETY: len and text are only written by hart 1 while full == 0, and only read by hart 0 while
// full == 1. The acquire/release pair on full orders those accesses between the harts
unsafe impl Sync for Mailbox {}

impl Mailbox {
    /// The mailbox at the start of the shared region
    pub fn get() -> &'static Mailbox {
        // SAFETY: SHARED_BASE is allowed by both harts' PMPs, nothing else uses it,
        // and any bytes there make a valid Mailbox (from_utf8 in take checks the text)
        unsafe { &*(SHARED_BASE as *const Mailbox) }
    }

    /// Empties the mailbox (hart 0 does this before starting hart 1)
    pub fn clear(&self) {
        self.full.store(0, Ordering::Release);
    }

    /// Waits until the mailbox is empty, then posts s (truncated to fit) for hart 0 to print
    pub fn post(&self, s: &str) {
        while self.full.load(Ordering::Acquire) != 0 {}

        let mut n = s.len().min(MAILBOX_TEXT_SIZE);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        // SAFETY: full == 0, so hart 0 isn't reading these
        unsafe {
            let text = &mut *self.text.get();
            text[..n].copy_from_slice(&s.as_bytes()[..n]);
            *self.len.get() = n as u32;
        }

        self.full.store(1, Ordering::Release);
    }

    /// Passes the waiting message, if there is one, to f, then empties the mailbox
    pub fn take(&self, f: impl FnOnce(&str)) {
        if self.full.load(Ordering::Acquire) != 0 {
            // SAFETY: full == 1, so hart 1 isn't writing these
            let text = unsafe {
                let len = (*self.len.get() as usize).min(MAILBOX_TEXT_SIZE);
                let text = &*self.text.get();
                &text[..len]
            };
            f(core::str::from_utf8(text).unwrap_or("<invalid message>\r\n"));
            self.full.store(0, Ordering::Release);
        }
    }
}

/// A fixed-size string buffer for formatting messages without a heap. Text that doesn't fit is
/// dropped
pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    pub fn as_str(&self) -> &str {
        // Only whole &strs (or char-boundary prefixes of them) are ever copied in
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut n = s.len().min(N - self.len);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Describes a synchronous exception from mcause, mtval and mepc, e.g.
/// "hart 1: store access fault at 0x80000000, mcause 0x7, PC 0x80200150, halting"
pub fn describe_fault(w: &mut impl Write, mcause: usize, mtval: usize, mepc: usize) -> fmt::Result {
    write!(w, "hart {CORE_ID}: ")?;
    match mcause {
        1 => write!(w, "instruction access fault at {mtval:#X}")?,
        5 => write!(w, "load access fault at {mtval:#X}")?,
        7 => write!(w, "store access fault at {mtval:#X}")?,
        _ => write!(w, "exception, mtval {mtval:#X}")?,
    }
    write!(w, ", mcause {mcause:#X}, PC {mepc:#X}, halting\r\n")
}
