// The memory map is shared with the images, so it's defined in one place
#[path = "memory_map.rs"]
mod memory_map;

use memory_map::*;

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Link each hart's image with our linker script, at the start of its own region
    for (bin, image_base, region_base) in [
        ("core0", CORE0_BASE, CORE0_BASE),
        ("core1", CORE1_BASE, CORE1_BASE),
    ] {
        println!("cargo::rustc-link-arg-bin={bin}=-T{dir}/linker.ld");
        println!("cargo::rustc-link-arg-bin={bin}=--defsym=IMAGE_BASE={image_base:#x}");
        println!("cargo::rustc-link-arg-bin={bin}=--defsym=REGION_BASE={region_base:#x}");
        println!("cargo::rustc-link-arg-bin={bin}=--defsym=REGION_SIZE={REGION_SIZE:#x}");
    }
    println!("cargo::rerun-if-changed=linker.ld");
    println!("cargo::rerun-if-changed=memory_map.rs");
}
