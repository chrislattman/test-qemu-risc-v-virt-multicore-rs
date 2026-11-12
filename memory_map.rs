// Memory map, used by both harts' images and by build.rs (which passes it to the linker).
// Each region is 2 MB, so each fits one NAPOT PMP entry:
//
//   0x80000000  CORE0_BASE   Hart 0: core0 (QEMU starts every hart here), its stack
//   0x80200000  CORE1_BASE   Hart 1: core1, its stack
//   0x80400000  SHARED_BASE  Shared by both harts: the mailbox
//
// Each hart's PMP allows only its own region, the shared region and the devices it uses, so any
// access to the other hart's region is an access fault (see pmp.rs)
#![allow(dead_code)] // Each user needs only some of these

pub const CORE0_BASE: usize = 0x8000_0000;
pub const CORE1_BASE: usize = 0x8020_0000;
pub const SHARED_BASE: usize = 0x8040_0000;
pub const REGION_SIZE: usize = 0x20_0000;
